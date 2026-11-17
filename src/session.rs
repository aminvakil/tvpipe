//! Orchestration: connect → plan → ffmpeg → fanout → teardown (PLAN §3).
//!
//! Owner: lead.
//!
//! `play` / `test-pattern`:
//!
//! 1. Probe the input (`ffprobe`): size, frame rate, audio or not, finite
//!    (paced with `-re`) or live.
//! 2. Resolve every `--to` (discovery for names; `ADDR[:PORT]` skips it)
//!    and connect all sinks before ffmpeg starts. Cast sinks negotiate
//!    OFFER/ANSWER here, so their capabilities are final.
//! 3. Plan the encode: the user's options (or the input's own size and
//!    rate) clamped by the intersection of the sinks' capabilities.
//! 4. Start ffmpeg and the demuxer; push every frame through the fanout.
//! 5. Tear down on EOF (after one playout delay, so the tail plays), on
//!    Ctrl-C / SIGTERM / `q` (see `util::interrupt`, without draining), or
//!    when every receiver has ended the session. Every sink's `stop()` runs
//!    (Cast: STOP + CLOSE) and ffmpeg is killed and reaped.

use std::ffi::OsString;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cast::{self, CastOptions};
use crate::cli::{Command, InputSpec, PlayOptions, Size};
use crate::error::{Error, Result};
use crate::log::Level;
use crate::media::fanout::{Fanout, FanoutConfig, SinkEnd};
use crate::media::ffmpeg::{self, Output, Source};
use crate::media::source::MediaReader;
use crate::types::{
    AudioCodec, AudioFamily, AudioFormat, AudioPlan, Capabilities, EncodePlan, Encoder, Sink,
    TrackKind, VideoCodec, VideoPlan,
};
use crate::util::interrupt;
use crate::util::ratio::Ratio;

pub fn run(cmd: Command) -> Result<()> {
    match cmd {
        Command::Discover {
            timeout,
            json,
            interfaces,
        } => crate::net::discover::run(timeout, json, &interfaces),
        Command::Pair { target, pin } => crate::airplay::pair::pair(&target, pin.as_deref()),
        Command::Unpair { target } => crate::airplay::pair::unpair(&target),
        Command::Play(opts) | Command::TestPattern(opts) => play(opts),
        Command::DebugPipeline(o) => crate::media::debug::run(&o),
    }
}

/// How long `--to NAME` browses before giving up.
const DISCOVER_TIMEOUT: Duration = Duration::from_secs(4);
/// Cast control port.
const CAST_PORT: u16 = 8009;
/// AirPlay control port (for `ADDR:7000` and `airplay:ADDR`).
const AIRPLAY_PORT: u16 = 7000;
/// Target playout delay without `--latency`. Cast needs room for a
/// retransmission or two on Wi-Fi; 200 ms is openscreen's sender default.
pub const DEFAULT_LATENCY_MS: u32 = 200;
/// Largest default output (an input larger than this is scaled down).
const DEFAULT_MAX_SIZE: Size = Size {
    width: 1920,
    height: 1080,
};
/// Output size when neither `--size` nor the input says.
const FALLBACK_SIZE: Size = Size {
    width: 1280,
    height: 720,
};
const FALLBACK_FPS: Ratio = Ratio::integer(30);
const MAX_DEFAULT_FPS: u32 = 60;
/// Default bitrate: bits per pixel per frame, clamped below.
const DEFAULT_BITS_PER_PIXEL: f64 = 0.1;
const MIN_DEFAULT_KBPS: u32 = 1_000;
const MAX_DEFAULT_KBPS: u32 = 20_000;
/// How long `ffprobe` may take before we go on without it.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// After a stop request, ffmpeg gets this long to exit by itself (it got
/// SIGINT too) before it is killed.
const KILL_GRACE: Duration = Duration::from_millis(300);
/// Opus frames: 10 ms (lowest latency libopus offers without overhead
/// blowing up).
const OPUS_FRAME_SAMPLES: u32 = 480;
const AAC_FRAME_SAMPLES: u32 = 1024;
const AUDIO_KBPS: u32 = 128;

// ---------------------------------------------------------------------------
// Targets

/// A resolved `--to`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Cast { addr: SocketAddr, name: String },
    AirPlay { name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    Cast,
    AirPlay,
}

/// `cast:` / `airplay:` prefixes force a protocol; `ADDR[:PORT]` skips
/// discovery (bare addresses are Cast on 8009, port 7000 is AirPlay).
fn parse_target(query: &str) -> (Option<Protocol>, &str) {
    for (prefix, proto) in [("cast:", Protocol::Cast), ("airplay:", Protocol::AirPlay)] {
        if query.len() > prefix.len() && query[..prefix.len()].eq_ignore_ascii_case(prefix) {
            return (Some(proto), &query[prefix.len()..]);
        }
    }
    (None, query)
}

