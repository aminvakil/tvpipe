//! `MirrorSink`: the AirPlay screen-mirroring [`Sink`] (PLAN §3, §7.4).
//!
//! [`connect_mirror`] pair-verifies, runs FairPlay and the session setup
//! up to the audio SETUP ([`Session::establish`]), so the canvas and
//! audio format are known before ffmpeg starts. [`Sink::start`] runs the
//! video SETUP, connects the data channel and starts:
//!
//! - `airplay-video`: writes codec packets, encrypted frames and the 1 s
//!   heartbeat to the data connection (one writer, whole packets);
//! - `airplay-feedback`: `POST /feedback` every 2 s (re-anchors PTP);
//! - `airplay-data`: watches the data connection for the receiver ending
//!   the session;
//! - `airplay-rtcp`: answers retransmit requests on the audio control
//!   socket.
//!
//! [`Sink::push`] timestamps video and queues it for the writer; audio is
//! wrapped as verbatim ALAC, encrypted and sent at once (with a sync
//! packet first and then every second).
//!
//! Timing: a shared [`MediaClock`] maps pts to capture instants (anchored
//! on the first frame, re-anchored when frames are more than
//! [`MAX_LATENESS`] late or [`MAX_EARLY`] early); the session timeline
//! (PTP via response headers, or NTP) maps those to receiver time.
//!
//! Sources: docs/spec/airplay-mirroring.md §4.8, §4.11, §4.13, §6, §7.4,
//! §8.2–§8.8; PLAN §3, §5.3.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::airplay::audio::{
    alac_verbatim, sync_ntp, sync_ptp, AudioCipher, Packetizer, Redundancy, ALAC_FRAME_SAMPLES,
    SAMPLE_RATE,
};
use crate::airplay::keys;
use crate::airplay::mirror::{self, avcc_record, codec_header, frame_payload, VideoCipher};
use crate::airplay::pair::{self, SecureClient};
use crate::airplay::policy::{self, bit};
use crate::airplay::secure::SecureStream;
use crate::airplay::session::{
    lock_control, ControlStream, MirrorOptions, Session, SetupError, DEFAULT_TIMEOUT,
};
use crate::airplay::setup::AudioCodecId;
use crate::airplay::timing::{
    anchor_from_headers, fp_from_duration, fp_to_ns, ntp_clock, Timeline, NTP_EPOCH_OFFSET_FP,
};
use crate::error::{Error, ErrorKind, Result};
use crate::media::clock::MediaClock;
use crate::media::h264::{self, AvcConfig};
use crate::types::{
    AudioCodec, AudioFamily, AudioFormat, AudioInfo, Capabilities, EncodedFrame, Sink, SinkStatus,
    TrackKind, VideoCodec, VideoInfo, VIDEO_CLOCK_RATE,
};
use crate::util::ratio::Ratio;

/// Frames later than this re-anchor the media clock.
pub const MAX_LATENESS: Duration = Duration::from_millis(100);
/// Frames earlier than this re-anchor it too (a jump in the input).
pub const MAX_EARLY: Duration = Duration::from_secs(1);
pub const FEEDBACK_INTERVAL: Duration = Duration::from_secs(2);
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
pub const SYNC_INTERVAL: Duration = Duration::from_secs(1);
/// Write timeouts (§6.5).
const FRAME_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const CODEC_WRITE_TIMEOUT: Duration = Duration::from_secs(1);
const HEARTBEAT_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Video packets queued for the writer before frames are dropped.
const MAX_QUEUED: usize = 32;
const MAX_QUEUED_BYTES: usize = 8 << 20;
/// Consecutive stale audio frames at start-up before giving up (§8.8).
const MAX_STALE_AT_START: u32 = 128;
/// Burst pacing: packets per window (§8.4).
const BURST_PACKETS: u32 = 12;
const BURST_WINDOW: Duration = Duration::from_millis(5);
/// Bitrate ceiling the reference uses (§6.6).
const MAX_BITRATE_KBPS: u32 = 12_000;

