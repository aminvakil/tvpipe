//! Orchestration: connect → choose the input → fanout → teardown (PLAN §3).
//!
//! Owner: lead.
//!
//! `play` / `test-pattern`:
//!
//! 1. Probe the input: an MP4/MOV/Matroska file with our own demuxer
//!    (`media::passthrough::probe`), anything else with `ffprobe`: size,
//!    frame rate, audio or not, finite (paced) or live.
//! 2. Resolve every `--to` (discovery for names; `ADDR[:PORT]` skips it)
//!    and connect all sinks before any input starts. Cast sinks negotiate
//!    OFFER/ANSWER here, so their capabilities are final; the offer keeps
//!    a passable file's bitrate and AAC, and offers AAC for an encoding
//!    game.
//! 3. Choose the input (`media::input`): a file that fits every receiver
//!    is sent unchanged (`PassthroughInput`), `--raw-video ...:h264` reads
//!    the game's H.264 (`SlrwEncodedInput`), raw SLRW and the test pattern
//!    use the built-in encoder (`media::builtin`) with `--encoder builtin`
//!    or when ffmpeg is missing, everything else is encoded by ffmpeg
//!    (`FfmpegInput`) after planning the encode from the user's options
//!    and the sinks' capabilities. ffmpeg is only located when needed.
//! 4. Push every frame through the fanout; while a sink wants a keyframe,
//!    ask the input for one (only an encoding game can).
//! 5. Tear down on EOF (after one playout delay, so the tail plays), on
//!    Ctrl-C / SIGTERM / `q` (see `util::interrupt`, without draining), or
//!    when every receiver has ended the session. Every sink's `stop()` runs
//!    (Cast: STOP + CLOSE) and the input finishes (ffmpeg is reaped).

use std::ffi::OsString;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};
use std::time::{Duration, Instant};

use crate::browser::layout;
use crate::browser::protocol::{PageConfig, SharePlan};
use crate::browser::share::{ShareInputOptions, ShareServer};
use crate::cast::{self, CastOptions};
use crate::cli::{Command, InputSpec, PlayOptions, RawAudioSpec, ShareOptions, Size};
use crate::error::{Error, Result};
use crate::media::builtin::{self, BuiltinInput, BuiltinOptions, VideoOut};
use crate::media::fanout::{Fanout, FanoutConfig, SinkEnd};
use crate::media::ffmpeg::{self, Source};
use crate::media::input::{
    self, EncodedOptions, FfmpegInput, Input, PassthroughInput, SlrwEncodedInput,
};
use crate::media::{passthrough, rawin};
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
        Command::Share(opts) => share(opts),
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
/// retransmission or two on Wi-Fi. openscreen's sender default is 200 ms,
/// but a Google TV on Wi-Fi has feedback gaps of ~220 ms, which overflow
/// the in-flight window a few times per 5 minutes at 200 ms (each costing
/// up to a GOP); decision 0010. `--latency` goes lower on good networks.
pub const DEFAULT_LATENCY_MS: u32 = 300;
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
/// Opus frames: 10 ms (lowest latency libopus offers without overhead
/// blowing up).
const OPUS_FRAME_SAMPLES: u32 = 480;
const AAC_FRAME_SAMPLES: u32 = 1024;
const AUDIO_KBPS: u32 = 128;
/// How long the session waits for every sink's `start()` before it pulls
/// the input anyway.
const SINK_START_WAIT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Targets

/// A resolved `--to`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Cast {
        addr: SocketAddr,
        name: String,
    },
    AirPlay {
        name: String,
        addr: SocketAddr,
        /// The discovered device (for the credentials lookup).
        device: Option<Box<crate::net::device::Device>>,
    },
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
        Protocol::AirPlay => Target::AirPlay {
            name,
            addr,
            device: None,
        },
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
            let ep = dev
                .airplay
                .as_ref()
                .or(dev.raop.as_ref())
                .map(|a| &a.endpoint)
                .ok_or_else(|| Error::not_found(format!("{name} has no AirPlay endpoint")))?;
            Ok(Target::AirPlay {
                name,
                addr: SocketAddr::new(ep.addr(), ep.port),
                device: Some(Box::new(dev.clone())),
            })
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
    if family == AudioFamily::AirPlay && f.codec == AudioCodec::Alac {
        // ffmpeg's ALAC encoder only makes 4096-sample frames; the AirPlay
        // sink wraps 352-sample PCM frames as verbatim ALAC instead.
        return Some(AudioPlan {
            family,
            codec: AudioCodec::PcmS16le,
            sample_rate: f.sample_rate,
            channels: f.channels,
            frame_samples: crate::airplay::audio::ALAC_FRAME_SAMPLES,
            bitrate_kbps: None,
        });
    }
    let (frame_samples, bitrate) = match f.codec {
        AudioCodec::Opus => (OPUS_FRAME_SAMPLES, Some(AUDIO_KBPS)),
        AudioCodec::AacLc => (AAC_FRAME_SAMPLES, Some(AUDIO_KBPS)),
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
    let latency_ms = latency(d, caps);
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
            // AirPlay receivers want an IDR at least every ~0.5 s.
            gop: if caps.iter().any(|c| c.audio_family == AudioFamily::AirPlay) {
                fps.ceil().div_ceil(2).max(1)
            } else {
                fps.ceil().max(1)
            },
        },
        audio: audio_plans,
        latency_ms,
    })
}

// ---------------------------------------------------------------------------
// The session

/// Connects one target (the tests substitute fakes).
type Connector<'a> = &'a (dyn Fn(&Target, &CastOptions, &crate::airplay::session::MirrorOptions) -> Result<Box<dyn Sink>>
         + Sync);

fn connect_one(
    t: &Target,
    opts: &CastOptions,
    mirror: &crate::airplay::session::MirrorOptions,
) -> Result<Box<dyn Sink>> {
    match t {
        Target::Cast { addr, name } => Ok(Box::new(cast::connect(*addr, name, opts)?)),
        Target::AirPlay { addr, device, .. } => {
            let target = crate::airplay::pair::Target {
                addr: *addr,
                device: device.as_deref().cloned(),
            };
            Ok(Box::new(crate::airplay::sink::connect_mirror(
                &target, mirror,
            )?))
        }
    }
}

fn target_name(t: &Target) -> &str {
    match t {
        Target::Cast { name, .. } | Target::AirPlay { name, .. } => name,
    }
}

/// Keeps what worked: with several receivers, one that cannot be resolved
/// or connected is logged and left out, and the session goes on with the
/// others; it fails only when none is left (decision 0010).
fn keep_working<T>(what: &str, results: Vec<(String, Result<T>)>) -> Result<Vec<T>> {
    let total = results.len();
    let mut ok = Vec::new();
    let mut first_err = None;
    for (name, r) in results {
        match r {
            Ok(v) => ok.push(v),
            Err(e) => {
                error!("{name}: {e}");
                first_err.get_or_insert(e);
            }
        }
    }
    match first_err {
        Some(e) if ok.is_empty() => Err(e),
        Some(_) => {
            warn!(
                "{} of {total} receivers {what}; going on without the others",
                ok.len()
            );
            Ok(ok)
        }
        None => Ok(ok),
    }
}

/// Resolves every `--to` in parallel (discovery browses for names).
fn resolve_all(queries: &[String]) -> Result<Vec<Target>> {
    let results = std::thread::scope(|s| {
        let handles: Vec<_> = queries
            .iter()
            .map(|q| s.spawn(move || resolve_target(q)))
            .collect();
        queries
            .iter()
            .zip(handles)
            .map(|(q, h)| {
                let r = h
                    .join()
                    .unwrap_or_else(|_| Err(Error::process("resolve thread panicked")));
                (q.clone(), r)
            })
            .collect()
    });
    keep_working("resolved", results)
}

/// Connects every target in parallel.
fn connect_all(
    targets: &[Target],
    connect: Connector<'_>,
    opts: &CastOptions,
    mirror: &crate::airplay::session::MirrorOptions,
) -> Result<Vec<Box<dyn Sink>>> {
    let results: Vec<(String, Result<Box<dyn Sink>>)> = std::thread::scope(|s| {
        let handles: Vec<_> = targets
            .iter()
            .map(|t| s.spawn(move || connect(t, opts, mirror)))
            .collect();
        targets
            .iter()
            .zip(handles)
            .map(|(t, h)| {
                let r = h
                    .join()
                    .unwrap_or_else(|_| Err(Error::process("connect thread panicked")));
                (target_name(t).to_owned(), r)
            })
            .collect()
    });
    keep_working("connected", results)
}

