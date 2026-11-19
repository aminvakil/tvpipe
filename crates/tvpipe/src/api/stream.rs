//! [`Stream`]: a program's own frames and sound, in process.
//!
//! The writers turn each call into an SLRW record (docs/slrw.md) on an
//! in-memory pipe that the session reads as its game input: raw frames go
//! to the built-in H.264 encoder (keyframes on demand), the program's own
//! H.264 is passed through. Writing never blocks: a frame the encoder
//! has no room for is dropped and reported as [`Written::Dropped`].

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::options::{Options, VideoSpec};
use super::session::{ReceiverInfo, Session, SessionState, Stopper};
use crate::error::{Error, Result};
use crate::events::{EndInfo, Event, Started, Stats};
use crate::media::slrw::{self, Kind};
use crate::opts::{Command, InputSpec};
use crate::session::GameIo;

/// Raw frames (or access units) waiting for the session; more are
/// dropped. The encoder keeps a queue of its own.
const VIDEO_QUEUE: usize = 2;
/// Access units of the program's own H.264 waiting; more are dropped.
const H264_QUEUE: usize = 8;
/// PCM waiting, in bytes; more is dropped.
const AUDIO_QUEUE_BYTES: usize = 4 << 20;

/// What became of a frame.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Written {
    /// It goes to the receivers.
    Queued,
    /// It was dropped: the receivers are not connected yet, the encoder
    /// is busy, or it came sooner than the stream's frame rate.
    Dropped,
}

/// A raw frame whose rows are `stride` bytes apart (a GPU readback's
/// padded rows, a sub-rectangle of a larger buffer), optionally stored
/// bottom row first (OpenGL's `glReadPixels`).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Frame<'a> {
    /// The first plane, `stride * height` bytes (the last row may be
    /// shorter).
    pub data: &'a [u8],
    /// Bytes from one row to the next.
    pub stride: usize,
    /// Rows are stored bottom to top.
    pub bottom_up: bool,
    /// The other planes of NV12 (UV) and YUV 4:2:0 (U, V) with their
    /// strides.
    pub planes: [Option<(&'a [u8], usize)>; 2],
}

impl<'a> Frame<'a> {
    /// Packed or padded rows, top to bottom.
    pub fn new(data: &'a [u8], stride: usize) -> Frame<'a> {
        Frame {
            data,
            stride,
            bottom_up: false,
            planes: [None, None],
        }
    }

    /// Rows bottom to top.
    pub fn bottom_up(mut self, bottom_up: bool) -> Frame<'a> {
        self.bottom_up = bottom_up;
        self
    }

    /// A planar frame: Y, then UV (NV12) or U and V (YUV 4:2:0), each with
    /// its stride.
    pub fn planes(planes: &[(&'a [u8], usize)]) -> Frame<'a> {
        let (data, stride) = planes.first().copied().unwrap_or((&[], 0));
        Frame {
            data,
            stride,
            bottom_up: false,
            planes: [planes.get(1).copied(), planes.get(2).copied()],
        }
    }
}

/// How [`Stream::write_h264_config`] describes the decoder configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigFormat {
    /// An `avcC` record (AVCDecoderConfigurationRecord).
    Avcc,
    /// Annex B NAL units with at least one SPS and one PPS.
    AnnexB,
}

/// One access unit of the program's own H.264.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct H264 {
    /// It is an IDR picture (a hint; tvpipe checks the NAL units too).
    pub keyframe: bool,
    /// Start codes (Annex B); otherwise 4-byte big-endian lengths (AVCC).
    pub annex_b: bool,
    /// Presentation time; must increase (no B-frames).
    pub pts: Duration,
}

impl H264 {
    /// An access unit shown at `pts`.
    pub fn new(pts: Duration, keyframe: bool, annex_b: bool) -> H264 {
        H264 {
            keyframe,
            annex_b,
            pts,
        }
    }
}

// ---------------------------------------------------------------------------
// The pipe

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChunkKind {
    Video,
    Audio,
    Other,
}

#[derive(Default)]
struct PipeState {
    chunks: VecDeque<(ChunkKind, Vec<u8>)>,
    /// Read position in the front chunk.
    pos: usize,
    video: usize,
    audio_bytes: usize,
    closed: bool,
    /// Video records the reader is done with, for the next frames.
    pool: Vec<Vec<u8>>,
}

/// Frame buffers kept for reuse.
const POOL: usize = VIDEO_QUEUE + 2;

/// SLRW bytes from the writers to the session's reader.
struct Pipe {
    state: Mutex<PipeState>,
    cv: Condvar,
}

impl Pipe {
    fn lock(&self) -> MutexGuard<'_, PipeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push(&self, kind: ChunkKind, record: Vec<u8>) {
        let mut st = self.lock();
        if st.closed {
            return;
        }
        match kind {
            ChunkKind::Video => st.video += 1,
            ChunkKind::Audio => st.audio_bytes += record.len(),
            ChunkKind::Other => {}
        }
        st.chunks.push_back((kind, record));
        drop(st);
        self.cv.notify_all();
    }

    fn close(&self) {
        self.lock().closed = true;
        self.cv.notify_all();
    }
}

struct PipeReader(Arc<Pipe>);

impl Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut st = self.0.lock();
        loop {
            if let Some((kind, chunk)) = st.chunks.front() {
                let (kind, len) = (*kind, chunk.len());
                let n = buf.len().min(len - st.pos);
                buf[..n].copy_from_slice(&chunk[st.pos..st.pos + n]);
                st.pos += n;
                if st.pos == len {
                    let done = st.chunks.pop_front();
                    st.pos = 0;
                    if let Some((ChunkKind::Video, v)) = done {
                        if st.pool.len() < POOL {
                            st.pool.push(v);
                        }
                    }
                    match kind {
                        ChunkKind::Video => st.video -= 1,
                        ChunkKind::Audio => st.audio_bytes -= len,
                        ChunkKind::Other => {}
                    }
                }
                return Ok(n);
            }
            if st.closed {
                return Ok(0);
            }
            st = self.0.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
    }
}