/// The production stream type.
pub type SecureMirrorSink = MirrorSink<SecureStream<TcpStream>>;

/// Connects to an AirPlay receiver for mirroring: pair-verify (stored
/// credentials, else transient), FairPlay, session setup. Falls back from
/// PTP to NTP on a fresh connection when PTP fails (§2.5).
pub fn connect_mirror(target: &pair::Target, opts: &MirrorOptions) -> Result<SecureMirrorSink> {
    let store = crate::airplay::credentials::Store::open_default()?;
    let mut opts = opts.clone();
    loop {
        let control = pair::connect(target, &store)?;
        let secret = control.shared.secret().to_vec();
        let client: SecureClient = control.client;
        match Session::establish(client, control.addr, secret, true, &opts) {
            Ok(s) => return Ok(MirrorSink::new(target.label(), s, &opts)),
            Err(SetupError::PtpFailed(e)) if opts.ptp => {
                warn!("airplay: PTP setup failed ({e}); retrying with NTP");
                opts.ptp = false;
            }
            Err(e) => return Err(e.into_error()),
        }
    }
}

/// Maps pts to capture instants on the shared clock (like the Cast sink):
/// early frames keep their future instants, frames far off re-anchor.
fn capture_instant(clock: &MediaClock, pts: u64, rate: u32, now: Instant) -> Instant {
    let ok = |late: i128| {
        late <= MAX_LATENESS.as_nanos() as i128 && -late <= MAX_EARLY.as_nanos() as i128
    };
    match (clock.lateness(pts, rate, now), clock.instant_for(pts, rate)) {
        (Some(late), Some(t)) if ok(late) => t,
        (late, _) => {
            if let Some(late) = late {
                debug!(
                    "airplay: re-anchoring media clock ({} ms late)",
                    late / 1_000_000
                );
            }
            clock.reanchor_at(pts, rate, now);
            now
        }
    }
}

// ---------------------------------------------------------------------------
// Video writer

enum Item {
    Codec(Vec<u8>),
    Frame {
        payload: Vec<u8>,
        ts: u64,
        idr: bool,
    },
}

impl Item {
    fn len(&self) -> usize {
        match self {
            Item::Codec(p) => p.len(),
            Item::Frame { payload, .. } => payload.len(),
        }
    }
}

#[derive(Default)]
struct Queue {
    items: VecDeque<Item>,
    bytes: usize,
    closed: bool,
}

/// State shared between the sink and its threads.
struct Shared {
    queue: Mutex<Queue>,
    cond: Condvar,
    /// The receiver ended the session.
    finished: AtomicBool,
    /// A fatal error in a thread.
    failed: Mutex<Option<Error>>,
    /// Stopping: threads exit, errors are expected.
    stopping: AtomicBool,
    /// The first video frame went out (audio may start).
    first_frame_sent: AtomicBool,
    /// A frame was dropped; skip to the next keyframe.
    need_key: AtomicBool,
}

impl Shared {
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn fail(&self, e: Error) {
        if self.stopping.load(Ordering::Acquire) {
            debug!("airplay: after stop: {e}");
            return;
        }
        error!("airplay: {e}");
        let mut f = self.failed.lock().unwrap_or_else(|e| e.into_inner());
        f.get_or_insert(e);
    }

