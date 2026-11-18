//! Built-in encoding without ffmpeg (decision 0006, option C): raw SLRW
//! game frames, or a generated test pattern, converted to I420
//! (`media::convert`), scaled to the plan size and encoded by
//! `media::h264enc` with IDRs on demand. Audio takes the encoded-SLRW
//! paths (decision 0008): PCM for AirPlay, `media::aacenc` for Cast.
//!
//! Threads: a reader takes records from the game (or the pattern
//! generator), cuts the video down to the plan's frame rate and hands
//! frames to the encoder thread through a queue of [`VIDEO_QUEUE`]; when
//! the encoder falls behind, frames are dropped, so reading never blocks
//! on encoding. Audio records and encoded access units meet in one channel
//! and go through [`EncodedState`], which owns the timeline.
//!
//! The test pattern: scrolling colour bars, a frame counter and the UTC
//! wallclock in milliseconds drawn with a built-in 5x7 digit font, and a
//! 1 kHz beep for the first 100 ms of every second together with a white
//! flash (for A/V sync). The wallclock is taken when the frame is made, so
//! photographing the TV next to a millisecond clock gives the end-to-end
//! latency.
//!
//! Sources: decisions 0002, 0006, 0008; PLAN §5.2; ITU-R BT.709-6 (bar
//! colours via `media::convert`).

use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::cli::{RawAudioSpec, RawVideoSpec, Size};
use crate::error::{Error, Result};
use crate::media::convert::{self, ColorSpec, PixelFormat, Yuv420Frame};
use crate::media::h264enc;
use crate::media::input::{
    AudioOut, EncodedState, Input, StopFn, KEYFRAME_REQUEST_INTERVAL, STOP_POLL,
};
use crate::media::rawin;
use crate::media::slrw::{self, Kind, Packet, SlrwReader};
use crate::media::source::StreamInfo;
use crate::types::{AudioFamily, EncodedFrame};
use crate::util::interrupt;
use crate::util::ratio::Ratio;

/// Raw frames waiting for the encoder; more are dropped.
const VIDEO_QUEUE: usize = 2;
/// Records and access units buffered for the session.
const EVENT_QUEUE: usize = 64;

/// The output video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoOut {
    pub size: Size,
    pub fps: Ratio,
    pub bitrate_kbps: u32,
    /// IDR interval in frames (0: only on request).
    pub gop: u32,
}

/// Options for [`BuiltinInput`].
pub struct BuiltinOptions {
    /// What the game sends (`--raw-video`).
    pub video_in: RawVideoSpec,
    pub video_out: VideoOut,
    pub audio: Option<RawAudioSpec>,
    /// From `input::encoded_audio_families`.
    pub audio_families: Vec<(AudioFamily, AudioOut)>,
    /// Encoder threads (0: automatic).
    pub threads: u32,
}

/// The pixel formats the built-in path converts (`--raw-video`).
pub fn supported_pix_fmt(name: &str) -> bool {
    PixelFormat::from_name(name).is_some()
}

/// Something that yields SLRW records: the game's stream or the pattern.
pub trait PacketSource: Send + 'static {
    fn next_packet(&mut self) -> io::Result<Option<Packet>>;
}

impl<R: Read + Send + 'static> PacketSource for SlrwReader<R> {
    fn next_packet(&mut self) -> io::Result<Option<Packet>> {
        SlrwReader::next_packet(self)
    }
}

struct Job {
    pts_us: u64,
    data: Vec<u8>,
}

#[derive(Default)]
struct Counters {
    frames: AtomicU64,
    dropped_rate: AtomicU64,
    dropped_busy: AtomicU64,
    encoded: AtomicU64,
    encode_us: AtomicU64,
}

/// Raw frames encoded in-process.
pub struct BuiltinInput {
    rx: Receiver<io::Result<Option<Packet>>>,
    st: EncodedState,
    info: StreamInfo,
    keyframe: Arc<AtomicBool>,
    last_request: Option<Instant>,
    counters: Arc<Counters>,
    what: &'static str,
    stop: StopFn,
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

impl BuiltinInput {
    /// Raw SLRW from the game (`--raw-video WxH@FPS:PIXFMT`, stdin).
    pub fn open_slrw<R: Read + Send + 'static>(input: R, opts: BuiltinOptions) -> Result<Self> {
        BuiltinInput::open_slrw_with(input, opts, interrupt::requested)
    }