fn stop_all(sinks: Vec<Box<dyn Sink>>) {
    for mut s in sinks {
        if let Err(e) = s.stop() {
            warn!("{}: stop: {e}", s.name());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndReason {
    Eof,
    Interrupted,
    ReceiversGone,
}

/// ffmpeg, located and probed on first use: passthrough, encoded SLRW and
/// the built-in test pattern never need it.
struct Tools<'a> {
    explicit: Option<&'a Path>,
    found: Option<(std::path::PathBuf, ffmpeg::Probe)>,
}

impl<'a> Tools<'a> {
    fn new(explicit: Option<&'a Path>) -> Tools<'a> {
        Tools {
            explicit,
            found: None,
        }
    }

    fn get(&mut self) -> Result<(&Path, &ffmpeg::Probe)> {
        if self.found.is_none() {
            let bin = ffmpeg::locate(self.explicit)?;
            let probe = ffmpeg::Probe::run(&bin)?;
            self.found = Some((bin, probe));
        }
        match &self.found {
            Some((bin, probe)) => Ok((bin, probe)),
            None => Err(Error::not_found("ffmpeg")),
        }
    }
}

/// What a probed file says, in the form `desired` takes.
fn file_input_info(fi: &passthrough::FileInfo) -> InputInfo {
    InputInfo {
        width: Some(fi.video.width).filter(|&w| w > 0),
        height: Some(fi.video.height).filter(|&h| h > 0),
        fps: Some(fi.video.fps).filter(|f| f.num > 0 && f.den > 0),
        has_audio: Some(fi.audio.is_some() || fi.unsupported_audio.is_some()),
        has_video: Some(true),
        duration: fi.duration,
    }
}

/// Where the frames come from, decided once the receivers are known.
enum Route {
    Passthrough(std::path::PathBuf, passthrough::PassPlan),
    Encoded,
    /// Raw SLRW or the test pattern through `media::h264enc`.
    Builtin,
    Ffmpeg,
}

/// Whether raw SLRW input or the test pattern goes through the built-in
/// encoder instead of ffmpeg. The test pattern always does unless an
/// ffmpeg encoder is named (`--encoder x264` etc. selects ffmpeg's
/// `testsrc2`); raw input does when asked (`--encoder builtin`), or with
/// `--encoder auto` when ffmpeg is missing.
fn use_builtin(opts: &PlayOptions, encoded: bool, tools: &mut Tools<'_>) -> Result<bool> {
    let raw = opts.input == InputSpec::Stdin && !encoded;
    let pattern = opts.input == InputSpec::TestPattern;
    let builtin = match opts.encoder {
        _ if encoded => false,
        Encoder::Builtin if raw || pattern => true,
        Encoder::Builtin => {
            return Err(Error::usage(
                "--encoder builtin encodes raw SLRW input (-) and test-pattern; \
                 files, URLs and capture devices need ffmpeg",
            ))
        }
        Encoder::Auto if raw => match tools.get() {
            Ok(_) => false,
            Err(e) => {
                debug!("{e}; encoding the raw input with the built-in H.264 encoder");
                true
            }
        },
        Encoder::Auto if pattern => true,
        _ => false,
    };
    if builtin && raw {
        let v = opts
            .raw_video
            .as_ref()
            .ok_or_else(|| Error::usage("raw input (-) needs --raw-video WxH@FPS:PIXFMT"))?;
        if !builtin::supported_pix_fmt(&v.pix_fmt) {
            return Err(Error::unsupported(format!(
                "the built-in encoder cannot read {:?} frames (rgba, bgra, rgb0, bgr0, rgb24, \
                 bgr24, nv12, yuv420p) and ffmpeg is {}",
                v.pix_fmt,
                if opts.encoder == Encoder::Builtin {
                    "not used with --encoder builtin"
                } else {
                    "not available"
                }
            )));
        }
    }
    Ok(builtin)
}

/// The game's ends of our stdio (SLRW in, keyframe requests out).
struct GameIo {
    stdin: Box<dyn std::io::Read + Send>,
    stdout: Box<dyn std::io::Write + Send>,
}

fn play(opts: PlayOptions) -> Result<()> {
    play_io(
        opts,
        GameIo {
            stdin: Box::new(std::io::stdin()),
            stdout: Box::new(std::io::stdout()),
        },
    )
}

fn play_io(opts: PlayOptions, io: GameIo) -> Result<()> {
    if !interrupt::install() {
        warn!("cannot catch Ctrl-C; receivers will not get a clean stop");
    }
    if opts.input != InputSpec::Stdin {
        interrupt::watch_stdin();
    }
    play_with(opts, io, &connect_one, interrupt::requested)
}

/// `play` with the connector and the stop request (Ctrl-C, SIGTERM, `q`)
/// given: the tests use fakes and their own stop flag.
fn play_with(
    opts: PlayOptions,
    io: GameIo,
    connect: Connector<'_>,
    stop: fn() -> bool,
) -> Result<()> {
    // Also here for the tests, which do not go through `main`.
    crate::util::timer::raise_resolution();
    let mut tools = Tools::new(opts.ffmpeg.as_deref());

    // 1. The input. A missing local file fails before any TV is touched.
    if let InputSpec::Path(p) = &opts.input {
        let looks_like_url = p.to_string_lossy().contains("://");
        if !looks_like_url && !Path::new(p).exists() {
            return Err(Error::not_found(format!(
                "{}: no such file",
                Path::new(p).display()
            )));
        }
    }
    let encoded = opts
        .raw_video
        .as_ref()
        .is_some_and(|v| v.pix_fmt == rawin::ENCODED_PIX_FMT);
    if encoded && opts.input != InputSpec::Stdin {
        return Err(Error::usage(
            "--raw-video ...:h264 reads SLRW from stdin (-)",
        ));
    }
    let builtin = use_builtin(&opts, encoded, &mut tools)?;
    let candidate = input::passthrough_candidate(&opts);
    let probed = candidate.as_deref().map(passthrough::probe);
    let input = match &opts.input {
        InputSpec::Path(p) => match &probed {
            Some(Ok(fi)) => file_input_info(fi),
            _ => match tools.get() {
                Ok((bin, _)) => probe_input(bin, p),
                Err(e) => {
                    debug!("not probing the input: {e}");
                    InputInfo::default()
                }
            },
        },
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

    // 2. Receivers, before ffmpeg. Offers follow what we can send: a file
    // that may pass keeps its bitrate and its AAC; an encoding game's PCM
    // becomes AAC for Cast.
    let mut cast_opts = cast_options(&d, audio);
    let check = passthrough::CheckOptions {
        audio,
        ..Default::default()
    };
    if let Some(Ok(fi)) = &probed {
        if input::encode_forced_by(&opts).is_none() && passthrough::file_ok(fi, check) {
            if let Some(k) = fi.peak_video_kbps.or(fi.video_kbps) {
                cast_opts.max_bitrate_kbps =
                    cast_opts.max_bitrate_kbps.max(k.min(MAX_DEFAULT_KBPS));
            }
            if let Some(a) = fi
                .audio
                .as_ref()
                .filter(|a| audio && a.codec == AudioCodec::AacLc)
            {
                cast_opts.audio_codecs = vec![AudioCodec::AacLc];
                cast_opts.audio_sample_rate = a.sample_rate;
                cast_opts.audio_channels = a.channels;
            }
        }
    }
    let targets = resolve_all(&opts.to)?;
    // The built-in test pattern's beep: 44.1 kHz when AirPlay listens.
    let pattern_rate =
        builtin::pattern_sample_rate(targets.iter().any(|t| matches!(t, Target::AirPlay { .. })));
    let pattern_audio = builtin::pattern_audio(pattern_rate);
    let game_audio = match &opts.input {
        InputSpec::TestPattern if builtin => Some(&pattern_audio),
        InputSpec::Stdin if encoded || builtin => opts.raw_audio.as_ref(),
        _ => None,
    };
    if audio {
        // In-process audio for Cast is AAC (media::aacenc).
        if let Some(f) = game_audio.and_then(|a| input::encoded_cast_audio(Some(a))) {
            cast_opts.audio_codecs = vec![AudioCodec::AacLc];
            cast_opts.audio_sample_rate = f.sample_rate;
            cast_opts.audio_channels = f.channels;
        }
    }
    let budget = shared_budget(opts.latency_ms, &targets);
    cast_opts.latency_ms = budget.cast_ms;
    let mirror = crate::airplay::session::MirrorOptions {
        latency_ms: budget.airplay_ms,
        audio,
        ..Default::default()
    };
    let sinks = connect_all(&targets, connect, &cast_opts, &mirror)?;
    if stop() {
        stop_all(sinks);
        return Ok(());
    }

    // 3. The route, then the input.
    let caps: Vec<&Capabilities> = sinks.iter().map(|s| s.capabilities()).collect();
    let named: Vec<(&str, &Capabilities)> =
        sinks.iter().map(|s| (s.name(), s.capabilities())).collect();
    let route = match (&candidate, &probed) {
        _ if encoded => Route::Encoded,
        _ if builtin => Route::Builtin,
        (Some(path), Some(probed)) => match input::decide_file(probed, &opts, &named) {
            input::FileDecision::Passthrough { info: fi, plan } => {
                info!(
                    "passthrough: sending {} unchanged ({:?} {}x{}@{}{})",
                    path.display(),
                    fi.video.codec,
                    fi.video.width,
                    fi.video.height,
                    fi.video.fps,
                    match (&fi.audio, plan.audio_families.is_empty()) {
                        (Some(a), false) => format!(", {:?} audio", a.codec),
                        _ => String::new(),
                    }
                );
                Route::Passthrough(path.clone(), plan)
            }
            input::FileDecision::Ffmpeg { why } => {
                info!("{why}");
                Route::Ffmpeg
            }
        },
        _ => Route::Ffmpeg,
    };
    let opened: Result<(Box<dyn Input>, u32)> = (|| match route {
        Route::Passthrough(path, plan) => {
            let realtime = opts.realtime.unwrap_or(true);
            let i = PassthroughInput::open_with(&path, &plan, realtime, opts.looped, stop)?;
            Ok((Box::new(i) as Box<dyn Input>, latency(&d, &caps)))
        }
        Route::Encoded => {
            if let Some(c) = caps
                .iter()
                .find(|c| !c.video_codecs.contains(&VideoCodec::H264))
            {
                return Err(Error::unsupported(format!(
                    "a receiver does not take H.264 (it offers {:?})",
                    c.video_codecs
                )));
            }
            let raw_audio = opts.raw_audio.as_ref().filter(|_| audio);
            let (families, warnings) = input::encoded_audio_families(raw_audio, &named);
            for w in warnings {
                warn!("{w}");
            }
            let video = opts
                .raw_video
                .clone()
                .ok_or_else(|| Error::usage("--raw-video missing"))?;
            let i = SlrwEncodedInput::open_with(
                io.stdin,
                EncodedOptions {
                    video,
                    audio: raw_audio.cloned(),
                    audio_families: families,
                    back_channel: Some(io.stdout),
                },
                stop,
            )?;
            Ok((Box::new(i) as Box<dyn Input>, latency(&d, &caps)))
        }
        Route::Builtin => {
            // Audio is planned like the encoded-SLRW path's, below.
            let plan = plan(&d, Encoder::Builtin, &caps, false)?;
            info!(
                "plan: {}x{}@{} {} kbit/s, gop {}, latency {} ms (built-in encoder)",
                plan.video.width,
                plan.video.height,
                plan.video.fps,
                plan.video.bitrate_kbps,
                plan.video.gop,
                plan.latency_ms
            );
            let out = VideoOut {
                size: Size {
                    width: plan.video.width,
                    height: plan.video.height,
                },
                fps: plan.video.fps,
                bitrate_kbps: plan.video.bitrate_kbps,
                gop: plan.video.gop,
            };
            let (families, warnings) =
                input::encoded_audio_families(game_audio.filter(|_| audio), &named);
            for w in warnings {
                warn!("{w}");
            }
            let i = if opts.input == InputSpec::TestPattern {
                BuiltinInput::test_pattern(out, pattern_rate, families, stop)?
            } else {
                let video_in = opts
                    .raw_video
                    .clone()
                    .ok_or_else(|| Error::usage("--raw-video missing"))?;
                BuiltinInput::open_slrw_with(
                    io.stdin,
                    BuiltinOptions {
                        video_in,
                        video_out: out,
                        audio: opts.raw_audio.clone().filter(|_| audio),
                        audio_families: families,
                        threads: 0,
                    },
                    stop,
                )?
            };
            Ok((Box::new(i) as Box<dyn Input>, plan.latency_ms))
        }
        Route::Ffmpeg => {
            let plan = plan(&d, opts.encoder, &caps, audio)?;
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
            let (bin, probe) = tools.get()?;
            let (source, raw) = ffmpeg_source(&opts, &input, io.stdin)?;
            let i = FfmpegInput::start_with(bin, probe, &source, &plan, raw, stop)?;
            Ok((Box::new(i) as Box<dyn Input>, plan.latency_ms))
        }
    })();
    drop(caps);
    drop(named);
    let (input, latency_ms) = match opened {
        Ok(v) => v,
        Err(e) => {
            stop_all(sinks);
            if stop() {
                info!("stopped");
                return Ok(());
            }
            return Err(e);
        }
    };
    match stream(input, latency_ms, sinks, stop) {
        Ok(EndReason::Interrupted) => {
            info!("stopped");
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Share a browser tab (decision 0012, option C)

/// `share`'s frame rate without `--fps`.
const SHARE_FPS: Ratio = Ratio::integer(30);
/// The most `share` offers Cast without `--size`/`--fps`; the ANSWER may
/// allow less.
const SHARE_OFFER_SIZE: Size = Size {
    width: 1920,
    height: 1080,
};
const SHARE_OFFER_FPS: Ratio = Ratio::integer(60);

/// What `share` offers Cast: upper bounds, not the stream's size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ShareOffer {
    size: Size,
    fps: Ratio,
    bitrate_kbps: u32,
}

impl ShareOffer {
    fn new(opts: &ShareOptions) -> ShareOffer {
        let size = opts.size.unwrap_or(SHARE_OFFER_SIZE);
        let fps = opts.fps.unwrap_or(SHARE_OFFER_FPS);
        ShareOffer {
            size,
            fps,
            bitrate_kbps: opts
                .bitrate_kbps
                .unwrap_or_else(|| default_bitrate(size, fps)),
        }
    }
}

/// "1920x1080@60, 12000 kbit/s": the tightest limits of all receivers.
fn receiver_limits(caps: &[&Capabilities]) -> String {
    let w = caps.iter().map(|c| c.max_width).min().unwrap_or(0);
    let h = caps.iter().map(|c| c.max_height).min().unwrap_or(0);
    let fps = caps
        .iter()
        .map(|c| c.max_fps)
        .filter(|f| f.num > 0 && f.den > 0)
        .reduce(ratio_min);
    let kbps = caps.iter().filter_map(|c| c.max_bitrate_kbps).min();
    format!(
        "{w}x{h}{}{}",
        fps.map(|f| format!("@{f}")).unwrap_or_default(),
        kbps.map(|k| format!(", {k} kbit/s")).unwrap_or_default()
    )
}

fn share(opts: ShareOptions) -> Result<()> {
    if !interrupt::install() {
        warn!("cannot catch Ctrl-C; receivers will not get a clean stop");
    }
    interrupt::watch_stdin();
    share_with(opts, &connect_one, interrupt::requested, &|_| {})
}

/// `share` with the connector and stop request given, and a hook that
/// sees the server once it listens (the tests start a fake page there).
///
/// The page comes first: once it captured something, its size decides
/// the offer; then the receivers connect, the plan goes to the page and
/// its stream runs through `stream` like an encoded-SLRW game's.
fn share_with(
    opts: ShareOptions,
    connect: Connector<'_>,
    stop: fn() -> bool,
    on_listening: &dyn Fn(&ShareServer),
) -> Result<()> {
    crate::util::timer::raise_resolution();
    let mut server = ShareServer::bind(opts.port)?;
    info!(
        "open {} in a browser and choose what to share (Ctrl-C to stop)",
        server.url()
    );
    if opts.open {
        if let Err(e) = crate::browser::platform::open_url(server.url()) {
            warn!("--open: {e}");
        }
    }
    on_listening(&server);
    let fail = |server: &mut ShareServer, e: Error| {
        server.set_end_reason(&e.to_string());
        e
    };
    let targets = resolve_all(&opts.to).map_err(|e| fail(&mut server, e))?;
    let names: Vec<String> = targets.iter().map(|t| target_name(t).to_owned()).collect();
    // One PCM stream for both families: 44.1 kHz when AirPlay listens.
    let pcm = RawAudioSpec {
        sample_rate: builtin::pattern_sample_rate(
            targets.iter().any(|t| matches!(t, Target::AirPlay { .. })),
        ),
        channels: 2,
        sample_fmt: "s16le".into(),
    };
    let fps = opts.fps.unwrap_or(SHARE_FPS);
    server.configure(&PageConfig {
        fps: fps.as_f64(),
        audio: !opts.no_audio,
        sample_rate: pcm.sample_rate,
        channels: pcm.channels,
        receivers: names.clone(),
    });
    server.set_status("waiting", "Press Share and pick what to show.", &names);
    let Some(cap) = server.wait_capture(stop) else {
        return Ok(());
    };
    let audio = !opts.no_audio && cap.audio;
    if !opts.no_audio && !cap.audio {
        info!(
            "share: the capture has no audio (the browser's picker has a \"share audio\" option)"
        );
    }
    // Sizing: the stream is the capture's size (or --size), scaled down
    // only as far as the receivers' real limits require. The Cast offer
    // is therefore not the capture size but the most we would send
    // (--size/--fps/--bitrate or 1920x1080@60), and the ANSWER's
    // constraints narrow it; AirPlay's limit is the canvas from SETUP.
    // `plan` then fits the capture's aspect into the tightest of them.
    let offer = ShareOffer::new(&opts);
    let fps = opts.fps.unwrap_or(SHARE_FPS);
    let d = Desired {
        size: opts.size.unwrap_or(Size {
            width: cap.width.max(2),
            height: cap.height.max(2),
        }),
        fps,
        // Without --bitrate: chosen from the planned size, below.
        bitrate_kbps: opts.bitrate_kbps.unwrap_or(offer.bitrate_kbps),
        latency_ms: opts.latency_ms.unwrap_or(DEFAULT_LATENCY_MS),
    };
    let mut cast_opts = cast_options(&d, audio);
    cast_opts.max_width = offer.size.width;
    cast_opts.max_height = offer.size.height;
    cast_opts.max_fps = offer.fps;
    cast_opts.max_bitrate_kbps = offer.bitrate_kbps;
    if let Some(f) = input::encoded_cast_audio(Some(&pcm)).filter(|_| audio) {
        cast_opts.audio_codecs = vec![AudioCodec::AacLc];
        cast_opts.audio_sample_rate = f.sample_rate;
        cast_opts.audio_channels = f.channels;
    }
    let budget = shared_budget(opts.latency_ms, &targets);
    cast_opts.latency_ms = budget.cast_ms;
    let mirror = crate::airplay::session::MirrorOptions {
        latency_ms: budget.airplay_ms,
        audio,
        ..Default::default()
    };
    server.set_status("connecting", "", &names);
    let sinks =
        connect_all(&targets, connect, &cast_opts, &mirror).map_err(|e| fail(&mut server, e))?;
    if stop() {
        stop_all(sinks);
        return Ok(());
    }
    let names: Vec<String> = sinks.iter().map(|s| s.name().to_owned()).collect();
    let caps: Vec<&Capabilities> = sinks.iter().map(|s| s.capabilities()).collect();
    let named: Vec<(&str, &Capabilities)> =
        sinks.iter().map(|s| (s.name(), s.capabilities())).collect();
    let limits = receiver_limits(&caps);
    // The receivers' screen shape: the stream has it, the capture is
    // centered inside (bars), since receivers stretch to fill.
    let aspects: Vec<Option<(u32, u32)>> = caps.iter().map(|c| c.display_aspect).collect();
    let aspect = layout::output_aspect(&aspects);
    let mut shapes: Vec<(u32, u32)> = aspects
        .iter()
        .map(|a| a.unwrap_or(layout::DEFAULT_ASPECT))
        .collect();
    shapes.dedup();
    if shapes.len() > 1 {
        warn!(
            "share: the receivers' screens differ in shape ({}); streaming {}:{}, the others stretch it",
            shapes
                .iter()
                .map(|(w, h)| format!("{w}:{h}"))
                .collect::<Vec<_>>()
                .join(", "),
            aspect.0,
            aspect.1
        );
    }
    let max = (
        caps.iter().map(|c| c.max_width).min().unwrap_or(0),
        caps.iter().map(|c| c.max_height).min().unwrap_or(0),
    );
    let d = Desired {
        size: opts.size.unwrap_or_else(|| {
            let (width, height) = layout::output_size((cap.width, cap.height), aspect, max);
            Size { width, height }
        }),
        ..d
    };
    let planned = plan(&d, Encoder::Auto, &caps, false).map(|mut p| {
        if opts.bitrate_kbps.is_none() {
            let size = Size {
                width: p.video.width,
                height: p.video.height,
            };
            p.video.bitrate_kbps = p.video.bitrate_kbps.min(default_bitrate(size, p.video.fps));
        }
        p
    });
    let (families, warnings) = if audio {
        input::encoded_audio_families(Some(&pcm), &named)
    } else {
        (Vec::new(), Vec::new())
    };
    drop((caps, named));
    let plan = match planned {
        Ok(p) => p,
        Err(e) => {
            stop_all(sinks);
            return Err(fail(&mut server, e));
        }
    };
    for w in warnings {
        warn!("{w}");
    }
    let page_audio = (!families.is_empty()).then_some((pcm.sample_rate, pcm.channels));
    let content = layout::place(
        (cap.width, cap.height),
        (plan.video.width, plan.video.height),
    );
    info!(
        "share: capture {}x{}, receivers allow {limits}, screen {}:{}; the capture fills {}x{} at +{}+{}",
        cap.width, cap.height, aspect.0, aspect.1, content.width, content.height, content.x, content.y
    );
    info!(
        "plan: {}x{}@{} {} kbit/s, keyframe every {} frames, {}, latency {} ms (encoded by the browser)",
        plan.video.width,
        plan.video.height,
        plan.video.fps,
        plan.video.bitrate_kbps,
        plan.video.gop,
        match page_audio {
            Some((rate, ch)) => format!("PCM {rate} Hz {ch} ch"),
            None => "no audio".into(),
        },
        plan.latency_ms
    );
    server.set_status("streaming", "", &names);
    let share_plan = SharePlan {
        width: plan.video.width,
        height: plan.video.height,
        fps: plan.video.fps.as_f64(),
        bitrate_bps: plan.video.bitrate_kbps as u64 * 1000,
        key_interval: plan.video.gop,
        audio: page_audio,
        content,
    };
    let opened = server.start(
        &share_plan,
        ShareInputOptions {
            fps: plan.video.fps,
            audio: page_audio.map(|_| pcm.clone()),
            audio_families: families,
        },
        stop,
    );
    let input = match opened {
        Ok(i) => i,
        Err(e) => {
            stop_all(sinks);
            if stop() {
                info!("stopped");
                return Ok(());
            }
            return Err(e);
        }
    };
    match stream(Box::new(input), plan.latency_ms, sinks, stop) {
        Ok(EndReason::Interrupted) => {
            info!("stopped");
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

/// `--latency` bounds (AirPlay accepts 5–2000 ms, Cast 1–5000 ms; the
/// intersection keeps one budget valid for both).
const MIN_LATENCY_MS: u32 = 5;
const MAX_LATENCY_MS: u32 = 2000;

/// The playout budget each family is set up with (decision 0010).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Budget {
    /// Cast target delay (audio and video).
    cast_ms: u32,
    /// AirPlay leads: `Some(L)` = video and audio lead L; `None` = the
    /// automatic profile ("wireless": video 100 ms, audio 170 ms).
    airplay_ms: Option<u32>,
}

/// One budget for every receiver (PLAN §5.3): `--latency L` is the
/// delay from the shared capture instant to presentation on all of them.
/// Without it, Cast uses [`DEFAULT_LATENCY_MS`] and AirPlay-only sessions
/// keep the automatic profile; a session with both families gives
/// AirPlay [`DEFAULT_LATENCY_MS`] as well, so the screens show a frame at
/// the same time instead of 100 ms apart.
fn shared_budget(user: Option<u32>, targets: &[Target]) -> Budget {
    let cast = targets.iter().any(|t| matches!(t, Target::Cast { .. }));
    let airplay = targets.iter().any(|t| matches!(t, Target::AirPlay { .. }));
    let b = match user {
        Some(l) => {
            let c = l.clamp(MIN_LATENCY_MS, MAX_LATENCY_MS);
            if c != l {
                warn!("--latency {l} ms is out of range; using {c} ms");
            }
            Budget {
                cast_ms: c,
                airplay_ms: Some(c),
            }
        }
        None if cast && airplay => Budget {
            cast_ms: DEFAULT_LATENCY_MS,
            airplay_ms: Some(DEFAULT_LATENCY_MS),
        },
        None => Budget {
            cast_ms: DEFAULT_LATENCY_MS,
            airplay_ms: None,
        },
    };
    if targets.len() > 1 {
        let ms = |v: u32| format!("{v} ms");
        info!(
            "{} receivers on one timeline; playout budget {}",
            targets.len(),
            match (cast, airplay, b.airplay_ms) {
                (_, true, None) => "AirPlay automatic (video 100 ms, audio 170 ms)".to_owned(),
                (_, true, Some(a)) => ms(a),
                _ => ms(b.cast_ms),
            }
        );
    }
    b
}

/// The playout delay shared by all sinks: `--latency` or the default,
/// raised to every receiver's minimum.
fn latency(d: &Desired, caps: &[&Capabilities]) -> u32 {
    caps.iter()
        .filter_map(|c| c.min_latency_ms)
        .fold(d.latency_ms, u32::max)
}

/// Raw SLRW for ffmpeg: the bound listeners and the game's stream.
type RawFeed = (rawin::RawInput, Box<dyn std::io::Read + Send>);

/// The ffmpeg input for `opts`.
fn ffmpeg_source(
    opts: &PlayOptions,
    input: &InputInfo,
    stdin: Box<dyn std::io::Read + Send>,
) -> Result<(Source, Option<RawFeed>)> {
    Ok(match &opts.input {
        InputSpec::Path(p) => (
            Source::Url {
                url: p.clone(),
                realtime: opts
                    .realtime
                    .unwrap_or(input.duration.is_some() || Path::new(p).is_file()),
                looped: opts.looped,
            },
            None,
        ),
        InputSpec::FfmpegArgs(a) => (Source::Args(a.clone()), None),
        InputSpec::TestPattern => (Source::TestPattern { clock: true }, None),
        InputSpec::Stdin => {
            let r = rawin::RawInput::bind(opts.raw_video.as_ref(), opts.raw_audio.as_ref())?;
            let s = r.source();
            (s, Some((r, stdin)))
        }
    })
}

/// Steps 4 and 5: input → fanout, then teardown. Consumes the sinks; they
/// are stopped on every path.
fn stream(
    mut input: Box<dyn Input>,
    latency_ms: u32,
    sinks: Vec<Box<dyn Sink>>,
    stop: fn() -> bool,
) -> Result<EndReason> {
    let info = input.info().clone();
    let v = &info.video;
    info!(
        "{}: {:?} {}x{}@{}{}",
        input.describe(),
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
    let fanout = match Fanout::new(sinks, &info, FanoutConfig::default()) {
        Ok(f) => f,
        Err(e) => {
            let _ = input.finish(false);
            return Err(e);
        }
    };

    // Pull the first frame only once every sink is set up: live inputs
    // (the built-in encoder, games) start their clock with it, so nothing
    // piles up in the queues of sinks still in start().
    let ready = Instant::now();
    if !fanout.wait_started(SINK_START_WAIT, stop) && !stop() {
        warn!("a receiver is slow to start; streaming anyway");
    }
    debug!("sinks started in {} ms", ready.elapsed().as_millis());
    let t0 = Instant::now();
    let mut frames = [0u64; 2];
    let mut input_error = None;
    let end = loop {
        if stop() {
            break EndReason::Interrupted;
        }
        match input.next_frame() {
            Ok(Some(f)) => {
                frames[(f.track != TrackKind::Video) as usize] += 1;
                fanout.push(&f);
                if fanout.keyframe_wanted() {
                    input.request_keyframe();
                }
                if fanout.active() == 0 {
                    info!("every receiver ended the session");
                    break EndReason::ReceiversGone;
                }
            }
            Ok(None) if stop() => break EndReason::Interrupted,
            Ok(None) => break EndReason::Eof,
            Err(e) if stop() => {
                debug!("input after stop request: {e}");
                break EndReason::Interrupted;
            }
            Err(e) => {
                input_error = Some(e);
                break EndReason::Eof;
            }
        }
    };
    let elapsed = t0.elapsed();
    let clean_eof = end == EndReason::Eof && input_error.is_none();
    if clean_eof {
        // Let the last frames play out before STOP.
        std::thread::sleep(Duration::from_millis(latency_ms as u64 + 100));
    }
    let stats = fanout.stats();
    let ends = fanout.close(end == EndReason::Eof);
    // ffmpeg: on a clean EOF it has exited (its status counts); otherwise
    // it is killed.
    let input_result = input.finish(clean_eof);

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
    if let Some(e) = input_error {
        return Err(e);
    }
    input_result?;
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
mod multi_tests;

#[cfg(test)]
mod share_tests;

#[cfg(test)]
mod browser_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::ffmpeg::Output;

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
            display_aspect: None,
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
        let airplay = |a: &str| Target::AirPlay {
            name: a.into(),
            addr: a.parse().unwrap(),
            device: None,
        };
        assert_eq!(t("10.0.0.5:7000"), Some(airplay("10.0.0.5:7000")));
        assert_eq!(t("airplay:10.0.0.5"), Some(airplay("10.0.0.5:7000")));
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

    pub(super) fn opts() -> PlayOptions {
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
        let dir = std::env::temp_dir().join(format!("tvpipe-session-{}", std::process::id()));
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
        // (Up to 104: the AAC track of the 1 s file is a little longer.)
        assert!((90..=104).contains(&a.frames.len()), "{}", a.frames.len());
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

    /// A file x264 made without B-frames (1 s, GOP 15, AAC-LC 48 kHz).
    fn passthrough_file(name: &str) -> Option<std::path::PathBuf> {
        let (bin, _) = crate::media::ffmpeg::tests::real_ffmpeg()?;
        let dir = std::env::temp_dir().join(format!("tvpipe-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(name);
        let ok = StdCommand::new(&bin)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
            .args([
                "-i",
                "testsrc2=size=320x240:rate=30:duration=1",
                "-f",
                "lavfi",
            ])
            .args(["-i", "sine=frequency=440:sample_rate=48000:duration=1"])
            .args([
                "-c:v", "libx264", "-bf", "0", "-g", "15", "-pix_fmt", "yuv420p",
            ])
            .args(["-c:a", "aac", "-ac", "2"])
            .arg(&path)
            .status()
            .ok()?
            .success();
        ok.then_some(path)
    }

    const NO_FFMPEG: &str = "/nonexistent/tvpipe-test/ffmpeg";

    fn is_adts(data: &[u8]) -> bool {
        data.len() > 7 && data[0] == 0xff && data[1] & 0xf6 == 0xf0
    }

    #[test]
    fn passthrough_to_cast_without_ffmpeg() {
        use crate::cast::fake_receiver::{FakeReceiver, Options};
        let Some(path) = passthrough_file("pt.mp4") else {
            return;
        };
        let rx = FakeReceiver::spawn(Options::default());
        play(file_opts(NO_FFMPEG.into(), path, rx.addr.to_string())).unwrap();
        let state = rx.shutdown();
        let st = state.lock().unwrap();
        assert!(st.stopped, "STOP not received");
        // The file's AAC is offered alone, so the receiver takes it.
        let offer = st.offer.as_ref().unwrap().to_string();
        assert!(
            offer.contains("\"aac\"") && !offer.contains("\"opus\""),
            "{offer}"
        );
        let v = st.streams.iter().find(|s| s.codec == "h264").unwrap();
        let a = st.streams.iter().find(|s| s.codec == "aac").unwrap();
        assert_eq!(v.frames.len(), 30);
        assert!(v.frames[&0].keyframe);
        // 1 s in 1024-sample units plus the encoder's priming unit.
        assert!((46..=49).contains(&a.frames.len()), "{}", a.frames.len());
        assert!(a.frames.values().all(|f| is_adts(&f.data)));
    }

    #[test]
    fn b_frames_need_ffmpeg() {
        use crate::cast::fake_receiver::{FakeReceiver, Options};
        let Some((_, path)) = media_file("bf.mp4", false) else {
            return;
        };
        let rx = FakeReceiver::spawn(Options::default());
        let e = play(file_opts(NO_FFMPEG.into(), path, rx.addr.to_string())).unwrap_err();
        assert!(e.to_string().contains(NO_FFMPEG), "{e}");
        let state = rx.shutdown();
        assert!(state.lock().unwrap().stopped, "STOP not received");
    }

    /// Hands out SLRW records at their due times, like a game.
    struct PacedReader {
        chunks: std::collections::VecDeque<(Duration, Vec<u8>)>,
        start: Option<Instant>,
        cur: Vec<u8>,
        pos: usize,
    }

    impl std::io::Read for PacedReader {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let start = *self.start.get_or_insert_with(Instant::now);
            if self.pos == self.cur.len() {
                let Some((due, chunk)) = self.chunks.pop_front() else {
                    return Ok(0);
                };
                std::thread::sleep((start + due).saturating_duration_since(Instant::now()));
                self.cur = chunk;
                self.pos = 0;
            }
            let n = out.len().min(self.cur.len() - self.pos);
            out[..n].copy_from_slice(&self.cur[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn encoded_slrw_to_cast_without_ffmpeg() {
        use crate::cast::fake_receiver::{FakeReceiver, Options};
        use crate::media::input::tests::{access_units, annexb_stream, SharedBuf};
        use crate::media::slrw::{self, Kind};
        let Some(stream) = annexb_stream() else {
            return;
        };
        let aus = access_units(&stream);
        let mut chunks = std::collections::VecDeque::new();
        let mut head = Vec::new();
        slrw::write_magic(&mut head).unwrap();
        slrw::write_control(&mut head, slrw::CONTROL_KEYFRAME_REQUESTS).unwrap();
        chunks.push_back((Duration::ZERO, head));
        for (i, au) in aus.iter().enumerate() {
            let pts = i as u64 * 33_333;
            let mut rec = Vec::new();
            slrw::write_h264(&mut rec, pts, i % 10 == 0, true, au).unwrap();
            slrw::write_packet(&mut rec, Kind::Audio, pts, &[3u8; 1600 * 4]).unwrap();
            chunks.push_back((Duration::from_micros(pts), rec));
        }
        let mut end = Vec::new();
        slrw::write_packet(&mut end, Kind::End, 0, &[]).unwrap();
        chunks.push_back((Duration::from_millis(1000), end));
        let rx = FakeReceiver::spawn(Options::default());
        let o = PlayOptions {
            to: vec![rx.addr.to_string()],
            latency_ms: Some(100),
            ffmpeg: Some(NO_FFMPEG.into()),
            raw_video: Some("160x120@30:h264".parse().unwrap()),
            raw_audio: Some("48000:2:s16le".parse().unwrap()),
            input: InputSpec::Stdin,
            ..opts()
        };
        let out = SharedBuf::default();
        let io = GameIo {
            stdin: Box::new(PacedReader {
                chunks,
                start: None,
                cur: Vec::new(),
                pos: 0,
            }),
            stdout: Box::new(out.clone()),
        };
        play_io(o, io).unwrap();
        let state = rx.shutdown();
        let st = state.lock().unwrap();
        assert!(st.stopped, "STOP not received");
        let offer = st.offer.as_ref().unwrap().to_string();
        assert!(
            offer.contains("\"aac\"") && !offer.contains("\"opus\""),
            "{offer}"
        );
        let v = st.streams.iter().find(|s| s.codec == "h264").unwrap();
        let a = st.streams.iter().find(|s| s.codec == "aac").unwrap();
        assert_eq!(v.frames.len(), 30);
        // 48000 samples of PCM through aacenc: 46 full units (+ the flush).
        assert!((45..=48).contains(&a.frames.len()), "{}", a.frames.len());
        assert!(a.frames.values().all(|f| is_adts(&f.data)));
        // Only "keyframe" lines (if any) went to the game.
        let back = out.0.lock().unwrap().clone();
        assert!(back
            .split(|&b| b == b'\n')
            .all(|l| l.is_empty() || l == b"keyframe"));
    }

    /// What a sink did with the frames the session gave it, per track.
    #[derive(Debug, Default, Clone, Copy)]
    pub(super) struct Tally {
        pub(super) video: u64,
        pub(super) video_accepted: u64,
        pub(super) audio: u64,
        pub(super) audio_accepted: u64,
    }

    pub(super) type SharedTally = std::sync::Arc<std::sync::Mutex<Tally>>;

    /// Counts what the wrapped sink accepts; with `left`, ends the session
    /// after that many video frames (the test pattern never ends by
    /// itself).
    pub(super) struct Tallied {
        pub(super) inner: Box<dyn Sink>,
        pub(super) left: Option<usize>,
        pub(super) tally: SharedTally,
    }

    impl Tallied {
        pub(super) fn boxed(
            inner: Box<dyn Sink>,
            left: Option<usize>,
            tally: &SharedTally,
        ) -> Box<dyn Sink> {
            Box::new(Tallied {
                inner,
                left,
                tally: tally.clone(),
            })
        }
    }

    impl Sink for Tallied {
        fn name(&self) -> &str {
            self.inner.name()
        }
        fn capabilities(&self) -> &Capabilities {
            self.inner.capabilities()
        }
        fn set_clock(&mut self, clock: crate::media::clock::SessionClock) {
            self.inner.set_clock(clock)
        }
        fn start(
            &mut self,
            v: &crate::types::VideoInfo,
            a: Option<&crate::types::AudioInfo>,
        ) -> Result<()> {
            self.inner.start(v, a)
        }
        fn push(&mut self, f: &crate::types::EncodedFrame) -> Result<crate::types::SinkStatus> {
            let video = f.track == TrackKind::Video;
            if video {
                match &mut self.left {
                    Some(0) => return Ok(crate::types::SinkStatus::Finished),
                    Some(left) => *left -= 1,
                    None => {}
                }
            }
            let status = self.inner.push(f)?;
            let accepted = (status == crate::types::SinkStatus::Accepted) as u64;
            let mut t = self.tally.lock().unwrap();
            if video {
                t.video += 1;
                t.video_accepted += accepted;
            } else {
                t.audio += 1;
                t.audio_accepted += accepted;
            }
            Ok(status)
        }
        fn wants_keyframe(&self) -> bool {
            self.inner.wants_keyframe()
        }
        fn stop(&mut self) -> Result<()> {
            self.inner.stop()
        }
    }

    /// Cast targets as usual, AirPlay ones to a fake mirror receiver
    /// (which skips pairing); each sink ends after `limit` video frames
    /// and counts into `tally`.
    fn fake_connector(
        fake: Option<&crate::airplay::fake_mirror::FakeMirror>,
        limit: Option<usize>,
        tally: &SharedTally,
        t: &Target,
        cast_opts: &CastOptions,
        mirror: &crate::airplay::session::MirrorOptions,
    ) -> Result<Box<dyn Sink>> {
        let sink: Box<dyn Sink> = match t {
            Target::Cast { addr, name } => Box::new(cast::connect(*addr, name, cast_opts)?),
            Target::AirPlay { .. } => {
                let fake = fake.expect("fake AirPlay receiver");
                fake_airplay(fake, "fake", mirror)?
            }
        };
        Ok(Tallied::boxed(sink, limit, tally))
    }

    /// A mirror sink connected to `fake` (no pairing).
    pub(super) fn fake_airplay(
        fake: &crate::airplay::fake_mirror::FakeMirror,
        name: &str,
        mirror: &crate::airplay::session::MirrorOptions,
    ) -> Result<Box<dyn Sink>> {
        let (addr, secret) = (fake.addr, fake.secret.clone());
        Ok(Box::new(crate::airplay::sink::connect_with(
            name.into(),
            mirror,
            || {
                let tcp = std::net::TcpStream::connect(addr)
                    .map_err(|e| Error::io("fake airplay connect", e))?;
                Ok((
                    crate::net::rtsp::RtspClient::new(tcp),
                    addr,
                    secret.clone(),
                    true,
                ))
            },
        )?))
    }

    /// Never asks to stop (the tests must not touch the process-wide
    /// Ctrl-C flag).
    pub(super) fn never() -> bool {
        false
    }

    /// Paced raw RGBA SLRW (like `slrw-gen`), 1 s at 30 fps with PCM.
    fn raw_game_io(rate: u32) -> GameIo {
        use crate::media::builtin::tests::{rgba_bars, Paced};
        use crate::media::slrw::{self, Kind};
        let mut chunks = std::collections::VecDeque::new();
        let mut head = Vec::new();
        slrw::write_magic(&mut head).unwrap();
        chunks.push_back((Duration::ZERO, head));
        let per_frame = rate as usize / 30;
        for i in 0..30u64 {
            let pts = 1_000_000 + i * 33_333;
            let mut rec = Vec::new();
            slrw::write_packet(&mut rec, Kind::Video, pts, &rgba_bars(320, 180, i)).unwrap();
            let pcm: Vec<u8> = (0..per_frame * 2)
                .flat_map(|k| (((k as f64 * 0.07).sin() * 5000.0) as i16).to_le_bytes())
                .collect();
            slrw::write_packet(&mut rec, Kind::Audio, pts, &pcm).unwrap();
            chunks.push_back((Duration::from_micros(i * 33_333), rec));
        }
        let mut end = Vec::new();
        slrw::write_packet(&mut end, Kind::End, 0, &[]).unwrap();
        chunks.push_back((Duration::from_millis(1000), end));
        GameIo {
            stdin: Box::new(Paced::new(chunks)),
            stdout: Box::new(std::io::sink()),
        }
    }

    fn raw_opts(to: String, rate: u32) -> PlayOptions {
        PlayOptions {
            to: vec![to],
            latency_ms: Some(100),
            ffmpeg: Some(NO_FFMPEG.into()),
            raw_video: Some("320x180@30:rgba".parse().unwrap()),
            raw_audio: Some(format!("{rate}:2:s16le").parse().unwrap()),
            input: InputSpec::Stdin,
            ..opts()
        }
    }

    /// The H.264 a Cast receiver got decodes (when ffmpeg is around).
    fn check_decodes(
        frames: &std::collections::BTreeMap<
            crate::cast::rtp::FrameId,
            crate::cast::fake_receiver::Frame,
        >,
    ) {
        let Some((bin, _)) = crate::media::ffmpeg::tests::real_ffmpeg() else {
            return;
        };
        let stream: Vec<u8> = frames.values().flat_map(|f| f.data.clone()).collect();
        let mut child = StdCommand::new(bin)
            .args(["-v", "error", "-f", "h264", "-i", "-", "-f", "null", "-"])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child.stdin.take().unwrap().write_all(&stream).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success() && out.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Every game frame of [`raw_game_io`] is accounted for by the
    /// built-in input. How many the encoder drops while busy depends on
    /// machine load, and the first record may come before the session
    /// pulls.
    fn assert_all_frames_accounted(c: &builtin::Counts) {
        assert_eq!(c.frames, 30, "{c:?}");
        assert_eq!(c.rate, 0, "{c:?}");
        assert_eq!(c.early + c.busy + c.encoded, 30, "{c:?}");
        assert!(c.encoded >= 1, "{c:?}");
    }

    /// The PCM frames of `frame_samples` the game's records after the
    /// first `early` (whose video was dropped before the start) make; the
    /// gate may open between a record's video and its audio.
    fn audio_frames(
        c: &builtin::Counts,
        record_samples: u64,
        frame_samples: u64,
    ) -> std::ops::RangeInclusive<u64> {
        (30 - c.early) * record_samples / frame_samples
            ..=(31 - c.early) * record_samples / frame_samples + 1
    }

    /// What a Cast receiver got of a track whose sink accepted `accepted`
    /// frames: a prefix of them, from frame 0 without holes (losses are
    /// retransmitted). Frames still being paced out when STOP comes may
    /// be missing at the end.
    pub(super) fn assert_cast_frames(
        frames: &std::collections::BTreeMap<
            crate::cast::rtp::FrameId,
            crate::cast::fake_receiver::Frame,
        >,
        accepted: u64,
    ) {
        assert!(!frames.is_empty());
        assert_eq!(frames.keys().next(), Some(&0), "the first frame is missing");
        assert!(
            frames.len() as u64 <= accepted,
            "{} > {accepted}",
            frames.len()
        );
        let ids: Vec<_> = frames.values().map(|f| f.frame_id).collect();
        assert!(ids.windows(2).all(|w| w[1] == w[0] + 1), "holes in {ids:?}");
    }

    /// Video RTP timestamps are media time: frames dropped on the way
    /// leave gaps of whole frames (3000 ticks at 30 fps), they never shift
    /// the timeline.
    fn assert_on_frame_grid(ts: &[u32]) {
        for w in ts.windows(2) {
            let d = w[1].wrapping_sub(w[0]);
            let k = (d + 1500) / 3000;
            assert!(k >= 1 && d.abs_diff(k * 3000) <= 1, "{ts:?}");
        }
    }

    #[test]
    fn raw_slrw_to_cast_without_ffmpeg() {
        use crate::cast::fake_receiver::{FakeReceiver, Options};
        let rx = FakeReceiver::spawn(Options::default());
        let tally = SharedTally::default();
        let connect = |t: &Target, c: &CastOptions, m: &crate::airplay::session::MirrorOptions| {
            fake_connector(None, None, &tally, t, c, m)
        };
        play_with(
            raw_opts(rx.addr.to_string(), 48_000),
            raw_game_io(48_000),
            &connect,
            never,
        )
        .unwrap();
        let c = builtin::take_finished_counts().expect("built-in input");
        let t = *tally.lock().unwrap();
        let state = rx.shutdown();
        let st = state.lock().unwrap();
        assert!(st.stopped, "STOP not received");
        let offer = st.offer.as_ref().unwrap().to_string();
        assert!(
            offer.contains("\"aac\"") && !offer.contains("\"opus\""),
            "{offer}"
        );
        let v = st.streams.iter().find(|s| s.codec == "h264").unwrap();
        let a = st.streams.iter().find(|s| s.codec == "aac").unwrap();
        assert_all_frames_accounted(&c);
        // One sink: it got every encoded frame unless its queue was full.
        assert!(t.video <= c.encoded, "{t:?} {c:?}");
        assert_cast_frames(&v.frames, t.video_accepted);
        assert!(v.frames.values().next().unwrap().keyframe);
        let ts: Vec<u32> = v.frames.values().map(|f| f.rtp_timestamp).collect();
        assert_on_frame_grid(&ts);
        // PCM is never dropped for the encoder: 1600 samples per record in
        // 1024-sample AAC units.
        assert!(
            audio_frames(&c, 1600, 1024).contains(&t.audio),
            "{t:?} {c:?}"
        );
        assert_cast_frames(&a.frames, t.audio_accepted);
        assert!(a.frames.values().all(|f| is_adts(&f.data)));
        check_decodes(&v.frames);
    }

    #[test]
    fn test_pattern_to_cast_builtin() {
        use crate::cast::fake_receiver::{FakeReceiver, Options};
        let rx = FakeReceiver::spawn(Options::default());
        let tally = SharedTally::default();
        let connect = |t: &Target, c: &CastOptions, m: &crate::airplay::session::MirrorOptions| {
            fake_connector(None, Some(20), &tally, t, c, m)
        };
        let o = PlayOptions {
            to: vec![rx.addr.to_string()],
            latency_ms: Some(100),
            size: Some(Size {
                width: 640,
                height: 360,
            }),
            encoder: Encoder::Builtin,
            ..opts()
        };
        let io = GameIo {
            stdin: Box::new(std::io::empty()),
            stdout: Box::new(std::io::sink()),
        };
        play_with(o, io, &connect, never).unwrap();
        let t = *tally.lock().unwrap();
        let state = rx.shutdown();
        let st = state.lock().unwrap();
        assert!(st.stopped, "STOP not received");
        let v = st.streams.iter().find(|s| s.codec == "h264").unwrap();
        let a = st.streams.iter().find(|s| s.codec == "aac").unwrap();
        // The pattern never ends: the session ran until the sink had 20.
        assert_eq!(t.video, 20, "{t:?}");
        assert_cast_frames(&v.frames, t.video_accepted);
        assert!(v.frames.values().next().unwrap().keyframe);
        assert_cast_frames(&a.frames, t.audio_accepted);
        check_decodes(&v.frames);
    }

    #[test]
    fn raw_slrw_and_test_pattern_to_airplay_without_ffmpeg() {
        use crate::airplay::fake_mirror::{Behaviour, FakeMirror};
        // Raw SLRW, --encoder auto with a bogus ffmpeg.
        let fake = FakeMirror::start(Behaviour::default());
        let tally = SharedTally::default();
        let connect = |t: &Target, c: &CastOptions, m: &crate::airplay::session::MirrorOptions| {
            fake_connector(Some(&fake), None, &tally, t, c, m)
        };
        let to = format!("airplay:{}", fake.addr);
        play_with(
            raw_opts(to.clone(), 44_100),
            raw_game_io(44_100),
            &connect,
            never,
        )
        .unwrap();
        {
            let c = builtin::take_finished_counts().expect("built-in input");
            let t = *tally.lock().unwrap();
            let r = fake.rec();
            assert!(r.teardown);
            let frames = r.video.iter().filter(|p| p.kind == 0).count();
            assert_all_frames_accounted(&c);
            assert!(t.video <= c.encoded, "{t:?} {c:?}");
            // Mirroring video is TCP, and the sink drains its queue on
            // stop: the receiver got every frame the sink accepted.
            assert_eq!(frames as u64, t.video_accepted, "{t:?} {c:?}");
            assert!(frames >= 1);
            assert!(r.video.iter().any(|p| p.kind == 1), "codec packet");
            assert!(r.video.iter().find(|p| p.kind == 0).unwrap().idr);
            // 1470 samples per record in 352-sample PCM frames. The sink
            // drops audio until the first video frame is out, and audio
            // is UDP.
            assert!(
                audio_frames(&c, 1470, 352).contains(&t.audio),
                "{t:?} {c:?}"
            );
            assert!(
                (1..=t.audio_accepted).contains(&(r.audio.len() as u64)),
                "{t:?}"
            );
        }
        // The test pattern, built in because ffmpeg is missing.
        let fake = FakeMirror::start(Behaviour::default());
        let tally = SharedTally::default();
        let connect = |t: &Target, c: &CastOptions, m: &crate::airplay::session::MirrorOptions| {
            fake_connector(Some(&fake), Some(15), &tally, t, c, m)
        };
        let o = PlayOptions {
            to: vec![format!("airplay:{}", fake.addr)],
            latency_ms: Some(100),
            ffmpeg: Some(NO_FFMPEG.into()),
            ..opts()
        };
        let io = GameIo {
            stdin: Box::new(std::io::empty()),
            stdout: Box::new(std::io::sink()),
        };
        play_with(o, io, &connect, never).unwrap();
        let t = *tally.lock().unwrap();
        let r = fake.rec();
        assert!(r.teardown);
        let frames = r.video.iter().filter(|p| p.kind == 0).count();
        // The pattern never ends: the session ran until the sink had 15.
        assert_eq!(t.video, 15, "{t:?}");
        assert_eq!(frames as u64, t.video_accepted, "{t:?}");
        assert!(frames >= 1);
        assert!(
            (1..=t.audio_accepted).contains(&(r.audio.len() as u64)),
            "{t:?}"
        );
    }

    /// Records when each video frame reaches the sink; `start()` takes
    /// as long as an AirPlay SETUP.
    struct ArrivalSink {
        caps: Capabilities,
        arrivals: std::sync::Arc<std::sync::Mutex<Vec<(Instant, u64)>>>,
        left: usize,
    }

    impl Sink for ArrivalSink {
        fn name(&self) -> &str {
            "arrivals"
        }
        fn capabilities(&self) -> &Capabilities {
            &self.caps
        }
        fn start(
            &mut self,
            _v: &crate::types::VideoInfo,
            _a: Option<&crate::types::AudioInfo>,
        ) -> Result<()> {
            std::thread::sleep(Duration::from_millis(250));
            Ok(())
        }
        fn push(&mut self, f: &crate::types::EncodedFrame) -> Result<crate::types::SinkStatus> {
            if f.track == TrackKind::Video {
                if self.left == 0 {
                    return Ok(crate::types::SinkStatus::Finished);
                }
                self.left -= 1;
                self.arrivals.lock().unwrap().push((Instant::now(), f.pts));
            }
            Ok(crate::types::SinkStatus::Accepted)
        }
        fn wants_keyframe(&self) -> bool {
            false
        }
        fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    /// A live input must not run ahead while a sink is still starting:
    /// frames reach the sink on their pts schedule from the first one, no
    /// burst of backlog (which the sink's clock would turn into latency;
    /// seen as ~230 ms on the Apple TV).
    #[test]
    fn live_input_waits_for_slow_sink_start() {
        let out = VideoOut {
            size: Size {
                width: 160,
                height: 90,
            },
            fps: Ratio::integer(30),
            bitrate_kbps: 300,
            gop: 30,
        };
        let input = BuiltinInput::test_pattern(out, 48_000, vec![], interrupt::requested).unwrap();
        let arrivals = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = ArrivalSink {
            caps: caps(1920, 1080, 60, 10_000),
            arrivals: arrivals.clone(),
            left: 15,
        };
        stream(
            Box::new(input),
            0,
            vec![Box::new(sink)],
            interrupt::requested,
        )
        .unwrap();
        let a = arrivals.lock().unwrap();
        assert_eq!(a.len(), 15);
        let (t0, p0) = a[0];
        for &(t, pts) in a.iter() {
            let due = Duration::from_micros((pts - p0) * 1000 / 90);
            let at = t - t0;
            assert!(
                at + Duration::from_millis(40) >= due,
                "pts {due:?} arrived at {at:?}"
            );
        }
    }

    #[test]
    fn builtin_needs_raw_or_pattern() {
        let o = PlayOptions {
            encoder: Encoder::Builtin,
            input: InputSpec::Path("x.mp4".into()),
            ..opts()
        };
        let mut tools = Tools::new(None);
        assert!(use_builtin(&o, false, &mut tools).is_err());
        let o = PlayOptions {
            encoder: Encoder::Builtin,
            input: InputSpec::Stdin,
            raw_video: Some("64x64@30:yuyv422".parse().unwrap()),
            ..opts()
        };
        let e = use_builtin(&o, false, &mut tools).unwrap_err();
        assert!(e.to_string().contains("cannot read"), "{e}");
        let o = PlayOptions {
            raw_video: Some("64x64@30:bgra".parse().unwrap()),
            ..o
        };
        assert!(use_builtin(&o, false, &mut tools).unwrap());
        // Encoded SLRW never goes through the encoder.
        assert!(!use_builtin(&o, true, &mut tools).unwrap());
        // Auto with a bogus ffmpeg falls back to the built-in encoder.
        let bogus = std::path::PathBuf::from(NO_FFMPEG);
        let mut tools = Tools::new(Some(&bogus));
        let o = PlayOptions {
            encoder: Encoder::Auto,
            ..o
        };
        assert!(use_builtin(&o, false, &mut tools).unwrap());
        let o = PlayOptions {
            input: InputSpec::TestPattern,
            ..o
        };
        assert!(use_builtin(&o, false, &mut tools).unwrap());
    }

    #[test]
    fn test_pattern_is_builtin_unless_an_ffmpeg_encoder_is_named() {
        // Whatever ffmpeg is installed (and whether it has drawtext), the
        // test pattern is the built-in one; ffmpeg is not even probed.
        let bogus = std::path::PathBuf::from(NO_FFMPEG);
        for ffmpeg in [None, Some(&bogus)] {
            let mut tools = Tools::new(ffmpeg.map(|p| p.as_path()));
            for encoder in [Encoder::Auto, Encoder::Builtin] {
                let o = PlayOptions { encoder, ..opts() };
                assert!(use_builtin(&o, false, &mut tools).unwrap());
            }
            assert!(tools.found.is_none(), "ffmpeg was probed");
            // Naming an ffmpeg encoder selects ffmpeg's testsrc2.
            let o = PlayOptions {
                encoder: Encoder::X264,
                ..opts()
            };
            assert!(!use_builtin(&o, false, &mut tools).unwrap());
        }
    }

    #[test]
    fn airplay_target_connect_failure() {
        let o = PlayOptions {
            to: vec!["airplay:127.0.0.1:1".into()],
            ffmpeg: crate::media::ffmpeg::tests::real_ffmpeg().map(|(b, _)| b),
            ..opts()
        };
        if o.ffmpeg.is_none() {
            return;
        }
        let e = play(o).unwrap_err();
        assert_ne!(e.kind(), crate::error::ErrorKind::Unsupported, "{e}");
        assert!(e.to_string().contains("airplay"), "{e}");
    }
}