pub(crate) fn record(kind: Kind, pts: Duration, payload_len: usize) -> Vec<u8> {
    let mut r = Vec::with_capacity(13 + payload_len);
    r.extend_from_slice(&header(kind, pts, payload_len));
    r
}

fn header(kind: Kind, pts: Duration, payload_len: usize) -> [u8; 13] {
    let mut h = [0u8; 13];
    h[0] = kind.code();
    h[1..9].copy_from_slice(&(pts.as_micros().min(u64::MAX as u128) as u64).to_le_bytes());
    h[9..13].copy_from_slice(&(payload_len as u32).to_le_bytes());
    h
}

impl Pipe {
    /// A video record of `payload_len` bytes (zeroed header), from the
    /// pool when one is free.
    fn video_buffer(&self, payload_len: usize) -> Vec<u8> {
        let mut v = self.lock().pool.pop().unwrap_or_default();
        v.clear();
        v.resize(13 + payload_len, 0);
        v
    }

    fn recycle(&self, v: Vec<u8>) {
        let mut st = self.lock();
        if st.pool.len() < POOL {
            st.pool.push(v);
        }
    }
}

/// A raw frame to fill in place (a GPU readback writing straight into
/// it skips a copy): from [`Stream::frame_buffer`], queued with
/// [`Stream::submit`]. Derefs to the frame's packed bytes.
pub struct FrameBuffer {
    record: Vec<u8>,
}

impl std::fmt::Debug for FrameBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameBuffer")
            .field("len", &(self.record.len() - 13))
            .finish()
    }
}

impl std::ops::Deref for FrameBuffer {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.record[13..]
    }
}

impl std::ops::DerefMut for FrameBuffer {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.record[13..]
    }
}

/// The keyframe requests the encoded input writes (`keyframe\n` lines).
struct KeyframeFlag(Arc<AtomicBool>);