    pub fn open_slrw_with<R: Read + Send + 'static>(
        input: R,
        opts: BuiltinOptions,
        stop: StopFn,
    ) -> Result<Self> {
        let v = &opts.video_in;
        let limit = PixelFormat::from_name(&v.pix_fmt)
            .map(|f| f.frame_size(v.size.width, v.size.height) as u32)
            .unwrap_or(u32::MAX);
        let reader = SlrwReader::new(input).with_limits(limit, 16 << 20);
        BuiltinInput::start(reader, opts, "SLRW raw", stop)
    }

    /// The generated test pattern at the plan's size and rate, with a
    /// beep at `sample_rate` (stereo) when `audio` families are given.
    pub fn test_pattern(
        video_out: VideoOut,
        sample_rate: u32,
        audio_families: Vec<(AudioFamily, AudioOut)>,
        stop: StopFn,
    ) -> Result<Self> {
        let audio = pattern_audio(sample_rate);
        let pattern = TestPattern::new(video_out.size, video_out.fps, sample_rate, stop)?;
        let opts = BuiltinOptions {
            video_in: RawVideoSpec {
                size: video_out.size,
                fps: video_out.fps,
                pix_fmt: "yuv420p".into(),
            },
            video_out,
            audio: Some(audio),
            audio_families,
            threads: 0,
        };
        BuiltinInput::start(pattern, opts, "test pattern", stop)
    }

    fn start(
        src: impl PacketSource,
        opts: BuiltinOptions,
        what: &'static str,
        stop: StopFn,
    ) -> Result<Self> {
        let fmt = PixelFormat::from_name(&opts.video_in.pix_fmt).ok_or_else(|| {
            Error::unsupported(format!(
                "the built-in encoder cannot read {:?} frames (rgba, bgra, rgb0, bgr0, \
                 rgb24, bgr24, nv12, yuv420p); use ffmpeg (--encoder auto with ffmpeg \
                 installed)",
                opts.video_in.pix_fmt
            ))
        })?;
        let out = opts.video_out;
        let enc = h264enc::Encoder::new(h264enc::Config {
            width: out.size.width,
            height: out.size.height,
            fps: out.fps,
            bitrate_kbps: out.bitrate_kbps,
            gop: out.gop,
            threads: opts.threads,
            ..h264enc::Config::default()
        })?;
        let mut avcc = vec![slrw::CONFIG_AVCC];
        avcc.extend_from_slice(&enc.avcc());
        // The timeline sees the output size (no "--raw-video says" warning
        // when we scale).
        let spec_out = RawVideoSpec {
            size: out.size,
            fps: out.fps,
            pix_fmt: rawin::ENCODED_PIX_FMT.into(),
        };
        let mut st = EncodedState::new(spec_out, opts.audio.clone(), &opts.audio_families, None)?;
        st.set_origin(0);
        st.handle(Packet {
            kind: Kind::H264Config,
            pts_us: 0,
            payload: avcc,
        })?;

        let (tx, rx) = mpsc::sync_channel(EVENT_QUEUE);
        let (job_tx, job_rx) = mpsc::sync_channel::<Job>(VIDEO_QUEUE);
        let keyframe = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(Counters::default());
        let in_spec = opts.video_in.clone();
        {
            let (tx, keyframe, counters) = (tx.clone(), keyframe.clone(), counters.clone());
            std::thread::Builder::new()
                .name("builtin-encoder".into())
                .spawn(move || encode_loop(enc, fmt, in_spec, job_rx, tx, keyframe, counters))
                .map_err(|e| Error::io("spawn encoder thread", e))?;
        }
        {
            let counters = counters.clone();
            let frame_bytes = fmt.frame_size(opts.video_in.size.width, opts.video_in.size.height);
            let in_fps = opts.video_in.fps;
            std::thread::Builder::new()
                .name("builtin-reader".into())
                .spawn(move || read_loop(src, frame_bytes, in_fps, out.fps, job_tx, tx, counters))
                .map_err(|e| Error::io("spawn SLRW reader", e))?;
        }
        let mut me = BuiltinInput {
            rx,
            info: st.stream_info()?,
            st,
            keyframe,
            last_request: None,
            counters,
            what,
            stop,
        };
        // Wait for the first picture (an IDR), like the encoded path.
        while !me.st.started() {
            if me.st.ended() {
                return Err(Error::closed(
                    "slrw: the game's stream ended before the first video frame",
                ));
            }
            if !me.pump()? {
                return Err(Error::closed("stopped before the first video frame"));
            }
        }
        me.info = me.st.stream_info()?;
        Ok(me)
    }

    /// Handles one event; `false` on a stop request.
    fn pump(&mut self) -> Result<bool> {
        loop {
            if (self.stop)() {
                return Ok(false);
            }
            match self.rx.recv_timeout(STOP_POLL) {
                Ok(Ok(Some(p))) => {
                    self.st.handle(p)?;
                    return Ok(true);
                }
                Ok(Ok(None)) | Err(RecvTimeoutError::Disconnected) => {
                    self.st.end();
                    return Ok(true);
                }
                Ok(Err(e)) if e.kind() == io::ErrorKind::InvalidData => {
                    return Err(Error::malformed(e.to_string()))
                }
                Ok(Err(e)) => return Err(Error::io("read SLRW input", e)),
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

/// Reader thread: records → (rate-limited) encoder jobs and audio events.
fn read_loop(
    mut src: impl PacketSource,
    frame_bytes: usize,
    in_fps: Ratio,
    out_fps: Ratio,
    jobs: SyncSender<Job>,
    events: SyncSender<io::Result<Option<Packet>>>,
    counters: Arc<Counters>,
) {
    let mut origin: Option<u64> = None;
    let mut last_kept: Option<u64> = None;
    // Frames closer than this to the last kept one are skipped when the
    // game renders faster than the plan's rate (15% slack for jitter).
    let min_gap_us = if out_fps.as_f64() + 0.01 < in_fps.as_f64() && out_fps.num > 0 {
        (850_000.0 / out_fps.as_f64()) as u64
    } else {
        0
    };
    loop {
        let p = match src.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => {
                let _ = events.send(Err(e));
                break;
            }
        };
        let rel = |o: &mut Option<u64>| p.pts_us.saturating_sub(*o.get_or_insert(p.pts_us));
        match p.kind {
            Kind::End => break,
            Kind::Video => {
                if p.payload.len() != frame_bytes {
                    let _ = events.send(Err(invalid(format!(
                        "slrw: video frame of {} bytes, expected {frame_bytes}",
                        p.payload.len()
                    ))));
                    break;
                }
                counters.frames.fetch_add(1, Ordering::Relaxed);
                let pts_us = rel(&mut origin);
                if last_kept.is_some_and(|l| pts_us < l + min_gap_us) {
                    counters.dropped_rate.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                match jobs.try_send(Job {
                    pts_us,
                    data: p.payload,
                }) {
                    Ok(()) => last_kept = Some(pts_us),
                    Err(TrySendError::Full(_)) => {
                        counters.dropped_busy.fetch_add(1, Ordering::Relaxed);
                        trace!("builtin: encoder busy; dropped a frame");
                    }
                    Err(TrySendError::Disconnected(_)) => break,
                }
            }
            Kind::Audio => {
                let pts_us = rel(&mut origin);
                let p = Packet { pts_us, ..p };
                if events.send(Ok(Some(p))).is_err() {
                    break;
                }
            }
            Kind::Control => debug!("slrw: control record ignored in raw mode"),
            k => {
                let _ = events.send(Err(invalid(format!(
                    "slrw: {k:?} record (encoded H.264) in raw mode; \
                     use --raw-video WxH@FPS:{}",
                    rawin::ENCODED_PIX_FMT
                ))));
                break;
            }
        }
    }
    // The encoder thread ends the stream once it has drained its queue.
}

/// Encoder thread: raw frame → I420 → scaled → H.264 access unit.
fn encode_loop(
    mut enc: h264enc::Encoder,
    fmt: PixelFormat,
    spec: RawVideoSpec,
    jobs: Receiver<Job>,
    events: SyncSender<io::Result<Option<Packet>>>,
    keyframe: Arc<AtomicBool>,
    counters: Arc<Counters>,
) {
    let (w, h) = (enc.config().width, enc.config().height);
    while let Ok(job) = jobs.recv() {
        let t0 = Instant::now();
        let r = convert::to_i420(
            fmt,
            spec.size.width,
            spec.size.height,
            &job.data,
            None,
            ColorSpec::default(),
        )
        .and_then(|f| {
            if (f.width, f.height) == (w, h) {
                Ok(f)
            } else {
                convert::scale(&f, w, h)
            }
        })
        .and_then(|f| enc.encode(&f, keyframe.swap(false, Ordering::AcqRel)));
        let au = match r {
            Ok(au) => au,
            Err(e) => {
                let _ = events.send(Err(invalid(format!("builtin encoder: {e}"))));
                return;
            }
        };
        counters.encoded.fetch_add(1, Ordering::Relaxed);
        counters
            .encode_us
            .fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
        let mut payload = Vec::with_capacity(au.data.len() + 1);
        payload.push(if au.keyframe { slrw::H264_KEYFRAME } else { 0 });
        payload.extend_from_slice(&au.data);
        let p = Packet {
            kind: Kind::H264,
            pts_us: job.pts_us,
            payload,
        };
        if events.send(Ok(Some(p))).is_err() {
            return;
        }
    }
    let _ = events.send(Ok(Some(Packet {
        kind: Kind::End,
        pts_us: 0,
        payload: Vec::new(),
    })));
    let _ = events.send(Ok(None));
}

impl Input for BuiltinInput {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn next_frame(&mut self) -> Result<Option<EncodedFrame>> {
        loop {
            if let Some(f) = self.st.pop() {
                return Ok(Some(f));
            }
            if self.st.ended() {
                return Ok(None);
            }
            if !self.pump()? {
                return Ok(None);
            }
        }
    }

    fn request_keyframe(&mut self) -> bool {
        let now = Instant::now();
        if self
            .last_request
            .is_none_or(|l| now.duration_since(l) >= KEYFRAME_REQUEST_INTERVAL)
        {
            self.last_request = Some(now);
            self.keyframe.store(true, Ordering::Release);
        }
        true
    }

    fn describe(&self) -> String {
        format!("{} (built-in H.264 encoder)", self.what)
    }

    fn finish(self: Box<Self>, _clean_eof: bool) -> Result<()> {
        let c = &self.counters;
        let encoded = c.encoded.load(Ordering::Relaxed);
        debug!(
            "builtin: {} frames in, {} encoded ({:.1} ms each), {} dropped for the frame rate, \
             {} while the encoder was busy",
            c.frames.load(Ordering::Relaxed),
            encoded,
            c.encode_us.load(Ordering::Relaxed) as f64 / encoded.max(1) as f64 / 1000.0,
            c.dropped_rate.load(Ordering::Relaxed),
            c.dropped_busy.load(Ordering::Relaxed),
        );
        // The reader may be blocked on the game's stdin; it ends with the
        // process (like the other SLRW paths).
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Test pattern

/// The pattern's audio: stereo s16le.
pub fn pattern_audio(sample_rate: u32) -> RawAudioSpec {
    RawAudioSpec {
        sample_rate,
        channels: 2,
        sample_fmt: "s16le".into(),
    }
}

/// Sample rate for the pattern: AirPlay receivers take 44.1 kHz.
pub fn pattern_sample_rate(airplay: bool) -> u32 {
    if airplay {
        44_100
    } else {
        48_000
    }
}

/// 5x7 glyphs, one byte per row, bit 4 = leftmost column.
fn glyph(c: char) -> [u8; 7] {
    match c {
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        '3' => [0x1e, 0x01, 0x01, 0x0e, 0x01, 0x01, 0x1e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        '6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        ':' => [0x00, 0x0c, 0x0c, 0x00, 0x0c, 0x0c, 0x00],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
        '#' => [0x0a, 0x0a, 0x1f, 0x0a, 0x1f, 0x0a, 0x0a],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        _ => [0; 7],
    }
}

/// Draws `text` with its top-left corner at (x, y), `scale` pixels per
/// font pixel, white on a black box one font pixel wider all around.
/// Clipped to the frame.
pub fn draw_text(f: &mut Yuv420Frame, x: usize, y: usize, scale: usize, text: &str) {
    let (w, h) = (f.width as usize, f.height as usize);
    let cols = text.chars().count() * 6;
    let (bx0, by0) = (x.saturating_sub(scale), y.saturating_sub(scale));
    let (bx1, by1) = ((x + (cols + 1) * scale).min(w), (y + 8 * scale).min(h));
    for yy in by0..by1 {
        f.y[yy * w + bx0..yy * w + bx1].fill(16);
    }
    let cw = f.chroma_width();
    for cy in by0 / 2..by1.div_ceil(2).min(f.chroma_height()) {
        let r = cy * cw;
        f.u[r + bx0 / 2..r + bx1.div_ceil(2).min(cw)].fill(128);
        f.v[r + bx0 / 2..r + bx1.div_ceil(2).min(cw)].fill(128);
    }
    for (i, c) in text.chars().enumerate() {
        let g = glyph(c);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..5 {
                if bits & (0x10 >> col) == 0 {
                    continue;
                }
                let px = x + (i * 6 + col) * scale;
                let py = y + row * scale;
                for yy in py..(py + scale).min(h) {
                    if px < w {
                        f.y[yy * w + px..yy * w + (px + scale).min(w)].fill(235);
                    }
                }
            }
        }
    }
}

/// `HH:MM:SS.mmm` (UTC) for a time since the Unix epoch.
pub fn clock_text(since_epoch: Duration) -> String {
    let ms = since_epoch.as_millis() % 86_400_000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// Generates the pattern, paced in real time.
pub struct TestPattern {
    size: Size,
    fps: Ratio,
    rate: u32,
    /// Per bar: (Y, U, V).
    bars: [(u8, u8, u8); 7],
    start: Option<Instant>,
    frames: u64,
    samples: u64,
    phase: f64,
    stop: StopFn,
}

/// Samples per audio record (10 ms).
fn audio_chunk(rate: u32) -> u64 {
    (rate / 100).max(1) as u64
}

impl TestPattern {
    pub fn new(size: Size, fps: Ratio, rate: u32, stop: StopFn) -> Result<TestPattern> {
        if fps.num == 0 || fps.den == 0 || rate == 0 {
            return Err(Error::usage("test pattern: invalid frame or sample rate"));
        }
        // 75% bars (BT.709 limited range, via the converter).
        const RGB: [[u8; 3]; 7] = [
            [191, 191, 191],
            [191, 191, 0],
            [0, 191, 191],
            [0, 191, 0],
            [191, 0, 191],
            [191, 0, 0],
            [0, 0, 191],
        ];
        let mut bars = [(0, 0, 0); 7];
        for (b, rgb) in bars.iter_mut().zip(RGB) {
            let px = [rgb[0], rgb[1], rgb[2]].repeat(4);
            let f = convert::to_i420(PixelFormat::Rgb24, 2, 2, &px, None, ColorSpec::default())?;
            *b = (f.y[0], f.u[0], f.v[0]);
        }
        Ok(TestPattern {
            size,
            fps,
            rate,
            bars,
            start: None,
            frames: 0,
            samples: 0,
            phase: 0.0,
            stop,
        })
    }

    fn video_due_us(&self) -> u64 {
        (self.frames as u128 * 1_000_000 * self.fps.den as u128 / self.fps.num as u128) as u64
    }

    fn audio_due_us(&self) -> u64 {
        (self.samples as u128 * 1_000_000 / self.rate as u128) as u64
    }

    /// Frame `n` at media time `t_us`, stamped with `wall`.
    pub fn frame(&self, n: u64, t_us: u64, wall: Duration) -> Yuv420Frame {
        let (w, h) = (self.size.width as usize, self.size.height as usize);
        let mut f = Yuv420Frame::new(self.size.width, self.size.height);
        let (cw, ch) = (f.chroma_width(), f.chroma_height());
        let bars_h = h * 3 / 5;
        let shift = (n as usize * 4) % w.max(1);
        let bar_of = |x: usize| ((x + shift) % w * 7 / w).min(6);
        for y in 0..bars_h {
            let row = &mut f.y[y * w..(y + 1) * w];
            for (x, p) in row.iter_mut().enumerate() {
                *p = self.bars[bar_of(x)].0;
            }
        }
        for cy in 0..(bars_h / 2).min(ch) {
            for cx in 0..cw {
                let b = self.bars[bar_of(cx * 2)];
                f.u[cy * cw + cx] = b.1;
                f.v[cy * cw + cx] = b.2;
            }
        }
        // A box moving along the bottom edge (motion is easy to judge).
        let bs = (h / 12).max(2);
        let period = (w.saturating_sub(bs)).max(1);
        let bx = (n as usize * 8) % period;
        for y in h.saturating_sub(bs)..h {
            f.y[y * w + bx..y * w + (bx + bs).min(w)].fill(235);
        }
        // White flash with the beep: the first 100 ms of every second.
        if t_us % 1_000_000 < 100_000 {
            let fs = (h / 6).max(2).min(w);
            for y in bars_h..(bars_h + fs).min(h) {
                f.y[y * w + w - fs..(y + 1) * w].fill(235);
            }
        }
        let scale = (w * 9 / 10 / (13 * 6)).max(1);
        let ty = bars_h + scale * 2;
        draw_text(&mut f, scale * 2, ty, scale, &clock_text(wall));
        let small = (scale / 2).max(1);
        let ms = t_us / 1000;
        let line = format!("UTC  #{n}  {}.{:03}", ms / 1000, ms % 1000);
        draw_text(&mut f, scale * 2, ty + scale * 10, small, &line);
        f
    }

    fn audio(&mut self, n: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(n as usize * 4);
        let step = 2.0 * std::f64::consts::PI * 1000.0 / self.rate as f64;
        for i in 0..n {
            let s = self.samples + i;
            let in_beep = (s % self.rate as u64) < self.rate as u64 / 10;
            let v = if in_beep {
                (self.phase.sin() * 8000.0) as i16
            } else {
                0
            };
            self.phase = (self.phase + step) % (2.0 * std::f64::consts::PI);
            out.extend_from_slice(&v.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
}

impl PacketSource for TestPattern {
    fn next_packet(&mut self) -> io::Result<Option<Packet>> {
        let start = *self.start.get_or_insert_with(Instant::now);
        let (v, a) = (self.video_due_us(), self.audio_due_us());
        let due = v.min(a);
        loop {
            if (self.stop)() {
                return Ok(None);
            }
            let at = start + Duration::from_micros(due);
            let now = Instant::now();
            if now >= at {
                break;
            }
            std::thread::sleep((at - now).min(STOP_POLL));
        }
        if a <= v {
            let n = audio_chunk(self.rate);
            let payload = self.audio(n);
            self.samples += n;
            return Ok(Some(Packet {
                kind: Kind::Audio,
                pts_us: a,
                payload,
            }));
        }
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let f = self.frame(self.frames, v, wall);
        self.frames += 1;
        Ok(Some(Packet {
            kind: Kind::Video,
            pts_us: v,
            payload: f.to_i420(),
        }))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::types::{AudioCodec, TrackKind};

    fn never() -> bool {
        false
    }

    #[test]
    fn clock_and_font() {
        assert_eq!(
            clock_text(Duration::from_millis(86_400_000 + 3_723_456)),
            "01:02:03.456"
        );
        let mut f = Yuv420Frame::new(64, 20);
        draw_text(&mut f, 2, 2, 1, "0:9");
        // The leftmost column of the "0" (rows 1..5 lit).
        assert_eq!(f.y[3 * 64 + 2], 235);
        assert_eq!(f.y[2 * 64 + 2], 16);
        // Clipping never panics.
        draw_text(&mut f, 60, 15, 3, "12:34");
        draw_text(&mut f, 100, 100, 2, "1");
    }

    pub(crate) fn rgba_bars(w: u32, h: u32, n: u64) -> Vec<u8> {
        let mut buf = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as u64 {
            for x in 0..w as u64 {
                let bar = ((x + n * 4) % w as u64 * 7 / w as u64) as u8;
                let i = ((y * w as u64 + x) * 4) as usize;
                buf[i..i + 4].copy_from_slice(&[bar * 36, 255 - bar * 30, (y as u8) ^ bar, 255]);
            }
        }
        buf
    }

    /// Hands out byte chunks at their due times, like a game.
    pub(crate) struct Paced {
        pub(crate) chunks: std::collections::VecDeque<(Duration, Vec<u8>)>,
        pub(crate) start: Option<Instant>,
        pub(crate) cur: Vec<u8>,
        pub(crate) pos: usize,
    }

    impl Paced {
        pub(crate) fn new(chunks: std::collections::VecDeque<(Duration, Vec<u8>)>) -> Paced {
            Paced {
                chunks,
                start: None,
                cur: Vec::new(),
                pos: 0,
            }
        }
    }

    impl Read for Paced {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
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

    /// Raw RGBA at 60 fps (paced), with 800 samples of PCM per frame.
    pub(crate) fn raw_stream(frames: u64, w: u32, h: u32) -> Paced {
        let mut chunks = std::collections::VecDeque::new();
        let mut head = Vec::new();
        slrw::write_magic(&mut head).unwrap();
        chunks.push_back((Duration::ZERO, head));
        for i in 0..frames {
            let pts = 5_000_000 + i * 16_667;
            let mut s = Vec::new();
            slrw::write_packet(&mut s, Kind::Video, pts, &rgba_bars(w, h, i)).unwrap();
            slrw::write_packet(&mut s, Kind::Audio, pts, &[1u8; 800 * 4]).unwrap();
            chunks.push_back((Duration::from_micros(i * 16_667), s));
        }
        let mut end = Vec::new();
        slrw::write_packet(&mut end, Kind::End, 0, &[]).unwrap();
        chunks.push_back((Duration::from_micros(frames * 16_667), end));
        Paced::new(chunks)
    }

    #[test]
    fn raw_slrw_encodes() {
        let (w, h) = (100, 60);
        let opts = BuiltinOptions {
            video_in: RawVideoSpec {
                size: Size {
                    width: w,
                    height: h,
                },
                fps: Ratio::integer(60),
                pix_fmt: "rgba".into(),
            },
            video_out: VideoOut {
                size: Size {
                    width: 64,
                    height: 38,
                },
                fps: Ratio::integer(30),
                bitrate_kbps: 500,
                gop: 15,
            },
            audio: Some(RawAudioSpec {
                sample_rate: 48_000,
                channels: 2,
                sample_fmt: "s16le".into(),
            }),
            audio_families: vec![
                (AudioFamily::AirPlay, AudioOut::Pcm { frame_samples: 352 }),
                (
                    AudioFamily::Cast,
                    AudioOut::AacLc {
                        bitrate_bps: 128_000,
                    },
                ),
            ],
            threads: 1,
        };
        let mut input = BuiltinInput::open_slrw_with(raw_stream(30, w, h), opts, never).unwrap();
        let info = input.info().clone();
        assert_eq!((info.video.width, info.video.height), (64, 38));
        assert_eq!(info.video.fps, Ratio::integer(30));
        assert_eq!(
            info.audio_for(AudioFamily::Cast).unwrap().codec,
            AudioCodec::AacLc
        );
        let mut frames = Vec::new();
        while let Some(f) = input.next_frame().unwrap() {
            frames.push(f);
        }
        let video: Vec<_> = frames
            .iter()
            .filter(|f| f.track == TrackKind::Video)
            .collect();
        // 30 frames at 60 fps cut to 30 fps.
        assert!((13..=15).contains(&video.len()), "{}", video.len());
        assert!(video[0].keyframe && video[0].pts == 0);
        assert!(video.windows(2).all(|p| p[1].pts > p[0].pts));
        let pcm = frames
            .iter()
            .filter(|f| f.track == TrackKind::Audio(AudioFamily::AirPlay))
            .count();
        assert_eq!(pcm, 30 * 800 / 352);
        assert!(input.request_keyframe());
        Box::new(input).finish(true).unwrap();
    }

    #[test]
    fn rejects_bad_records() {
        let opts = |pix: &str| BuiltinOptions {
            video_in: RawVideoSpec {
                size: Size {
                    width: 16,
                    height: 16,
                },
                fps: Ratio::integer(30),
                pix_fmt: pix.into(),
            },
            video_out: VideoOut {
                size: Size {
                    width: 16,
                    height: 16,
                },
                fps: Ratio::integer(30),
                bitrate_kbps: 100,
                gop: 0,
            },
            audio: None,
            audio_families: vec![],
            threads: 1,
        };
        let e = BuiltinInput::open_slrw_with(io::Cursor::new(Vec::new()), opts("yuyv422"), never)
            .err()
            .unwrap();
        assert!(e.to_string().contains("cannot read"), "{e}");
        // A short frame.
        let mut s = Vec::new();
        slrw::write_magic(&mut s).unwrap();
        slrw::write_packet(&mut s, Kind::Video, 0, &[0; 100]).unwrap();
        assert!(BuiltinInput::open_slrw_with(io::Cursor::new(s), opts("rgba"), never).is_err());
        // Encoded records in raw mode.
        let mut s = Vec::new();
        slrw::write_magic(&mut s).unwrap();
        slrw::write_h264(&mut s, 0, true, false, &[0, 0, 0, 1, 0x65]).unwrap();
        let e = BuiltinInput::open_slrw_with(io::Cursor::new(s), opts("rgba"), never)
            .err()
            .unwrap();
        assert!(e.to_string().contains("raw mode"), "{e}");
        // Ends before any frame.
        let mut s = Vec::new();
        slrw::write_magic(&mut s).unwrap();
        assert!(BuiltinInput::open_slrw_with(io::Cursor::new(s), opts("nv12"), never).is_err());
    }

    #[test]
    fn test_pattern_runs_in_real_time() {
        use std::sync::atomic::AtomicUsize;
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn soon() -> bool {
            CALLS.load(Ordering::SeqCst) > 0
        }
        let out = VideoOut {
            size: Size {
                width: 160,
                height: 90,
            },
            fps: Ratio::integer(30),
            bitrate_kbps: 500,
            gop: 30,
        };
        let fams = vec![(AudioFamily::AirPlay, AudioOut::Pcm { frame_samples: 352 })];
        let mut input = BuiltinInput::test_pattern(out, 44_100, fams, soon).unwrap();
        let t0 = Instant::now();
        let mut video = 0;
        let mut audio = 0;
        while t0.elapsed() < Duration::from_millis(500) {
            match input.next_frame().unwrap() {
                Some(f) if f.track == TrackKind::Video => video += 1,
                Some(_) => audio += 1,
                None => break,
            }
        }
        CALLS.store(1, Ordering::SeqCst);
        assert!((12..=17).contains(&video), "{video}");
        assert!(audio > 40, "{audio}");
        assert!(input.next_frame().unwrap().is_none() || soon());
        Box::new(input).finish(false).unwrap();
    }

    #[test]
    fn pattern_frames_and_beep() {
        let mut p = TestPattern::new(
            Size {
                width: 320,
                height: 180,
            },
            Ratio::integer(30),
            48_000,
            never,
        )
        .unwrap();
        let f = p.frame(3, 50_000, Duration::from_millis(1234));
        assert_eq!(f.y.len(), 320 * 180);
        // Beep in the first 100 ms, silence after.
        let a = p.audio(480);
        assert!(a.chunks_exact(4).any(|s| s[0] != 0 || s[1] != 0));
        p.samples = 4800;
        let a = p.audio(480);
        assert!(a.iter().all(|&b| b == 0));
    }
}

#[cfg(test)]
mod preview {
    use super::*;

    /// Writes one pattern frame as raw I420 to `$STREAMLIB_PATTERN_OUT`.
    #[test]
    #[ignore = "writes a preview frame"]
    fn pattern_preview() {
        let out = std::env::var("STREAMLIB_PATTERN_OUT").expect("STREAMLIB_PATTERN_OUT");
        let p = TestPattern::new(
            Size {
                width: 1280,
                height: 720,
            },
            Ratio::integer(30),
            48_000,
            || false,
        )
        .unwrap();
        let wall = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        std::fs::write(out, p.frame(42, 1_040_000, wall).to_i420()).unwrap();
    }
}