    fn take_failure(&self) -> Option<Error> {
        self.failed.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

fn write_packet(sock: &mut TcpStream, bytes: &[u8], timeout: Duration) -> Result<()> {
    let _ = sock.set_write_timeout(Some(timeout));
    sock.write_all(bytes)
        .map_err(|e| Error::io("airplay: video data write", e))
}

enum Next {
    Item(Item),
    Heartbeat,
    Done,
}

fn writer_loop(shared: &Shared, mut sock: TcpStream, mut cipher: VideoCipher, timeline: Timeline) {
    let mut next_heartbeat: Option<Instant> = None;
    let mut frames = 0u64;
    loop {
        let next = {
            let mut q = shared.queue();
            loop {
                if let Some(it) = q.items.pop_front() {
                    q.bytes -= it.len();
                    break Next::Item(it);
                }
                if q.closed {
                    break Next::Done;
                }
                let now = Instant::now();
                match next_heartbeat {
                    Some(at) if now >= at => break Next::Heartbeat,
                    Some(at) => {
                        q = shared
                            .cond
                            .wait_timeout(q, at - now)
                            .map(|(g, _)| g)
                            .unwrap_or_else(|e| e.into_inner().0);
                    }
                    None => q = shared.cond.wait(q).unwrap_or_else(|e| e.into_inner()),
                }
            }
        };
        let r = match next {
            Next::Done => return,
            Next::Heartbeat => {
                next_heartbeat = Some(Instant::now() + HEARTBEAT_INTERVAL);
                trace!("airplay: heartbeat");
                write_packet(&mut sock, &mirror::heartbeat(), HEARTBEAT_WRITE_TIMEOUT)
            }
            Next::Item(Item::Codec(p)) => {
                debug!("airplay: codec packet ({} bytes)", p.len());
                write_packet(&mut sock, &p, CODEC_WRITE_TIMEOUT)
            }
            Next::Item(Item::Frame { payload, ts, idr }) => cipher
                .frame_packet(payload, ts, idr, timeline.timeline_id())
                .and_then(|pkt| {
                    write_packet(&mut sock, &pkt, FRAME_WRITE_TIMEOUT)?;
                    frames += 1;
                    if frames == 1 {
                        debug!("airplay: first video frame sent ({} bytes)", pkt.len());
                        shared.first_frame_sent.store(true, Ordering::Release);
                        next_heartbeat = Some(Instant::now() + HEARTBEAT_INTERVAL);
                    }
                    Ok(())
                }),
        };
        if let Err(e) = r {
            shared.fail(e);
            let mut q = shared.queue();
            q.items.clear();
            q.bytes = 0;
            q.closed = true;
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Video and audio state on the push side

struct VideoState {
    length_size: usize,
    sps: Vec<u8>,
    pps: Vec<u8>,
    width: u32,
    height: u32,
    need_codec: bool,
    last_ts: Option<u64>,
    lead: u64,
}

impl VideoState {
    fn new(video: &VideoInfo, lead_ms: u32) -> Result<VideoState> {
        if video.codec != VideoCodec::H264 {
            return Err(Error::unsupported(
                "airplay: only H.264 mirroring is implemented",
            ));
        }
        let cfg = AvcConfig::parse(&video.avcc)?;
        let sps = cfg
            .sps
            .first()
            .cloned()
            .ok_or_else(|| Error::malformed("airplay: avcC without SPS"))?;
        let pps = cfg
            .pps
            .first()
            .cloned()
            .ok_or_else(|| Error::malformed("airplay: avcC without PPS"))?;
        let mut v = VideoState {
            length_size: cfg.length_size as usize,
            sps: Vec::new(),
            pps,
            width: video.width,
            height: video.height,
            need_codec: true,
            last_ts: None,
            lead: fp_from_duration(Duration::from_millis(lead_ms as u64)),
        };
        v.set_sps(sps);
        Ok(v)
    }

    fn set_sps(&mut self, sps: Vec<u8>) {
        if let Ok(s) = h264::parse_sps(&sps) {
            if s.width > 0 && s.height > 0 {
                self.width = s.width;
                self.height = s.height;
            }
        }
        self.sps = sps;
    }

    /// §6.3: strictly increasing timestamps.
    fn timestamp(&mut self, t: u64) -> u64 {
        let ts = match self.last_ts {
            Some(prev) if t <= prev => prev + 1,
            _ => t,
        };
        self.last_ts = Some(ts);
        ts
    }
}

struct AudioState {
    channels: u8,
    packetizer: Arc<Mutex<Packetizer>>,
    data: UdpSocket,
    control: UdpSocket,
    rtcp: SocketAddr,
    lead: Duration,
    lead_samples: u32,
    started: bool,
    stale: u32,
    /// `rtp = pts + offset` (mod 2³²).
    offset: u32,
    last_pts: Option<u64>,
    last_sync: Option<Instant>,
    first_sync: bool,
    window: (Instant, u32),
    frames: u64,
}

fn lock_pk(p: &Mutex<Packetizer>) -> MutexGuard<'_, Packetizer> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

impl AudioState {
    fn pace(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.window.0) >= BURST_WINDOW {
            self.window = (now, 0);
        }
        if self.window.1 >= BURST_PACKETS {
            let until = self.window.0 + BURST_WINDOW;
            if let Some(d) = until.checked_duration_since(now) {
                std::thread::sleep(d);
            }
            self.window = (Instant::now(), 0);
        }
        self.window.1 += 1;
    }

    fn send_sync(&mut self, timeline: &Timeline, rtp_now: u32, t: Instant) {
        let first = self.first_sync;
        self.first_sync = false;
        let play = rtp_now.wrapping_sub(self.lead_samples);
        let pkt = match timeline {
            Timeline::Ptp(c) => sync_ptp(first, play, fp_to_ns(c.at(t)), c.timeline()).to_vec(),
            Timeline::Ntp => sync_ntp(
                first,
                play,
                ntp_clock(t).wrapping_add(NTP_EPOCH_OFFSET_FP),
                rtp_now,
            )
            .to_vec(),
        };
        if first {
            debug!("airplay: first audio sync (rtp {rtp_now}, play {play})");
        }
        if let Err(e) = self.control.send_to(&pkt, self.rtcp) {
            debug!("airplay: sync packet: {e}");
        }
        self.last_sync = Some(Instant::now());
    }

    fn push(
        &mut self,
        frame: &EncodedFrame,
        clock: &MediaClock,
        timeline: &Timeline,
    ) -> Result<SinkStatus> {
        let now = Instant::now();
        let capture = capture_instant(clock, frame.pts, SAMPLE_RATE, now);
        if !self.started {
            // §8.8: drop frames that would arrive too late.
            if capture + self.lead <= now + Duration::from_millis(5) {
                self.stale += 1;
                if self.stale >= MAX_STALE_AT_START {
                    return Err(Error::protocol(
                        "airplay: audio frames keep arriving too late to play",
                    ));
                }
                return Ok(SinkStatus::Dropped);
            }
            self.started = true;
            self.offset = ALAC_FRAME_SAMPLES.wrapping_sub(frame.pts as u32);
        }
        let samples = (frame.data.len() / (2 * self.channels as usize)) as u32;
        if let Some(last) = self.last_pts {
            if frame.pts < last {
                // A new segment: continue after the last frame (§8.5).
                let last_rtp = (last as u32).wrapping_add(self.offset);
                self.offset = last_rtp
                    .wrapping_add(ALAC_FRAME_SAMPLES)
                    .wrapping_sub(frame.pts as u32);
                self.first_sync = true;
                self.last_sync = None;
            }
        }
        self.last_pts = Some(frame.pts);
        let rtp = (frame.pts as u32).wrapping_add(self.offset);
        let alac = alac_verbatim(&frame.data, self.channels)?;
        let due = self
            .last_sync
            .is_none_or(|t| now.duration_since(t) >= SYNC_INTERVAL);
        if due {
            self.send_sync(timeline, rtp, capture);
        }
        let packets = lock_pk(&self.packetizer).frame(rtp, &alac)?;
        for p in packets {
            self.pace();
            if let Err(e) = self.data.send(&p) {
                debug!("airplay: audio send: {e}");
            }
        }
        self.frames += 1;
        if self.frames == 1 {
            debug!("airplay: first audio frame sent (rtp {rtp}, {samples} samples)");
        }
        Ok(SinkStatus::Accepted)
    }
}

// ---------------------------------------------------------------------------
// The sink

struct Media {
    clock: MediaClock,
    video: VideoState,
    audio: Option<AudioState>,
    data_sock: TcpStream,
    threads: Vec<JoinHandle<()>>,
}

pub struct MirrorSink<S: ControlStream> {
    name: String,
    caps: Capabilities,
    session: Session<S>,
    shared: Arc<Shared>,
    media: Option<Media>,
    stopped: bool,
}

impl<S: ControlStream> MirrorSink<S> {
    pub fn new(name: String, session: Session<S>, opts: &MirrorOptions) -> MirrorSink<S> {
        let c = session.canvas;
        let fps = c.max_fps.unwrap_or(60).clamp(1, 60);
        let audio_formats = if opts.audio && session.audio.ports.is_some() {
            match session.audio.codec {
                AudioCodecId::Alac => vec![AudioFormat {
                    codec: AudioCodec::Alac,
                    sample_rate: SAMPLE_RATE,
                    channels: 2,
                }],
                AudioCodecId::AacEld => vec![AudioFormat {
                    codec: AudioCodec::AacEld,
                    sample_rate: SAMPLE_RATE,
                    channels: 2,
                }],
            }
        } else {
            Vec::new()
        };
        let caps = Capabilities {
            max_width: c.width,
            max_height: c.height,
            max_fps: Ratio::integer(fps),
            video_codecs: vec![VideoCodec::H264],
            audio_formats,
            audio_family: AudioFamily::AirPlay,
            max_bitrate_kbps: Some(MAX_BITRATE_KBPS),
            min_latency_ms: None,
            preferred_latency_ms: Some(session.leads.video_ms),
        };
        info!(
            "airplay: {name}: session ready ({}, {}x{}@{fps}, audio {:?})",
            match session.timeline {
                Timeline::Ptp(_) => "PTP",
                Timeline::Ntp => "NTP",
            },
            c.width,
            c.height,
            caps.audio_formats.first().map(|f| f.codec)
        );
        MirrorSink {
            name,
            caps,
            session,
            shared: Arc::new(Shared {
                queue: Mutex::new(Queue::default()),
                cond: Condvar::new(),
                finished: AtomicBool::new(false),
                failed: Mutex::new(None),
                stopping: AtomicBool::new(false),
                first_frame_sent: AtomicBool::new(false),
                need_key: AtomicBool::new(false),
            }),
            media: None,
            stopped: false,
        }
    }

    pub fn session(&self) -> &Session<S> {
        &self.session
    }

    fn audio_state(&self, audio: Option<&AudioInfo>) -> Result<Option<AudioState>> {
        let (Some(info), Some(ports)) = (audio, self.session.audio.ports) else {
            return Ok(None);
        };
        if info.codec != AudioCodec::PcmS16le || info.sample_rate != SAMPLE_RATE {
            return Err(Error::unsupported(format!(
                "airplay: audio must be 44.1 kHz s16le PCM (got {:?} at {} Hz)",
                info.codec, info.sample_rate
            )));
        }
        if self.session.audio.codec != AudioCodecId::Alac {
            return Err(Error::unsupported(
                "airplay: AAC-ELD audio is not implemented",
            ));
        }
        let ip = self.session.peer.ip();
        let cipher = match (&self.session.audio.key, &self.session.fairplay) {
            (Some(k), _) => AudioCipher::chacha(k),
            (None, Some(fp)) => AudioCipher::aes_cbc(&fp.master, &fp.eiv)?,
            (None, None) => AudioCipher::Clear,
        };
        let features = self.session.info.features;
        let redundancy = if policy::has(features, bit::RFC2198) {
            Redundancy::Rfc2198
        } else {
            Redundancy::Resend
        };
        let socks = &self.session.audio.sockets;
        let clone = |s: &UdpSocket| {
            s.try_clone()
                .map_err(|e| Error::io("airplay: clone UDP", e))
        };
        let data = clone(&socks.data)?;
        data.connect(SocketAddr::new(ip, ports.rtp))
            .map_err(|e| Error::io("airplay: audio connect", e))?;
        Ok(Some(AudioState {
            channels: info.channels,
            packetizer: Arc::new(Mutex::new(Packetizer::new(cipher, redundancy))),
            data,
            control: clone(&socks.control)?,
            rtcp: SocketAddr::new(ip, ports.rtcp),
            lead: Duration::from_millis(self.session.leads.audio_ms as u64),
            lead_samples: self.session.leads.audio_samples(),
            started: false,
            stale: 0,
            offset: 0,
            last_pts: None,
            last_sync: None,
            first_sync: true,
            window: (Instant::now(), 0),
            frames: 0,
        }))
    }

    fn spawn(
        threads: &mut Vec<JoinHandle<()>>,
        name: &str,
        f: impl FnOnce() + Send + 'static,
    ) -> Result<()> {
        threads.push(
            std::thread::Builder::new()
                .name(name.into())
                .spawn(f)
                .map_err(|e| Error::io(format!("airplay: spawn {name}"), e))?,
        );
        Ok(())
    }

    fn start_inner(&mut self, video: &VideoInfo, audio: Option<&AudioInfo>) -> Result<()> {
        let vstate = VideoState::new(video, self.session.leads.video_ms)?;
        let astate = self.audio_state(audio)?;
        let data = self.session.start_video()?;
        let cipher = if self.session.hap {
            VideoCipher::chacha(&keys::video_key(
                &self.session.secret.0,
                self.session.video_id,
            )?)
        } else {
            let fp =
                self.session.fairplay.as_ref().ok_or_else(|| {
                    Error::unsupported("airplay: legacy profile without FairPlay")
                })?;
            let (k, iv) = keys::legacy_video_key_iv(&fp.master, self.session.video_id);
            VideoCipher::aes_ctr(&k, &iv)?
        };
        let mut threads = Vec::new();
        let clone = |s: &TcpStream| {
            s.try_clone()
                .map_err(|e| Error::io("airplay: clone socket", e))
        };

        // Writer.
        {
            let shared = self.shared.clone();
            let sock = clone(&data)?;
            let timeline = self.session.timeline.clone();
            Self::spawn(&mut threads, "airplay-video", move || {
                writer_loop(&shared, sock, cipher, timeline)
            })?;
        }
        // Data connection reader: EOF means the receiver ended mirroring.
        {
            let shared = self.shared.clone();
            let mut sock = clone(&data)?;
            Self::spawn(&mut threads, "airplay-data", move || {
                let mut buf = [0u8; 1024];
                loop {
                    match sock.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => trace!("airplay: {n} bytes on the video data connection"),
                    }
                }
                if !shared.stopping.load(Ordering::Acquire) {
                    info!("airplay: receiver closed the video connection");
                    shared.finished.store(true, Ordering::Release);
                }
            })?;
        }
        // Feedback (§4.13), first one immediately.
        {
            let shared = self.shared.clone();
            let control = self.session.control.clone();
            let timeline = self.session.timeline.clone();
            Self::spawn(&mut threads, "airplay-feedback", move || {
                feedback_loop(&shared, &control, &timeline)
            })?;
        }
        // Retransmits (§8.6).
        if let Some(a) = &astate {
            let shared = self.shared.clone();
            let sock = a
                .control
                .try_clone()
                .map_err(|e| Error::io("airplay: clone UDP", e))?;
            let pk = a.packetizer.clone();
            let ip = self.session.peer.ip();
            Self::spawn(&mut threads, "airplay-rtcp", move || {
                rtcp_loop(&shared, &sock, &pk, ip)
            })?;
        }
        info!(
            "airplay: {}: streaming {}x{} video{}",
            self.name,
            vstate.width,
            vstate.height,
            if astate.is_some() {
                " and ALAC audio"
            } else {
                ""
            }
        );
        self.media = Some(Media {
            clock: MediaClock::new(),
            video: vstate,
            audio: astate,
            data_sock: data,
            threads,
        });
        Ok(())
    }

    fn queue_video(shared: &Shared, items: Vec<Item>, keyframe: bool) -> SinkStatus {
        let mut q = shared.queue();
        let bytes: usize = items.iter().map(Item::len).sum();
        if q.closed {
            return SinkStatus::Dropped;
        }
        if q.items.len() + items.len() > MAX_QUEUED || q.bytes + bytes > MAX_QUEUED_BYTES {
            if !shared.need_key.swap(true, Ordering::AcqRel) {
                debug!("airplay: video queue full; skipping to the next keyframe");
            }
            return SinkStatus::Dropped;
        }
        if keyframe {
            shared.need_key.store(false, Ordering::Release);
        }
        q.bytes += bytes;
        q.items.extend(items);
        drop(q);
        shared.cond.notify_all();
        SinkStatus::Accepted
    }

    fn push_video(&mut self, frame: &EncodedFrame) -> Result<SinkStatus> {
        let Some(m) = self.media.as_mut() else {
            return Err(Error::protocol("airplay: push before start"));
        };
        let now = Instant::now();
        let p = frame_payload(&frame.data, m.video.length_size)?;
        if p.idr {
            if let Some(sps) = p.sps.as_ref().filter(|s| **s != m.video.sps) {
                debug!("airplay: SPS changed");
                m.video.set_sps(sps.clone());
                m.video.need_codec = true;
            }
            if let Some(pps) = p.pps.as_ref().filter(|s| **s != m.video.pps) {
                debug!("airplay: PPS changed");
                m.video.pps = pps.clone();
                m.video.need_codec = true;
            }
        } else if m.video.need_codec || self.shared.need_key.load(Ordering::Acquire) {
            return Ok(SinkStatus::Dropped);
        }
        if p.data.is_empty() {
            return Ok(SinkStatus::Dropped);
        }
        let capture = capture_instant(&m.clock, frame.pts, VIDEO_CLOCK_RATE, now);
        let ts = m
            .video
            .timestamp(self.session.timeline.at(capture).wrapping_add(m.video.lead));
        let mut items = Vec::with_capacity(2);
        if p.idr && m.video.need_codec {
            let rec = avcc_record(&m.video.sps, &m.video.pps)?;
            let mut pkt = codec_header(rec.len(), ts, m.video.width, m.video.height)?.to_vec();
            pkt.extend_from_slice(&rec);
            items.push(Item::Codec(pkt));
        }
        items.push(Item::Frame {
            payload: p.data,
            ts,
            idr: p.idr,
        });
        let status = Self::queue_video(&self.shared, items, p.idr);
        if status == SinkStatus::Accepted && p.idr {
            m.video.need_codec = false;
        }
        Ok(status)
    }

    fn push_audio(&mut self, frame: &EncodedFrame) -> Result<SinkStatus> {
        let Some(m) = self.media.as_mut() else {
            return Err(Error::protocol("airplay: push before start"));
        };
        let Some(a) = m.audio.as_mut() else {
            return Ok(SinkStatus::Dropped);
        };
        if !self.shared.first_frame_sent.load(Ordering::Acquire) {
            return Ok(SinkStatus::Dropped);
        }
        a.push(frame, &m.clock, &self.session.timeline)
    }
}

fn feedback_loop<S: ControlStream>(
    shared: &Shared,
    control: &Mutex<crate::airplay::session::Control<S>>,
    timeline: &Timeline,
) {
    let mut next = Instant::now();
    loop {
        {
            let mut q = shared.queue();
            loop {
                if shared.stopping.load(Ordering::Acquire) || q.closed {
                    return;
                }
                let now = Instant::now();
                if now >= next {
                    break;
                }
                q = shared
                    .cond
                    .wait_timeout(q, next - now)
                    .map(|(g, _)| g)
                    .unwrap_or_else(|e| e.into_inner().0);
            }
        }
        next += FEEDBACK_INTERVAL;
        let mut c = lock_control(control);
        match c.request("POST", "/feedback", &[], None, DEFAULT_TIMEOUT) {
            Ok(r) => {
                let t1 = Instant::now();
                drop(c);
                if !r.is_success() {
                    debug!("airplay: /feedback answered {}", r.status);
                }
                if let (Timeline::Ptp(clock), Some(r1)) =
                    (timeline, anchor_from_headers(&r.headers))
                {
                    clock.reanchor(t1, r1);
                }
            }
            Err(e) if c.is_broken() => {
                drop(c);
                shared.fail(e);
                return;
            }
            Err(e) => warn!("airplay: /feedback: {e}"),
        }
    }
}

fn rtcp_loop(shared: &Shared, sock: &UdpSocket, pk: &Mutex<Packetizer>, peer: IpAddr) {
    let _ = sock.set_read_timeout(Some(Duration::from_millis(100)));
    let mut buf = [0u8; 2048];
    while !shared.stopping.load(Ordering::Acquire) {
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if from.ip() != peer {
            continue;
        }
        let replies = lock_pk(pk).retransmit(&buf[..n]);
        if replies.is_empty() {
            trace!("airplay: {n} bytes on the audio control socket");
            continue;
        }
        debug!("airplay: retransmitting {} audio packets", replies.len());
        for r in replies {
            let _ = sock.send_to(&r, from);
        }
    }
}

impl<S: ControlStream> Sink for MirrorSink<S> {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn start(&mut self, video: &VideoInfo, audio: Option<&AudioInfo>) -> Result<()> {
        if self.media.is_some() {
            return Err(Error::protocol("airplay: sink already started"));
        }
        self.start_inner(video, audio)
    }

    fn push(&mut self, frame: &EncodedFrame) -> Result<SinkStatus> {
        if let Some(e) = self.shared.take_failure() {
            return Err(e);
        }
        if self.shared.finished.load(Ordering::Acquire) {
            return Ok(SinkStatus::Finished);
        }
        match frame.track {
            TrackKind::Video => self.push_video(frame),
            TrackKind::Audio(AudioFamily::AirPlay) => self.push_audio(frame),
            TrackKind::Audio(_) => Ok(SinkStatus::Dropped),
        }
    }

    fn wants_keyframe(&self) -> bool {
        self.media.is_none() || self.shared.need_key.load(Ordering::Acquire)
    }

    fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        // Let the writer finish what is queued, then stop everything.
        {
            let mut q = self.shared.queue();
            q.closed = true;
        }
        self.shared.cond.notify_all();
        let mut threads = Vec::new();
        if let Some(m) = self.media.as_mut() {
            // The writer drains and exits on `closed`.
            if let Some(w) = (!m.threads.is_empty()).then(|| m.threads.remove(0)) {
                let _ = w.join();
            }
            threads = std::mem::take(&mut m.threads);
        }
        self.shared.stopping.store(true, Ordering::Release);
        self.shared.cond.notify_all();
        let r = self.session.teardown(DEFAULT_TIMEOUT);
        if let Some(m) = self.media.take() {
            let _ = m.data_sock.shutdown(std::net::Shutdown::Both);
            if let Some(a) = &m.audio {
                debug!("airplay: sent {} audio frames", a.frames);
            }
        }
        for t in threads {
            let _ = t.join();
        }
        info!("airplay: {}: stopped", self.name);
        match r {
            Err(e) if e.kind() == ErrorKind::Closed => {
                debug!("airplay: teardown: {e}");
                Ok(())
            }
            other => other,
        }
    }
}

impl<S: ControlStream> Drop for MirrorSink<S> {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