impl Write for KeyframeFlag {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.contains(&b'\n') {
            self.0.store(true, Ordering::Release);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The stream

/// A live stream of the program's own frames and sound to the receivers
/// of its [`Options`]. Dropping it stops it: every receiver gets its
/// teardown before `drop` returns.
///
/// ```no_run
/// use std::time::Duration;
/// use tvpipe::{Options, PixelFormat};
///
/// # fn main() -> tvpipe::Result<()> {
/// let mut s = tvpipe::stream(
///     Options::new().to("browser").video(640, 360, 30, PixelFormat::Rgba),
/// )?;
/// let frame = vec![0u8; 640 * 360 * 4];
/// for i in 0..300u64 {
///     let _ = s.write_video(&frame, Duration::from_millis(i * 1000 / 30))?;
///     std::thread::sleep(Duration::from_millis(33));
/// }
/// s.finish()?;
/// # Ok(())
/// # }
/// ```
pub struct Stream {
    session: Option<Session>,
    pipe: Arc<Pipe>,
    keyframe: Arc<AtomicBool>,
    video: VideoSpec,
    audio: Option<(u32, u8)>,
    frame_size: usize,
    min_gap: Duration,
    last_video: Option<Duration>,
    /// The program's H.264: drop until the next IDR.
    need_idr: bool,
    t0: Instant,
    audio_clock: Arc<AudioClock>,
    scratch: Vec<u8>,
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stream")
            .field("video", &self.video)
            .field("audio", &self.audio)
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

/// Where [`AudioWriter`]s are on the stream clock.
struct AudioClock {
    t0: Instant,
    /// Media time of the first sample, in µs past `t0` (+1; 0: not yet).
    origin_us: AtomicU64,
    samples: AtomicU64,
}

impl Stream {
    /// Starts streaming and returns at once (state
    /// [`SessionState::Connecting`]); frames written before the receivers
    /// are connected are dropped. [`stream`](crate::stream) waits for
    /// them instead.
    pub fn start(options: Options) -> Result<Stream> {
        let video = options.video.ok_or_else(|| {
            Error::usage("a stream needs Options::video (raw frames) or Options::h264")
        })?;
        if video.width == 0 || video.height == 0 || video.fps == 0 {
            return Err(Error::usage(
                "the video size and frame rate must be positive",
            ));
        }
        if let Some((rate, ch)) = options.audio {
            if rate == 0 || ch == 0 {
                return Err(Error::usage("the audio rate and channels must be positive"));
            }
        }
        let pipe = Arc::new(Pipe {
            state: Mutex::new(PipeState::default()),
            cv: Condvar::new(),
        });
        let mut magic = slrw::MAGIC.to_vec();
        if video.format.is_none() {
            // Keyframe requests on: they become take_keyframe_request().
            let mut r = record(Kind::Control, Duration::ZERO, 4);
            r.extend_from_slice(&slrw::CONTROL_KEYFRAME_REQUESTS.to_le_bytes());
            magic.extend_from_slice(&r);
        }
        pipe.push(ChunkKind::Other, magic);
        let keyframe = Arc::new(AtomicBool::new(false));
        let mut opts = options.play_options(InputSpec::Stdin);
        if video.format.is_some() {
            // In process, the encoder is ours: keyframes on demand.
            opts.encoder = crate::types::Encoder::Builtin;
        }
        opts.threads = options.encoder_threads;
        let io = GameIo {
            stdin: Box::new(PipeReader(pipe.clone())),
            stdout: Box::new(KeyframeFlag(keyframe.clone())),
            approve: Box::new(|_, _| false),
        };
        let session = Session::spawn(
            Command::Play(opts),
            options.ctx(),
            io,
            super::callback(&options),
        )?;
        let t0 = Instant::now();
        Ok(Stream {
            session: Some(session),
            pipe,
            keyframe,
            frame_size: video
                .format
                .map_or(0, |f| f.frame_size(video.width, video.height)),
            min_gap: Duration::from_micros(850_000 / u64::from(video.fps)),
            video,
            audio: options.audio,
            last_video: None,
            need_idr: true,
            t0,
            audio_clock: Arc::new(AudioClock {
                t0,
                origin_us: AtomicU64::new(0),
                samples: AtomicU64::new(0),
            }),
            scratch: Vec::new(),
        })
    }

    fn session(&self) -> &Session {
        self.session.as_ref().expect("the stream's session")
    }

    /// Whether the receivers are connected (the input opens).
    fn open(&self) -> bool {
        self.session().state_handle().connected()
    }

    /// Time since the stream started: a clock for programs without one.
    pub fn elapsed(&self) -> Duration {
        self.t0.elapsed()
    }

    /// Whether a frame written now would be encoded: skip the GPU readback
    /// when it would not.
    pub fn wants_frame(&self) -> bool {
        self.video.format.is_some()
            && self.open()
            && self.pipe.lock().video < VIDEO_QUEUE
            && self
                .last_video
                .is_none_or(|l| self.elapsed() >= l + self.min_gap)
    }