fn address_target(forced: Option<Protocol>, rest: &str) -> Option<Target> {
    let addr = match rest.parse::<SocketAddr>() {
        Ok(a) => a,
        Err(_) => {
            let ip = rest
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<IpAddr>()
                .ok()?;
            let port = match forced {
                Some(Protocol::AirPlay) => AIRPLAY_PORT,
                _ => CAST_PORT,
            };
            SocketAddr::new(ip, port)
        }
    };
    let proto = forced.unwrap_or(if addr.port() == AIRPLAY_PORT {
        Protocol::AirPlay
    } else {
        Protocol::Cast
    });
    let name = addr.to_string();
    Some(match proto {
        Protocol::Cast => Target::Cast { addr, name },
        Protocol::AirPlay => Target::AirPlay { name },
    })
}

fn device_target(dev: &crate::net::device::Device, forced: Option<Protocol>) -> Result<Target> {
    let name = dev.name.clone();
    let cast = dev
        .cast
        .as_ref()
        .filter(|_| forced != Some(Protocol::AirPlay));
    match cast {
        // AirPlay mirroring is not implemented yet, so a device with both
        // (Samsung, LG) goes over Cast.
        Some(c) if dev.cast_video() => Ok(Target::Cast {
            addr: SocketAddr::new(c.endpoint.addr(), c.endpoint.port),
            name,
        }),
        Some(_) if dev.airplay.is_none() => Err(Error::unsupported(format!(
            "{name} is a Cast audio device (no video output)"
        ))),
        _ if dev.airplay.is_some() && forced != Some(Protocol::Cast) => {
            Ok(Target::AirPlay { name })
        }
        _ => Err(Error::not_found(format!(
            "{name} has no {} endpoint",
            if forced == Some(Protocol::AirPlay) {
                "AirPlay"
            } else {
                "Cast"
            }
        ))),
    }
}

fn resolve_target(query: &str) -> Result<Target> {
    let (forced, rest) = parse_target(query);
    if let Some(t) = address_target(forced, rest) {
        return Ok(t);
    }
    let dev = crate::net::discover::resolve(rest, DISCOVER_TIMEOUT, &[])?
        .ok_or_else(|| Error::not_found(format!("no receiver named {rest:?}")))?;
    let t = device_target(&dev, forced)?;
    debug!("resolved {query:?} to {t:?}");
    Ok(t)
}

// ---------------------------------------------------------------------------
// Input probing

/// What `ffprobe` says about the input. Every field is optional: unknown
/// inputs (capture devices, URLs that time out) keep the defaults.
#[derive(Debug, Clone, Default, PartialEq)]
struct InputInfo {
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<Ratio>,
    /// `None`: unknown (ffmpeg maps `0:a:0?`, so a missing track is fine).
    has_audio: Option<bool>,
    has_video: Option<bool>,
    /// Finite media (a file): paced with `-re` by default.
    duration: Option<f64>,
}

/// Parses `ffprobe -of compact` output (`stream|codec_type=video|width=...`
/// and `format|duration=...` lines).
fn parse_ffprobe(text: &str) -> InputInfo {
    let mut info = InputInfo {
        has_audio: Some(false),
        has_video: Some(false),
        ..InputInfo::default()
    };
    for line in text.lines() {
        let mut fields = line.trim().split('|');
        let section = fields.next().unwrap_or("");
        let kv: Vec<(&str, &str)> = fields.filter_map(|f| f.split_once('=')).collect();
        let get = |k: &str| kv.iter().find(|(key, _)| *key == k).map(|(_, v)| *v);
        match section {
            "stream" => match get("codec_type") {
                Some("video") if info.has_video != Some(true) => {
                    // Cover art and the like are "video" with no frame rate.
                    let fps = get("avg_frame_rate")
                        .and_then(|r| r.parse::<Ratio>().ok())
                        .filter(|r| r.den > 0 && r.num > 0 && r.as_f64() <= 240.0);
                    info.has_video = Some(true);
                    info.width = get("width").and_then(|w| w.parse().ok()).filter(|&w| w > 0);
                    info.height = get("height")
                        .and_then(|h| h.parse().ok())
                        .filter(|&h| h > 0);
                    info.fps = fps;
                }
                Some("audio") => info.has_audio = Some(true),
                _ => {}
            },
            "format" => {
                info.duration = get("duration")
                    .and_then(|d| d.parse::<f64>().ok())
                    .filter(|d| d.is_finite() && *d > 0.0);
            }
            _ => {}
        }
    }
    info
}

/// Runs ffprobe on a file or URL. Any failure means "unknown".
fn probe_input(ffmpeg: &Path, url: &OsString) -> InputInfo {
    let Some(ffprobe) = ffmpeg::locate_sibling(ffmpeg, "ffprobe") else {
        debug!("ffprobe not found; not probing the input");
        return InputInfo::default();
    };
    let child = StdCommand::new(&ffprobe)
        .args(["-v", "error", "-show_entries"])
        .arg("stream=codec_type,width,height,avg_frame_rate:format=duration")
        .args(["-of", "compact"])
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            debug!("cannot run {}: {e}", ffprobe.display());
            return InputInfo::default();
        }
    };
    let read = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut s);
            }
            String::from_utf8_lossy(&s).into_owned()
        })
    };
    let out = read(child.stdout.take().map(|p| Box::new(p) as _));
    let err = read(child.stderr.take().map(|p| Box::new(p) as _));
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                warn!("ffprobe did not finish; going on without it");
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    match status {
        Some(s) if s.success() => {
            let info = parse_ffprobe(&out);
            debug!("ffprobe: {info:?}");
            info
        }
        Some(s) => {
            let last = err.lines().last().unwrap_or("").trim();
            warn!("ffprobe failed ({s}): {last}");
            InputInfo::default()
        }
        None => InputInfo::default(),
    }
}