    /// One raw frame in the stream's size and pixel format, rows packed.
    pub fn write_video(&mut self, frame: &[u8], pts: Duration) -> Result<Written> {
        let Some(format) = self.video.format else {
            return Err(Error::usage("this stream takes H.264 (write_h264)"));
        };
        if frame.len() != self.frame_size {
            return Err(Error::usage(format!(
                "a {}x{} {} frame is {} bytes, not {}",
                self.video.width,
                self.video.height,
                format.as_str(),
                self.frame_size,
                frame.len()
            )));
        }
        if !self.accept_video(pts) {
            return Ok(Written::Dropped);
        }
        let mut r = self.pipe.video_buffer(frame.len());
        r[..13].copy_from_slice(&header(Kind::Video, pts, frame.len()));
        r[13..].copy_from_slice(frame);
        self.pipe.push(ChunkKind::Video, r);
        self.last_video = Some(pts);
        Ok(Written::Queued)
    }

    /// One raw frame with padded rows, bottom-up rows or separate planes
    /// (see [`Frame`]); packed while copied.
    pub fn write_video_frame(&mut self, frame: Frame<'_>, pts: Duration) -> Result<Written> {
        let Some(format) = self.video.format else {
            return Err(Error::usage("this stream takes H.264 (write_h264)"));
        };
        if !self.accept_video(pts) {
            return Ok(Written::Dropped);
        }
        let mut r = record(Kind::Video, pts, self.frame_size);
        let planes = format.planes(self.video.width as usize, self.video.height as usize);
        let sources = [
            Some((frame.data, frame.stride)),
            frame.planes[0],
            frame.planes[1],
        ];
        for (i, &(row, rows)) in planes.iter().enumerate() {
            let (data, stride) = sources[i].ok_or_else(|| {
                Error::usage(format!(
                    "a {} frame has {} planes",
                    format.as_str(),
                    planes.len()
                ))
            })?;
            if stride < row || (rows > 0 && data.len() < stride * (rows - 1) + row) {
                return Err(Error::usage(format!(
                    "plane {i}: {} bytes with stride {stride} is too small for {rows} rows of \
                     {row} bytes",
                    data.len()
                )));
            }
            for y in 0..rows {
                let src = if frame.bottom_up { rows - 1 - y } else { y };
                r.extend_from_slice(&data[src * stride..src * stride + row]);
            }
        }
        self.pipe.push(ChunkKind::Video, r);
        self.last_video = Some(pts);
        Ok(Written::Queued)
    }

    /// A buffer for one packed raw frame (reused between frames) to fill
    /// and [`submit`](Stream::submit): no copy of the frame on the
    /// caller's thread.
    pub fn frame_buffer(&mut self) -> FrameBuffer {
        FrameBuffer {
            record: self.pipe.video_buffer(self.frame_size),
        }
    }

    /// Queues a filled [`FrameBuffer`] (or drops it, like
    /// [`write_video`](Stream::write_video); the buffer is reused).
    pub fn submit(&mut self, buffer: FrameBuffer, pts: Duration) -> Result<Written> {
        if self.video.format.is_none() {
            return Err(Error::usage("this stream takes H.264 (write_h264)"));
        }
        let mut r = buffer.record;
        if r.len() != 13 + self.frame_size {
            return Err(Error::usage("a frame buffer of another stream"));
        }
        if !self.accept_video(pts) {
            self.pipe.recycle(r);
            return Ok(Written::Dropped);
        }
        r[..13].copy_from_slice(&header(Kind::Video, pts, self.frame_size));
        self.pipe.push(ChunkKind::Video, r);
        self.last_video = Some(pts);
        Ok(Written::Queued)
    }

    /// One frame of `0x00RRGGBB` pixels (softbuffer, minifb): the
    /// stream's format must be [`PixelFormat::Bgr0`](super::PixelFormat::Bgr0) on little-endian
    /// machines.
    pub fn write_video_u32(&mut self, frame: &[u32], pts: Duration) -> Result<Written> {
        let mut bytes = std::mem::take(&mut self.scratch);
        bytes.clear();
        bytes.reserve(frame.len() * 4);
        for p in frame {
            bytes.extend_from_slice(&p.to_le_bytes());
        }
        let r = self.write_video(&bytes, pts);
        self.scratch = bytes;
        r
    }

    fn accept_video(&mut self, pts: Duration) -> bool {
        self.open()
            && self.pipe.lock().video < VIDEO_QUEUE
            && self
                .last_video
                .is_none_or(|l| pts >= l + self.min_gap || pts < l)
    }

    /// Interleaved 16-bit PCM in the stream's rate and channels.
    pub fn write_audio(&mut self, pcm: &[i16], pts: Duration) -> Result<()> {
        let Some((_, ch)) = self.audio else {
            return Err(Error::usage("this stream has no audio (Options::audio)"));
        };
        if pcm.len() % ch as usize != 0 {
            return Err(Error::usage("PCM must hold whole sample frames"));
        }
        push_audio(&self.pipe, self.open(), pcm.iter().copied(), pcm.len(), pts);
        Ok(())
    }

    /// Interleaved float PCM (-1.0..1.0) in the stream's rate and
    /// channels.
    pub fn write_audio_f32(&mut self, pcm: &[f32], pts: Duration) -> Result<()> {
        let Some((_, ch)) = self.audio else {
            return Err(Error::usage("this stream has no audio (Options::audio)"));
        };
        if pcm.len() % ch as usize != 0 {
            return Err(Error::usage("PCM must hold whole sample frames"));
        }
        push_audio(
            &self.pipe,
            self.open(),
            pcm.iter().map(|&s| f32_to_i16(s)),
            pcm.len(),
            pts,
        );
        Ok(())
    }

    /// A writer for an audio thread: timestamps by sample count from its
    /// first write, on the stream's clock.
    pub fn audio_writer(&self) -> AudioWriter {
        AudioWriter {
            pipe: self.pipe.clone(),
            clock: self.audio_clock.clone(),
            format: self.audio,
            session_open: self.session().state_handle(),
        }
    }

    /// The program's own H.264: one access unit. When the session has no
    /// room for it, it and every following non-IDR picture are dropped and
    /// a keyframe is requested ([`take_keyframe_request`]).
    ///
    /// [`take_keyframe_request`]: Stream::take_keyframe_request
    pub fn write_h264(&mut self, au: &[u8], h: H264) -> Result<Written> {
        if self.video.format.is_some() {
            return Err(Error::usage("this stream takes raw frames (write_video)"));
        }
        if !self.open() {
            self.need_idr = true;
            return Ok(Written::Dropped);
        }
        if self.need_idr && !h.keyframe {
            self.keyframe.store(true, Ordering::Release);
            return Ok(Written::Dropped);
        }
        if self.pipe.lock().video >= H264_QUEUE {
            self.need_idr = true;
            self.keyframe.store(true, Ordering::Release);
            return Ok(Written::Dropped);
        }
        self.need_idr = false;
        let mut r = record(Kind::H264, h.pts, au.len() + 1);
        r.push(
            if h.keyframe { slrw::H264_KEYFRAME } else { 0 }
                | if h.annex_b { slrw::H264_ANNEX_B } else { 0 },
        );
        r.extend_from_slice(au);
        self.pipe.push(ChunkKind::Video, r);
        Ok(Written::Queued)
    }

    /// The decoder configuration (SPS/PPS), when the access units do not
    /// carry it in band before each IDR.
    pub fn write_h264_config(&mut self, config: &[u8], format: ConfigFormat) -> Result<()> {
        if self.video.format.is_some() {
            return Err(Error::usage("this stream takes raw frames (write_video)"));
        }
        let mut r = record(Kind::H264Config, Duration::ZERO, config.len() + 1);
        r.push(match format {
            ConfigFormat::Avcc => slrw::CONFIG_AVCC,
            ConfigFormat::AnnexB => slrw::CONFIG_ANNEX_B,
        });
        r.extend_from_slice(config);
        self.pipe.push(ChunkKind::Other, r);
        Ok(())
    }

    /// Whether a receiver asked for a keyframe since the last call (the
    /// program's own H.264): make the next picture an IDR.
    pub fn take_keyframe_request(&self) -> bool {
        self.keyframe.swap(false, Ordering::AcqRel)
    }

    /// Where the session is.
    pub fn state(&self) -> SessionState {
        self.session().state()
    }

    /// The events since the last call (at most the latest 256).
    pub fn poll_events(&mut self) -> Vec<Event> {
        self.session
            .as_mut()
            .map(Session::poll_events)
            .unwrap_or_default()
    }

    /// Every receiver's latest state.
    pub fn receivers(&self) -> Vec<ReceiverInfo> {
        self.session().receivers()
    }

    /// The latest counters.
    pub fn stats(&self) -> Option<Stats> {
        self.session().stats()
    }

    /// The plan, once streaming started.
    pub fn started(&self) -> Option<Started> {
        self.session().started()
    }

    /// The `browser` receiver's page.
    pub fn player_url(&self) -> Option<String> {
        self.session().player_url()
    }