// ---------------------------------------------------------------------------
// Planning

/// What we would like to send, before the receivers had their say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Desired {
    size: Size,
    fps: Ratio,
    bitrate_kbps: u32,
    latency_ms: u32,
}

/// The largest even size with `w:h`'s aspect ratio that fits `max`.
fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> Size {
    let even = |x: u64| (x.max(2) & !1) as u32;
    let (w64, h64) = (w.max(1) as u64, h.max(1) as u64);
    if w <= max_w && h <= max_h {
        return Size {
            width: even(w64),
            height: even(h64),
        };
    }
    // Scale by the tighter of the two limits.
    if w64 * max_h as u64 >= h64 * max_w as u64 {
        Size {
            width: even(max_w as u64),
            height: even(h64 * max_w as u64 / w64),
        }
    } else {
        Size {
            width: even(w64 * max_h as u64 / h64),
            height: even(max_h as u64),
        }
    }
}

fn default_bitrate(size: Size, fps: Ratio) -> u32 {
    let kbps =
        size.width as f64 * size.height as f64 * fps.as_f64() * DEFAULT_BITS_PER_PIXEL / 1000.0;
    (kbps as u32).clamp(MIN_DEFAULT_KBPS, MAX_DEFAULT_KBPS)
}

fn ratio_min(a: Ratio, b: Ratio) -> Ratio {
    if b.as_f64() < a.as_f64() {
        b
    } else {
        a
    }
}

fn desired(opts: &PlayOptions, input: &InputInfo) -> Desired {
    let raw = opts.raw_video.as_ref();
    let size = opts.size.unwrap_or_else(|| {
        let (w, h) = match (raw, input.width, input.height) {
            (Some(r), _, _) => (r.size.width, r.size.height),
            (None, Some(w), Some(h)) => (w, h),
            _ => (FALLBACK_SIZE.width, FALLBACK_SIZE.height),
        };
        fit(w, h, DEFAULT_MAX_SIZE.width, DEFAULT_MAX_SIZE.height)
    });
    let fps = opts.fps.unwrap_or_else(|| {
        let f = raw.map(|r| r.fps).or(input.fps).unwrap_or(FALLBACK_FPS);
        ratio_min(f, Ratio::integer(MAX_DEFAULT_FPS))
    });
    Desired {
        size,
        fps,
        bitrate_kbps: opts
            .bitrate_kbps
            .unwrap_or_else(|| default_bitrate(size, fps)),
        latency_ms: opts.latency_ms.unwrap_or(DEFAULT_LATENCY_MS),
    }
}

fn cast_options(d: &Desired, audio: bool) -> CastOptions {
    CastOptions {
        latency_ms: d.latency_ms,
        max_width: d.size.width,
        max_height: d.size.height,
        max_fps: d.fps,
        max_bitrate_kbps: d.bitrate_kbps,
        audio,
        ..CastOptions::default()
    }
}

/// The first audio format of the family that every sink of it accepts.
fn common_audio(caps: &[&Capabilities]) -> Option<AudioFormat> {
    let first = caps.first()?;
    first
        .audio_formats
        .iter()
        .find(|f| caps.iter().all(|c| c.accepts_audio(f)))
        .copied()
}

fn audio_plan(family: AudioFamily, f: AudioFormat) -> Option<AudioPlan> {
    let (frame_samples, bitrate) = match f.codec {
        AudioCodec::Opus => (OPUS_FRAME_SAMPLES, Some(AUDIO_KBPS)),
        AudioCodec::AacLc => (AAC_FRAME_SAMPLES, Some(AUDIO_KBPS)),
        // AirPlay codecs arrive with M7.
        _ => return None,
    };
    Some(AudioPlan {
        family,
        codec: f.codec,
        sample_rate: f.sample_rate,
        channels: f.channels,
        frame_samples,
        bitrate_kbps: bitrate,
    })
}