    /// A handle that stops the stream from another thread.
    pub fn stopper(&self) -> Stopper {
        self.session().stopper()
    }

    /// The last record queued (tests).
    #[cfg(test)]
    pub(crate) fn last_record(&self) -> Option<Vec<u8>> {
        self.pipe.lock().chunks.back().map(|c| c.1.clone())
    }

    pub(crate) fn wait_connected(&self) -> Result<()> {
        self.session().wait_connected()
    }

    /// Stops now: every receiver gets its TEARDOWN/STOP.
    pub fn stop(mut self) -> Result<EndInfo> {
        self.pipe.close();
        self.session.take().map_or_else(
            || Err(Error::closed("the stream was stopped")),
            Session::stop,
        )
    }

    /// Ends the stream: what is queued plays out, then teardown.
    pub fn finish(mut self) -> Result<EndInfo> {
        self.pipe
            .push(ChunkKind::Other, record(Kind::End, Duration::ZERO, 0));
        self.pipe.close();
        self.session.take().map_or_else(
            || Err(Error::closed("the stream was stopped")),
            Session::wait,
        )
    }

    /// Waits until the session ends by itself (every receiver left).
    pub fn wait(mut self) -> Result<EndInfo> {
        self.session.take().map_or_else(
            || Err(Error::closed("the stream was stopped")),
            Session::wait,
        )
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.pipe.close();
        if let Some(s) = self.session.take() {
            let _ = s.stop();
        }
    }
}

/// -1.0..1.0 to s16, as the Node SDK converts (`floatToS16`): negative
/// samples scale by 32768, positive ones by 32767.
pub(crate) fn f32_to_i16(s: f32) -> i16 {
    let s = if s.is_nan() { 0.0 } else { s.clamp(-1.0, 1.0) };
    (if s < 0.0 { s * 32768.0 } else { s * 32767.0 }).round() as i16
}

fn push_audio(
    pipe: &Pipe,
    open: bool,
    samples: impl Iterator<Item = i16>,
    n: usize,
    pts: Duration,
) {
    if !open || n == 0 {
        return;
    }
    if pipe.lock().audio_bytes + n * 2 > AUDIO_QUEUE_BYTES {
        trace!("stream: audio queue full; dropped {n} samples");
        return;
    }
    let mut r = record(Kind::Audio, pts, n * 2);
    for s in samples {
        r.extend_from_slice(&s.to_le_bytes());
    }
    pipe.push(ChunkKind::Audio, r);
}

/// Writes audio from another thread (a sound card callback): cloneable,
/// never blocks for long, timestamps by sample count from its first
/// write on the stream's clock.
#[derive(Clone)]
pub struct AudioWriter {
    pipe: Arc<Pipe>,
    clock: Arc<AudioClock>,
    format: Option<(u32, u8)>,
    session_open: super::session::StateHandle,
}

impl std::fmt::Debug for AudioWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioWriter")
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl AudioWriter {
    fn pts(&self, frames: u64) -> Option<Duration> {
        let (rate, _) = self.format?;
        let mut origin = self.clock.origin_us.load(Ordering::Acquire);
        if origin == 0 {
            let now = self.clock.t0.elapsed().as_micros() as u64 + 1;
            origin = match self.clock.origin_us.compare_exchange(
                0,
                now,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => now,
                Err(v) => v,
            };
        }
        let at = self.clock.samples.fetch_add(frames, Ordering::AcqRel);
        Some(Duration::from_micros(
            origin - 1 + at * 1_000_000 / u64::from(rate.max(1)),
        ))
    }

    /// Interleaved 16-bit PCM.
    pub fn write(&self, pcm: &[i16]) {
        let Some((_, ch)) = self.format else { return };
        let frames = (pcm.len() / ch as usize) as u64;
        if let Some(pts) = self.pts(frames) {
            let open = self.session_open.connected();
            push_audio(&self.pipe, open, pcm.iter().copied(), pcm.len(), pts);
        }
    }

    /// Interleaved float PCM (-1.0..1.0).
    pub fn write_f32(&self, pcm: &[f32]) {
        let Some((_, ch)) = self.format else { return };
        let frames = (pcm.len() / ch as usize) as u64;
        if let Some(pts) = self.pts(frames) {
            let open = self.session_open.connected();
            push_audio(
                &self.pipe,
                open,
                pcm.iter().map(|&s| f32_to_i16(s)),
                pcm.len(),
                pts,
            );
        }
    }
}