/// The encode plan: `d` clamped by every sink's capabilities.
fn plan(d: &Desired, encoder: Encoder, caps: &[&Capabilities], audio: bool) -> Result<EncodePlan> {
    if caps.is_empty() {
        return Err(Error::usage("no receivers"));
    }
    if let Some(c) = caps
        .iter()
        .find(|c| !c.video_codecs.contains(&VideoCodec::H264))
    {
        return Err(Error::unsupported(format!(
            "a receiver does not take H.264 (it offers {:?})",
            c.video_codecs
        )));
    }
    let max_w = caps.iter().map(|c| c.max_width).min().unwrap_or(0);
    let max_h = caps.iter().map(|c| c.max_height).min().unwrap_or(0);
    if max_w < 2 || max_h < 2 {
        return Err(Error::unsupported("a receiver allows no video size"));
    }
    let size = fit(d.size.width, d.size.height, max_w, max_h);
    let fps = caps
        .iter()
        .map(|c| c.max_fps)
        .filter(|f| f.num > 0 && f.den > 0)
        .fold(d.fps, ratio_min);
    let mut bitrate = d.bitrate_kbps;
    for c in caps {
        if let Some(b) = c.max_bitrate_kbps.filter(|&b| b > 0) {
            bitrate = bitrate.min(b);
        }
    }
    let latency_ms = caps
        .iter()
        .filter_map(|c| c.min_latency_ms)
        .fold(d.latency_ms, u32::max);
    let mut audio_plans = Vec::new();
    if audio {
        for family in [AudioFamily::Cast, AudioFamily::AirPlay] {
            let of: Vec<&Capabilities> = caps
                .iter()
                .copied()
                .filter(|c| c.audio_family == family)
                .collect();
            if of.is_empty() {
                continue;
            }
            match common_audio(&of).and_then(|f| audio_plan(family, f)) {
                Some(p) => audio_plans.push(p),
                None => warn!("no audio format all {family:?} receivers take; video only"),
            }
        }
    }
    Ok(EncodePlan {
        video: VideoPlan {
            codec: VideoCodec::H264,
            encoder,
            width: size.width,
            height: size.height,
            fps,
            bitrate_kbps: bitrate.max(100),
            gop: fps.ceil().max(1),
        },
        audio: audio_plans,
        latency_ms,
    })
}

// ---------------------------------------------------------------------------
// The session

fn lock(p: &Mutex<ffmpeg::Process>) -> std::sync::MutexGuard<'_, ffmpeg::Process> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

fn connect_all(targets: &[Target], opts: &CastOptions) -> Result<Vec<Box<dyn Sink>>> {
    let results: Vec<Result<Box<dyn Sink>>> = std::thread::scope(|s| {
        let handles: Vec<_> = targets
            .iter()
            .map(|t| {
                s.spawn(move || -> Result<Box<dyn Sink>> {
                    match t {
                        Target::Cast { addr, name } => {
                            let sink = cast::connect(*addr, name, opts)?;
                            Ok(Box::new(sink))
                        }
                        Target::AirPlay { name } => Err(Error::unsupported(format!(
                            "{name}: AirPlay mirroring not implemented yet (M6)"
                        ))),
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(Error::process("connect thread panicked")))
            })
            .collect()
    });
    let mut sinks = Vec::new();
    let mut first_err = None;
    for (t, r) in targets.iter().zip(results) {
        match r {
            Ok(s) => sinks.push(s),
            Err(e) => {
                let name = match t {
                    Target::Cast { name, .. } | Target::AirPlay { name } => name,
                };
                error!("{name}: {e}");
                first_err.get_or_insert(e);
            }
        }
    }
    match first_err {
        Some(e) => {
            stop_all(sinks);
            Err(e)
        }
        None => Ok(sinks),
    }
}

fn stop_all(sinks: Vec<Box<dyn Sink>>) {
    for mut s in sinks {
        if let Err(e) = s.stop() {
            warn!("{}: stop: {e}", s.name());
        }
    }
}

/// Kills ffmpeg when a stop is requested (after a grace period: it got
/// SIGINT too) or when the session is done, so the demuxer's read ends.
struct Watcher {
    done: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watcher {
    fn start(proc: Arc<Mutex<ffmpeg::Process>>) -> Result<Watcher> {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        let thread = std::thread::Builder::new()
            .name("ffmpeg-watch".into())
            .spawn(move || {
                let mut stop_at = None;
                loop {
                    if flag.load(Ordering::Acquire) {
                        return;
                    }
                    if interrupt::requested() {
                        let at = *stop_at.get_or_insert_with(|| Instant::now() + KILL_GRACE);
                        if Instant::now() >= at {
                            lock(&proc).kill();
                            return;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            })
            .map_err(|e| Error::io("spawn ffmpeg watcher", e))?;
        Ok(Watcher {
            done,
            thread: Some(thread),
        })
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndReason {
    Eof,
    Interrupted,
    ReceiversGone,
}

fn play(opts: PlayOptions) -> Result<()> {
    if !interrupt::install() {
        warn!("cannot catch Ctrl-C; receivers will not get a clean stop");
    }
    if opts.input != InputSpec::Stdin {
        interrupt::watch_stdin();
    }
    let bin = ffmpeg::locate(opts.ffmpeg.as_deref())?;
    let probe = ffmpeg::Probe::run(&bin)?;

    // 1. The input.
    let input = match &opts.input {
        InputSpec::Path(p) => probe_input(&bin, p),
        InputSpec::TestPattern => InputInfo {
            has_audio: Some(true),
            has_video: Some(true),
            ..InputInfo::default()
        },
        InputSpec::Stdin => InputInfo {
            has_audio: Some(opts.raw_audio.is_some()),
            has_video: Some(true),
            ..InputInfo::default()
        },
        InputSpec::FfmpegArgs(_) => InputInfo::default(),
    };
    if input.has_video == Some(false) {
        return Err(Error::unsupported("the input has no video stream"));
    }
    let audio = !opts.no_audio && input.has_audio != Some(false);
    if !opts.no_audio && !audio {
        info!("the input has no audio; streaming video only");
    }
    let d = desired(&opts, &input);

    // 2. Receivers, before ffmpeg.
    let targets = opts
        .to
        .iter()
        .map(|q| resolve_target(q))
        .collect::<Result<Vec<_>>>()?;
    if let Some(Target::AirPlay { name }) =
        targets.iter().find(|t| matches!(t, Target::AirPlay { .. }))
    {
        return Err(Error::unsupported(format!(
            "{name}: AirPlay mirroring not implemented yet (M6)"
        )));
    }
    let sinks = connect_all(&targets, &cast_options(&d, audio))?;
    if interrupt::requested() {
        stop_all(sinks);
        return Ok(());
    }

    // 3. The plan.
    let caps: Vec<&Capabilities> = sinks.iter().map(|s| s.capabilities()).collect();
    let plan = match plan(&d, opts.encoder, &caps, audio) {
        Ok(p) => p,
        Err(e) => {
            stop_all(sinks);
            return Err(e);
        }
    };
    info!(
        "plan: {}x{}@{} {} kbit/s, gop {}, audio {:?}, latency {} ms",
        plan.video.width,
        plan.video.height,
        plan.video.fps,
        plan.video.bitrate_kbps,
        plan.video.gop,
        plan.audio
            .iter()
            .map(|a| format!("{:?} {:?} {} Hz", a.family, a.codec, a.sample_rate))
            .collect::<Vec<_>>(),
        plan.latency_ms
    );
    match stream(&opts, &bin, &probe, &input, &plan, sinks) {
        Ok(EndReason::Interrupted) => {
            info!("stopped");
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Steps 4 and 5: ffmpeg → demuxer → fanout, then teardown. Consumes the
/// sinks; they are stopped on every path.
fn stream(
    opts: &PlayOptions,
    bin: &Path,
    probe: &ffmpeg::Probe,
    input: &InputInfo,
    plan: &EncodePlan,
    sinks: Vec<Box<dyn Sink>>,
) -> Result<EndReason> {
    let mut raw = None;
    let source = match &opts.input {
        InputSpec::Path(p) => Source::Url {
            url: p.clone(),
            realtime: opts
                .realtime
                .unwrap_or(input.duration.is_some() || Path::new(p).is_file()),
            looped: opts.looped,
        },
        InputSpec::FfmpegArgs(a) => Source::Args(a.clone()),
        InputSpec::TestPattern => Source::TestPattern { clock: true },
        InputSpec::Stdin => {
            let r = match crate::media::rawin::RawInput::bind(
                opts.raw_video.as_ref(),
                opts.raw_audio.as_ref(),
            ) {
                Ok(r) => r,
                Err(e) => {
                    stop_all(sinks);
                    return Err(e);
                }
            };
            let s = r.source();
            raw = Some(r);
            s
        }
    };
    let (loglevel, level) = if crate::log::enabled(Level::Debug) {
        ("info", Level::Debug)
    } else {
        ("warning", Level::Warn)
    };
    let started = (|| {
        let inv = ffmpeg::build(&source, Output::Encode(plan), probe, loglevel)?;
        let mut proc = ffmpeg::Process::spawn_logging(bin, &inv.args, level)?;
        let stdout = proc
            .take_stdout()
            .ok_or_else(|| Error::process("ffmpeg stdout not piped"))?;
        Ok((inv, proc, stdout))
    })();
    let (inv, proc, stdout) = match started {
        Ok(v) => v,
        Err(e) => {
            stop_all(sinks);
            return Err(e);
        }
    };
    let proc = Arc::new(Mutex::new(proc));
    let watcher = match Watcher::start(proc.clone()) {
        Ok(w) => w,
        Err(e) => {
            stop_all(sinks);
            return Err(e);
        }
    };
    let forwarder = match raw.take().map(|r| r.start(std::io::stdin())).transpose() {
        Ok(f) => f,
        Err(e) => {
            stop_all(sinks);
            return Err(e);
        }
    };
    let ffmpeg_error =
        |proc: &Mutex<ffmpeg::Process>, e: Error| -> Error { lock(proc).failure().unwrap_or(e) };

    let mut reader = match MediaReader::new(stdout, &inv.tracks) {
        Ok(r) => r,
        Err(e) => {
            stop_all(sinks);
            if interrupt::requested() {
                return Ok(EndReason::Interrupted);
            }
            return Err(ffmpeg_error(&proc, e));
        }
    };
    let info = reader.info().clone();
    let v = &info.video;
    info!(
        "ffmpeg: {:?} {}x{}@{}{}",
        v.codec,
        v.width,
        v.height,
        v.fps,
        info.audio
            .iter()
            .map(|(f, a)| format!(", {f:?} audio {:?} {} Hz", a.codec, a.sample_rate))
            .collect::<String>()
    );
    // On failure the sinks are dropped, which stops them.
    let fanout = Fanout::new(sinks, &info, FanoutConfig::default())?;

    let t0 = Instant::now();
    let mut frames = [0u64; 2];
    let mut demux_error = None;
    let end = loop {
        if interrupt::requested() {
            break EndReason::Interrupted;
        }
        match reader.next_frame() {
            Ok(Some(f)) => {
                frames[(f.frame.track != TrackKind::Video) as usize] += 1;
                fanout.push(&f.frame);
                if fanout.active() == 0 {
                    info!("every receiver ended the session");
                    break EndReason::ReceiversGone;
                }
            }
            Ok(None) if interrupt::requested() => break EndReason::Interrupted,
            Ok(None) => break EndReason::Eof,
            Err(e) if interrupt::requested() => {
                debug!("demuxer after stop request: {e}");
                break EndReason::Interrupted;
            }
            Err(e) => {
                demux_error = Some(e);
                break EndReason::Eof;
            }
        }
    };
    let elapsed = t0.elapsed();
    if end == EndReason::Eof && demux_error.is_none() {
        // Let the last frames play out before STOP.
        std::thread::sleep(Duration::from_millis(plan.latency_ms as u64 + 100));
    }
    let stats = fanout.stats();
    let ends = fanout.close(end == EndReason::Eof);

    // ffmpeg: on EOF it has exited; otherwise kill it.
    drop(watcher);
    let ffmpeg_result = {
        let mut p = lock(&proc);
        if end != EndReason::Eof {
            p.kill();
        }
        match demux_error {
            Some(e) => Err(p.failure().unwrap_or(e)),
            None if end == EndReason::Eof => p.wait().map(|_| ()),
            None => {
                let _ = p.wait();
                Ok(())
            }
        }
    };
    if let Some(f) = forwarder {
        match f.finish() {
            Ok(s) => debug!("raw input: {s:?}"),
            Err(e) => debug!("raw input: {e}"),
        }
    }

    info!(
        "sent {} video and {} audio frames in {:.1} s",
        frames[0],
        frames[1],
        elapsed.as_secs_f64()
    );
    let mut failed = 0;
    for ((name, end), (_, st)) in ends.iter().zip(&stats) {
        let how = match end {
            SinkEnd::Stopped => "stopped".to_owned(),
            SinkEnd::Finished => "ended by the receiver".to_owned(),
            SinkEnd::Failed(e) => {
                failed += 1;
                format!("failed: {e}")
            }
        };
        info!(
            "{name}: {how}; {} frames pushed, {} dropped by the sink, {} on a full queue, \
             {} waiting for a keyframe",
            st.pushed, st.sink_dropped, st.dropped_full, st.dropped_waiting
        );
    }
    ffmpeg_result?;
    if failed > 0 && failed == ends.len() {
        let msg = ends.into_iter().find_map(|(name, e)| match e {
            SinkEnd::Failed(e) => Some(format!("{name}: {e}")),
            _ => None,
        });
        return Err(Error::protocol(
            msg.unwrap_or_else(|| "every receiver failed".into()),
        ));
    }
    Ok(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(w: u32, h: u32, fps: u32, kbps: u32) -> Capabilities {
        Capabilities {
            max_width: w,
            max_height: h,
            max_fps: Ratio::integer(fps),
            video_codecs: vec![VideoCodec::H264],
            audio_formats: vec![AudioFormat {
                codec: AudioCodec::Opus,
                sample_rate: 48_000,
                channels: 2,
            }],
            audio_family: AudioFamily::Cast,
            max_bitrate_kbps: Some(kbps),
            min_latency_ms: None,
            preferred_latency_ms: Some(200),
        }
    }

    #[test]
    fn targets() {
        let cast = |a: &str| Target::Cast {
            addr: a.parse().unwrap(),
            name: a.into(),
        };
        let t = |q: &str| {
            let (f, rest) = parse_target(q);
            address_target(f, rest)
        };
        assert_eq!(t("127.0.0.1"), Some(cast("127.0.0.1:8009")));
        assert_eq!(t("127.0.0.1:8010"), Some(cast("127.0.0.1:8010")));
        assert_eq!(t("CAST:10.0.0.5:7000"), Some(cast("10.0.0.5:7000")));
        assert_eq!(t("[::1]"), Some(cast("[::1]:8009")));
        assert_eq!(t("[::1]:8010"), Some(cast("[::1]:8010")));
        assert_eq!(
            t("10.0.0.5:7000"),
            Some(Target::AirPlay {
                name: "10.0.0.5:7000".into()
            })
        );
        assert_eq!(
            t("airplay:10.0.0.5"),
            Some(Target::AirPlay {
                name: "10.0.0.5:7000".into()
            })
        );
        assert_eq!(t("Living Room"), None);
        assert_eq!(t("cast:Living Room"), None);
        assert_eq!(parse_target("cast:"), (None, "cast:"));
    }

    #[test]
    fn discovered_targets() {
        let devs = crate::net::device::tests::lan_devices();
        for d in &devs {
            let t = device_target(d, None);
            match (&d.cast, &d.airplay) {
                (Some(_), _) if d.cast_video() => {
                    assert!(matches!(t, Ok(Target::Cast { .. })), "{}", d.name)
                }
                (_, Some(_)) => assert!(matches!(t, Ok(Target::AirPlay { .. })), "{}", d.name),
                _ => assert!(t.is_err(), "{}", d.name),
            }
        }
        let tv = devs.iter().find(|d| d.name == "Livingroom (2)").unwrap();
        assert!(device_target(tv, Some(Protocol::Cast)).is_err());
    }

    #[test]
    fn ffprobe_output() {
        let info = parse_ffprobe(
            "stream|codec_type=video|width=1920|height=1080|avg_frame_rate=30000/1001\n\
             stream|codec_type=audio|width=N/A|height=N/A|avg_frame_rate=0/0\n\
             format|duration=12.345000\n",
        );
        assert_eq!(info.width, Some(1920));
        assert_eq!(info.height, Some(1080));
        assert_eq!(info.fps, Some(Ratio::new(30000, 1001)));
        assert_eq!(info.has_audio, Some(true));
        assert_eq!(info.has_video, Some(true));
        assert_eq!(info.duration, Some(12.345));
        let live = parse_ffprobe("stream|codec_type=video|width=640|height=480|avg_frame_rate=0/0\nformat|duration=N/A\n");
        assert_eq!(live.has_audio, Some(false));
        assert_eq!(live.fps, None);
        assert_eq!(live.duration, None);
        // Garbage never panics and yields "nothing found".
        let junk = parse_ffprobe(
            "stream|codec_type=video|width=-1|height=x|avg_frame_rate=1/0\n|||\n=\nformat",
        );
        assert_eq!(junk.width, None);
        assert_eq!(junk.fps, None);
        assert_eq!(parse_ffprobe("").has_video, Some(false));
    }

    #[test]
    fn fitting() {
        let s = |width, height| Size { width, height };
        assert_eq!(fit(1280, 720, 1920, 1080), s(1280, 720));
        assert_eq!(fit(3840, 2160, 1920, 1080), s(1920, 1080));
        assert_eq!(fit(1920, 800, 1280, 720), s(1280, 532));
        assert_eq!(fit(1080, 1920, 1920, 1080), s(606, 1080));
        assert_eq!(fit(641, 481, 1920, 1080), s(640, 480));
        assert_eq!(fit(0, 0, 1920, 1080), s(2, 2));
    }

    fn opts() -> PlayOptions {
        PlayOptions {
            to: vec!["tv".into()],
            latency_ms: None,
            size: None,
            fps: None,
            bitrate_kbps: None,
            encoder: Encoder::Auto,
            no_audio: false,
            looped: false,
            realtime: None,
            raw_video: None,
            raw_audio: None,
            ffmpeg: None,
            input: InputSpec::TestPattern,
        }
    }

    #[test]
    fn desired_from_input_and_options() {
        let d = desired(&opts(), &InputInfo::default());
        assert_eq!((d.size.width, d.size.height), (1280, 720));
        assert_eq!(d.fps, Ratio::integer(30));
        assert_eq!(d.bitrate_kbps, 2764);
        assert_eq!(d.latency_ms, DEFAULT_LATENCY_MS);
        let input = InputInfo {
            width: Some(3840),
            height: Some(2160),
            fps: Some(Ratio::integer(120)),
            ..InputInfo::default()
        };
        let d = desired(&opts(), &input);
        assert_eq!((d.size.width, d.size.height), (1920, 1080));
        assert_eq!(d.fps, Ratio::integer(60));
        assert_eq!(d.bitrate_kbps, 12441);
        let o = PlayOptions {
            size: Some(Size {
                width: 640,
                height: 360,
            }),
            fps: Some(Ratio::integer(25)),
            bitrate_kbps: Some(1500),
            latency_ms: Some(120),
            ..opts()
        };
        let d = desired(&o, &input);
        assert_eq!(
            d,
            Desired {
                size: Size {
                    width: 640,
                    height: 360
                },
                fps: Ratio::integer(25),
                bitrate_kbps: 1500,
                latency_ms: 120
            }
        );
        let c = cast_options(&d, false);
        assert_eq!(
            (c.max_width, c.max_height, c.max_bitrate_kbps),
            (640, 360, 1500)
        );
        assert_eq!((c.latency_ms, c.audio), (120, false));
    }

    #[test]
    fn plan_intersects_capabilities() {
        let d = Desired {
            size: Size {
                width: 1920,
                height: 1080,
            },
            fps: Ratio::integer(60),
            bitrate_kbps: 8000,
            latency_ms: 150,
        };
        let a = caps(1920, 1080, 60, 10_000);
        let mut b = caps(1280, 720, 30, 4000);
        b.min_latency_ms = Some(250);
        let p = plan(&d, Encoder::Auto, &[&a, &b], true).unwrap();
        assert_eq!((p.video.width, p.video.height), (1280, 720));
        assert_eq!(p.video.fps, Ratio::integer(30));
        assert_eq!(p.video.gop, 30);
        assert_eq!(p.video.bitrate_kbps, 4000);
        assert_eq!(p.latency_ms, 250);
        assert_eq!(p.audio.len(), 1);
        assert_eq!(p.audio[0].codec, AudioCodec::Opus);
        assert_eq!(p.audio[0].frame_samples, 480);
        // No audio requested, or no common format: video only.
        assert!(plan(&d, Encoder::Auto, &[&a], false)
            .unwrap()
            .audio
            .is_empty());
        b.audio_formats[0].codec = AudioCodec::AacLc;
        assert!(plan(&d, Encoder::Auto, &[&a, &b], true)
            .unwrap()
            .audio
            .is_empty());
        let p = plan(&d, Encoder::Auto, &[&b], true).unwrap();
        assert_eq!(p.audio[0].frame_samples, 1024);
        // A receiver without H.264 is an error.
        b.video_codecs = vec![VideoCodec::Hevc];
        assert!(plan(&d, Encoder::Auto, &[&a, &b], true).is_err());
        assert!(plan(&d, Encoder::Auto, &[], true).is_err());
    }

    #[test]
    fn plan_builds_ffmpeg_args() {
        let d = desired(&opts(), &InputInfo::default());
        let c = caps(1920, 1080, 60, 10_000);
        let p = plan(&d, Encoder::Auto, &[&c], true).unwrap();
        let inv = ffmpeg::build(
            &Source::TestPattern { clock: false },
            Output::Encode(&p),
            &crate::media::ffmpeg::tests::probe(),
            "warning",
        )
        .unwrap();
        assert_eq!(inv.tracks.len(), 2);
        assert_eq!(inv.tracks[1].kind, TrackKind::Audio(AudioFamily::Cast));
    }

    /// Generates a short file with ffmpeg; `None` if ffmpeg is unusable.
    fn media_file(name: &str, audio: bool) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let (bin, _) = crate::media::ffmpeg::tests::real_ffmpeg()?;
        let dir = std::env::temp_dir().join(format!("streamlib-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(name);
        let mut cmd = StdCommand::new(&bin);
        cmd.args(["-hide_banner", "-loglevel", "error", "-y"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=30:duration=1",
            ]);
        if audio {
            cmd.args(["-f", "lavfi", "-i", "sine=frequency=440:duration=1"]);
        }
        cmd.args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&path);
        let ok = cmd.status().ok()?.success();
        ok.then_some((bin, path))
    }

    fn file_opts(bin: std::path::PathBuf, path: std::path::PathBuf, to: String) -> PlayOptions {
        PlayOptions {
            to: vec![to],
            latency_ms: Some(100),
            ffmpeg: Some(bin),
            input: InputSpec::Path(path.into()),
            ..opts()
        }
    }

    #[test]
    fn plays_a_file_to_a_cast_receiver() {
        use crate::cast::fake_receiver::{FakeReceiver, Options};
        let Some((bin, path)) = media_file("av.mp4", true) else {
            return;
        };
        let rx = FakeReceiver::spawn(Options::default());
        play(file_opts(bin, path, rx.addr.to_string())).unwrap();
        let state = rx.shutdown();
        let st = state.lock().unwrap();
        assert!(st.stopped, "STOP not received");
        let offer = st.offer.as_ref().unwrap().to_string();
        assert!(offer.contains("\"opus\""), "{offer}");
        let v = st.streams.iter().find(|s| s.codec == "h264").unwrap();
        let a = st.streams.iter().find(|s| s.codec == "opus").unwrap();
        // 1 s at 30 fps; 10 ms Opus frames.
        assert!((28..=31).contains(&v.frames.len()), "{}", v.frames.len());
        assert!(v.frames[&0].keyframe);
        assert!((90..=102).contains(&a.frames.len()), "{}", a.frames.len());
    }

    #[test]
    fn video_only_input_offers_no_audio() {
        use crate::cast::fake_receiver::{FakeReceiver, Options};
        let Some((bin, path)) = media_file("v.mp4", false) else {
            return;
        };
        let rx = FakeReceiver::spawn(Options::default());
        play(file_opts(bin, path, format!("cast:{}", rx.addr))).unwrap();
        let state = rx.shutdown();
        let st = state.lock().unwrap();
        assert!(st.stopped);
        assert!(st.streams.iter().all(|s| s.codec == "h264"));
        let v = &st.streams[0];
        assert!((28..=31).contains(&v.frames.len()), "{}", v.frames.len());
    }

    #[test]
    fn airplay_targets_are_unsupported() {
        let o = PlayOptions {
            to: vec!["airplay:127.0.0.1".into()],
            ffmpeg: crate::media::ffmpeg::tests::real_ffmpeg().map(|(b, _)| b),
            ..opts()
        };
        if o.ffmpeg.is_none() {
            return;
        }
        let e = play(o).unwrap_err();
        assert_eq!(e.kind(), crate::error::ErrorKind::Unsupported);
        assert!(e.to_string().contains("not implemented yet (M6)"), "{e}");
    }
}
