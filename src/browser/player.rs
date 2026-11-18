//! `--to browser`: a receiver that plays tvpipe's stream in a browser tab
//! (W14; protocol in docs/player-protocol.md).
//!
//! [`BrowserSink`] serves the player page (`player.html`, `player.js`) on
//! 127.0.0.1 with the same security as the share page
//! (`browser::share`): a random `?t=` token on the page and the socket, a
//! loopback `Host` (no DNS rebinding), and the WebSocket only from the
//! page's own origin. Any number of tabs may watch. Each gets the decoder
//! configuration as JSON, then one binary SLRW record (decision 0008) per
//! WebSocket message: H.264 access units (AVCC, as the session has them)
//! and s16le PCM, stamped with the frame's instant on the session's
//! shared clock (decision 0010) in microseconds of tvpipe's clock. The
//! page maps that clock to its own (ping/pong), adds the playout delay and
//! presents video and audio on time, in step with the other receivers.
//!
//! Each tab has its own bounded queue and writer thread, so `push` never
//! blocks: a tab that falls behind drops frames and waits for the next
//! keyframe, the others are not affected. A tab that joins mid-stream
//! waits for the next keyframe too; the sink asks the input for one
//! ([`Sink::wants_keyframe`]) only while no tab is playing, because the
//! fanout answers that by skipping video for the whole sink, which would
//! freeze the tabs already watching. Without a tab the frames are
//! dropped and the session goes on.
//!
//! Audio: the browser takes the AirPlay family's PCM (any common rate,
//! mono or stereo), which needs no extra encode for the test pattern,
//! games, `share` and AirPlay sessions, and only one cheap PCM track from
//! ffmpeg otherwise. Like the AirPlay sink, it lists ALAC formats (what
//! the AirPlay family plans as PCM, see `passthrough::sink_input_formats`).

use std::collections::VecDeque;
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::codec::json::Value;
use crate::error::{Error, ErrorKind, Result};
use crate::media::clock::{SessionClock, SinkClock, Tolerance};
use crate::media::h264;
use crate::media::slrw::{self, Kind};
use crate::net::http::Request;
use crate::net::httpd::{self, Reply};
use crate::net::websocket::{self, close_code, Message, Role, Sender};
use crate::types::{
    AudioCodec, AudioFamily, AudioFormat, AudioInfo, Capabilities, EncodedFrame, Sink, SinkStatus,
    TrackKind, VideoCodec, VideoInfo, VIDEO_CLOCK_RATE,
};
use crate::util::ratio::Ratio;

pub const PLAYER_HTML: &str = include_str!("player.html");
pub const PLAYER_JS: &str = include_str!("player.js");

/// Protocol version (`config` says it; docs/player-protocol.md).
pub const VERSION: u32 = 1;
/// Largest message a page may send (all are small JSON).
const MAX_PAGE_MESSAGE: usize = 64 << 10;
/// Per tab: messages and bytes queued for its socket; more are dropped.
const TAB_QUEUE_MESSAGES: usize = 256;
const TAB_QUEUE_BYTES: usize = 8 << 20;
/// How often a tab's reader checks for shutdown.
const TAB_POLL: Duration = Duration::from_millis(200);
/// The largest picture the page is offered (WebCodecs decodes more, but
/// nothing tvpipe makes is bigger).
const MAX_SIZE: (u32, u32) = (3840, 2160);
const MAX_FPS: u32 = 60;
/// PCM rates and channel counts the page plays (Web Audio resamples).
const RATES: [u32; 7] = [48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 96_000];
/// How long the per-tab writer waits for the socket before it gives up
/// on a stalled tab.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long `stop` waits for the tabs to get the end.
const STOP_WAIT: Duration = Duration::from_millis(500);

/// How a `--to browser` receiver is set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerOptions {
    /// Name in logs.
    pub name: String,
    /// Port on 127.0.0.1 (0: any free one).
    pub port: u16,
    /// Open the page with the OS's opener.
    pub open: bool,
    /// Block in [`BrowserSink::bind`] until a tab is connected.
    pub wait_for_viewer: bool,
    /// Playout delay: the page shows a frame this long after its instant
    /// on the session's clock.
    pub latency_ms: u32,
    /// Offer audio.
    pub audio: bool,
}

impl Default for PlayerOptions {
    fn default() -> PlayerOptions {
        PlayerOptions {
            name: "browser".into(),
            port: 0,
            open: false,
            wait_for_viewer: false,
            latency_ms: 100,
            audio: true,
        }
    }
}

/// The capabilities of every browser receiver.
pub fn capabilities(opts: &PlayerOptions) -> Capabilities {
    let mut audio_formats = Vec::new();
    if opts.audio {
        for channels in [2, 1] {
            for sample_rate in RATES {
                audio_formats.push(AudioFormat {
                    codec: AudioCodec::Alac,
                    sample_rate,
                    channels,
                });
            }
        }
    }
    Capabilities {
        max_width: MAX_SIZE.0,
        max_height: MAX_SIZE.1,
        max_fps: Ratio::integer(MAX_FPS),
        video_codecs: vec![VideoCodec::H264],
        audio_formats,
        audio_family: AudioFamily::AirPlay,
        max_bitrate_kbps: None,
        min_latency_ms: None,
        preferred_latency_ms: Some(opts.latency_ms),
        // A window of any shape: the page letterboxes.
        display_aspect: None,
    }
}

/// The WebCodecs codec string of an avcC (`avc1.PPCCLL`).
pub fn codec_string(avcc: &[u8]) -> Option<String> {
    let c = h264::AvcConfig::parse(avcc).ok()?;
    Some(format!(
        "avc1.{:02X}{:02X}{:02X}",
        c.profile_idc, c.profile_compatibility, c.level_idc
    ))
}

/// The `config` message for a stream.
pub fn config_message(
    video: &VideoInfo,
    audio: Option<&AudioInfo>,
    latency_ms: u32,
    name: &str,
) -> Result<String> {
    let mut v = Value::object()
        .with("width", video.width)
        .with("height", video.height)
        .with("fps", video.fps.as_f64());
    if video.avcc.is_empty() {
        // No avcC: the access units carry SPS/PPS in band (Annex B).
        v = v.with("codec", "avc1.42E01F");
    } else {
        let codec = codec_string(&video.avcc)
            .ok_or_else(|| Error::malformed("browser: the stream's avcC does not parse"))?;
        v = v
            .with("codec", codec)
            .with("description", crate::net::base64::encode(&video.avcc));
    }
    let a = audio.map(|a| {
        Value::object()
            .with("codec", "pcm-s16le")
            .with("sampleRate", a.sample_rate)
            .with("channels", a.channels as u32)
    });
    Ok(Value::object()
        .with("type", "config")
        .with("version", VERSION)
        .with("name", name)
        .with("video", v)
        .with("audio", a.unwrap_or(Value::Null))
        .with("latency", latency_ms)
        .to_json())
}

fn status_message(state: &str, message: &str) -> String {
    Value::object()
        .with("type", "status")
        .with("state", state)
        .with("message", message)
        .to_json()
}

fn end_message(reason: &str) -> String {
    Value::object()
        .with("type", "end")
        .with("reason", reason)
        .to_json()
}

/// What a tab's writer sends.
enum Out {
    Record(Arc<[u8]>),
    /// A last text message, then a close frame.
    End(String),
}

/// A tab's outgoing queue, drained by its writer thread.
struct TabQueue {
    state: Mutex<QueueState>,
    cv: Condvar,
}

#[derive(Default)]
struct QueueState {
    msgs: VecDeque<Out>,
    bytes: usize,
    /// No more records (the end may still be queued).
    closing: bool,
    /// The writer is gone.
    closed: bool,
}

impl TabQueue {
    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queues a record; `false` when full or closing.
    fn offer(&self, msg: &Arc<[u8]>) -> bool {
        let mut q = self.lock();
        if q.closing || q.msgs.len() >= TAB_QUEUE_MESSAGES || q.bytes + msg.len() > TAB_QUEUE_BYTES
        {
            return false;
        }
        q.bytes += msg.len();
        q.msgs.push_back(Out::Record(msg.clone()));
        drop(q);
        self.cv.notify_one();
        true
    }

    /// Drops queued records and sends `end` and a close instead (from the
    /// writer thread: a stalled tab does not hold up the caller).
    fn end(&self, end: String) {
        let mut q = self.lock();
        if !q.closing {
            q.closing = true;
            q.msgs.clear();
            q.bytes = 0;
            q.msgs.push_back(Out::End(end));
        }
        drop(q);
        self.cv.notify_all();
    }

    /// Stops the writer without a goodbye.
    fn close(&self) {
        let mut q = self.lock();
        q.closing = true;
        q.closed = true;
        drop(q);
        self.cv.notify_all();
    }

    fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// Sends queued messages until closed or the socket fails.
    fn run(&self, sender: &Sender<TcpStream>) {
        loop {
            let msg = {
                let mut q = self.lock();
                loop {
                    if q.closed {
                        return;
                    }
                    if let Some(m) = q.msgs.pop_front() {
                        if let Out::Record(r) = &m {
                            q.bytes -= r.len();
                        }
                        break m;
                    }
                    q = self.cv.wait(q).unwrap_or_else(|e| e.into_inner());
                }
            };
            let ok = match msg {
                Out::Record(r) => sender.send_binary(&r).is_ok(),
                Out::End(text) => {
                    let _ = sender.send_text(&text);
                    let _ = sender.close(close_code::GOING_AWAY, "the stream ended");
                    false
                }
            };
            if !ok {
                self.close();
                return;
            }
        }
    }
}

struct Tab {
    id: u64,
    sender: Sender<TcpStream>,
    queue: Arc<TabQueue>,
    /// Drop video until the next keyframe (joined, overflowed, or the
    /// page's decoder asked).
    waiting_key: bool,
    /// Frames this tab dropped (full queue).
    dropped: u64,
}

#[derive(Default)]
struct State {
    next_id: u64,
    tabs: Vec<Tab>,
    /// The `config` message, once the stream started.
    config: Option<String>,
    status: Option<String>,
    shutdown: bool,
}

struct Shared {
    name: String,
    token: String,
    port: u16,
    /// tvpipe's clock for the page: µs since this instant.
    epoch: Instant,
    state: Mutex<State>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now_us(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }

    fn us_of(&self, t: Instant) -> u64 {
        t.saturating_duration_since(self.epoch).as_micros() as u64
    }

    fn handle(self: &Arc<Self>, req: &Request) -> Reply {
        let port = self.port;
        let loopback = |authority: &str| {
            [
                format!("127.0.0.1:{port}"),
                format!("localhost:{port}"),
                format!("[::1]:{port}"),
            ]
            .iter()
            .any(|h| h.eq_ignore_ascii_case(authority))
        };
        if !loopback(req.headers.get("host").unwrap_or("")) {
            return Reply::Response(httpd::error_response(421, "Misdirected Request"));
        }
        if req.method != "GET" && req.method != "HEAD" {
            return Reply::Response(httpd::error_response(405, "Method Not Allowed"));
        }
        let (path, query) = req.target.split_once('?').unwrap_or((&req.target, ""));
        let token_ok = query.split('&').any(|kv| {
            kv.strip_prefix("t=")
                .is_some_and(|t| crate::crypto::ring::ct_eq(t.as_bytes(), self.token.as_bytes()))
        });
        match path {
            "/player.js" => Reply::Response(httpd::response(
                200,
                "OK",
                "text/javascript; charset=utf-8",
                PLAYER_JS.as_bytes().to_vec(),
            )),
            "/favicon.ico" => Reply::Response(httpd::response(
                204,
                "No Content",
                "image/x-icon",
                Vec::new(),
            )),
            _ if !token_ok => Reply::Response(httpd::error_response(
                403,
                "Forbidden (open the URL tvpipe printed)",
            )),
            "/" => Reply::Response(httpd::response(
                200,
                "OK",
                "text/html; charset=utf-8",
                PLAYER_HTML.as_bytes().to_vec(),
            )),
            "/ws" if websocket::is_upgrade(req) => {
                // Browsers send the page's origin: only ours may connect
                // (other clients send none).
                let origin_ok = req
                    .headers
                    .get("origin")
                    .is_none_or(|o| o.strip_prefix("http://").is_some_and(loopback));
                if !origin_ok {
                    return Reply::Response(httpd::error_response(403, "Forbidden origin"));
                }
                match websocket::server_response(req) {
                    Ok(resp) => {
                        let me = self.clone();
                        Reply::Upgrade(resp, Box::new(move |tcp, rest| me.tab(tcp, rest)))
                    }
                    Err(e) => {
                        debug!("{}: bad upgrade: {e}", self.name);
                        let mut r = if e.kind() == ErrorKind::Unsupported {
                            httpd::error_response(426, "Upgrade Required")
                        } else {
                            httpd::error_response(400, "Bad Request")
                        };
                        r.headers.add("Sec-WebSocket-Version", "13");
                        Reply::Response(r)
                    }
                }
            }
            _ => Reply::Response(httpd::error_response(404, "Not Found")),
        }
    }

    /// One tab, on its connection thread until it closes.
    fn tab(self: Arc<Self>, tcp: TcpStream, rest: Vec<u8>) {
        if tcp.set_read_timeout(Some(TAB_POLL)).is_err()
            || tcp.set_write_timeout(Some(WRITE_TIMEOUT)).is_err()
        {
            return;
        }
        let limits = websocket::Limits {
            max_message: MAX_PAGE_MESSAGE,
        };
        let Ok((mut rx, tx)) = websocket::pair(tcp, rest, Role::Server, limits) else {
            return;
        };
        let queue = Arc::new(TabQueue {
            state: Mutex::new(QueueState::default()),
            cv: Condvar::new(),
        });
        let id = {
            let mut st = self.lock();
            if st.shutdown {
                let _ = tx.send_text(&end_message("the stream ended"));
                let _ = tx.close(close_code::GOING_AWAY, "");
                return;
            }
            st.next_id += 1;
            let id = st.next_id;
            // The config goes out before the tab can get records.
            for m in [&st.status, &st.config].into_iter().flatten() {
                let _ = tx.send_text(m);
            }
            st.tabs.push(Tab {
                id,
                sender: tx.clone(),
                queue: queue.clone(),
                waiting_key: true,
                dropped: 0,
            });
            self.cv.notify_all();
            id
        };
        let (q, s) = (queue.clone(), tx.clone());
        let writer = std::thread::Builder::new()
            .name("browser-tab".into())
            .spawn(move || q.run(&s));
        if writer.is_err() {
            queue.close();
        }
        info!("{}: a viewer connected (tab {id})", self.name);
        let why = loop {
            if self.lock().shutdown {
                break "tvpipe stopped".to_owned();
            }
            if queue.is_closed() {
                break "its connection stalled or failed".to_owned();
            }
            match rx.read_message() {
                Err(e) if e.kind() == ErrorKind::Timeout => {}
                Err(e) => break format!("connection failed: {e}"),
                Ok(Message::Close(_)) => break "closed".to_owned(),
                Ok(Message::Binary(_)) => trace!("{}: tab {id}: binary message ignored", self.name),
                Ok(Message::Text(t)) => self.page_message(id, &tx, &t),
            }
        };
        if !self.lock().shutdown {
            queue.close();
        } // else the writer sends the end, then stops.
        if let Ok(w) = writer {
            let _ = w.join();
        }
        let _ = tx.close(close_code::NORMAL, "");
        tx.with_stream(|s| {
            let _ = s.shutdown(std::net::Shutdown::Both);
        });
        let dropped = {
            let mut st = self.lock();
            let i = st.tabs.iter().position(|t| t.id == id);
            let d = i.map(|i| st.tabs.remove(i).dropped).unwrap_or(0);
            self.cv.notify_all();
            d
        };
        info!(
            "{}: viewer left (tab {id}: {why}{})",
            self.name,
            if dropped > 0 {
                format!("; it dropped {dropped} frames")
            } else {
                String::new()
            }
        );
    }

    fn page_message(&self, id: u64, tx: &Sender<TcpStream>, text: &str) {
        let v = match Value::parse(text) {
            Ok(v) => v,
            Err(e) => {
                debug!("{}: tab {id}: {e}", self.name);
                return;
            }
        };
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "ping" => {
                // The page's clock comes back as given (a number).
                let t = v.get("t").cloned().unwrap_or(Value::Null);
                let pong = Value::object()
                    .with("type", "pong")
                    .with("t", t)
                    .with("now", self.now_us())
                    .to_json();
                let _ = tx.send_text(&pong);
            }
            "hello" => {
                let agent = v.get("agent").and_then(Value::as_str).unwrap_or("");
                debug!("{}: tab {id}: {agent}", self.name);
            }
            "keyframe" => {
                let why = v.get("reason").and_then(Value::as_str).unwrap_or("");
                debug!("{}: tab {id} wants a keyframe ({why})", self.name);
                if let Some(t) = self.lock().tabs.iter_mut().find(|t| t.id == id) {
                    t.waiting_key = true;
                }
            }
            "stats" => debug!("{}: tab {id}: {v}", self.name),
            "log" => {
                let msg = v.get("message").and_then(Value::as_str).unwrap_or("");
                match v.get("level").and_then(Value::as_str) {
                    Some("error" | "warn") => warn!("{} page: {msg}", self.name),
                    _ => debug!("{} page: {msg}", self.name),
                }
            }
            other => trace!("{}: tab {id}: unknown message {other:?}", self.name),
        }
    }
}

/// A browser receiver (`--to browser`).
pub struct BrowserSink {
    name: String,
    caps: Capabilities,
    latency_ms: u32,
    http: Option<httpd::Server>,
    shared: Arc<Shared>,
    url: String,
    clock: Option<SessionClock>,
    sink_clock: SinkClock,
    audio_rate: u32,
    /// Access units are Annex B (no avcC from the input).
    annexb: bool,
    stopped: bool,
}

impl BrowserSink {
    /// Listens on 127.0.0.1 and prints the URL; with `wait_for_viewer`,
    /// returns once a tab is connected (or `stop` says so: an error).
    pub fn bind(opts: &PlayerOptions, stop: fn() -> bool) -> Result<BrowserSink> {
        let token = crate::util::hex::encode(&crate::crypto::ring::random_array::<16>()?);
        let addr: SocketAddr = (std::net::Ipv4Addr::LOCALHOST, opts.port).into();
        // The handler needs the port, known only after binding.
        let cell: Arc<Mutex<Option<Arc<Shared>>>> = Arc::new(Mutex::new(None));
        let c = cell.clone();
        let handler: httpd::Handler = Arc::new(move |req: &Request| {
            let shared = c.lock().unwrap_or_else(|e| e.into_inner()).clone();
            match shared {
                Some(s) => s.handle(req),
                None => Reply::Response(httpd::error_response(503, "Service Unavailable")),
            }
        });
        let http = httpd::Server::bind(addr, handler)?;
        let port = http.local_addr().port();
        let shared = Arc::new(Shared {
            name: opts.name.clone(),
            token: token.clone(),
            port,
            epoch: Instant::now(),
            state: Mutex::new(State {
                status: Some(status_message("waiting", "Waiting for the stream…")),
                ..State::default()
            }),
            cv: Condvar::new(),
        });
        *cell.lock().unwrap_or_else(|e| e.into_inner()) = Some(shared.clone());
        let url = format!("http://127.0.0.1:{port}/?t={token}");
        info!("{}: open {url} to watch", opts.name);
        if opts.open {
            if let Err(e) = crate::browser::platform::open_url(&url) {
                warn!("{}: --open: {e}", opts.name);
            }
        }
        let sink = BrowserSink {
            name: opts.name.clone(),
            caps: capabilities(opts),
            latency_ms: opts.latency_ms,
            http: Some(http),
            shared,
            url,
            clock: None,
            sink_clock: SinkClock::private(Tolerance::DEFAULT),
            audio_rate: 0,
            annexb: false,
            stopped: false,
        };
        if opts.wait_for_viewer {
            info!("{}: waiting for a viewer", opts.name);
            let mut st = sink.shared.lock();
            while st.tabs.is_empty() {
                if stop() {
                    return Err(Error::closed(format!(
                        "{}: stopped while waiting for a viewer",
                        opts.name
                    )));
                }
                st = sink
                    .shared
                    .cv
                    .wait_timeout(st, TAB_POLL)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
        Ok(sink)
    }

    /// The page's URL (with the token).
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The WebSocket URL the page connects to.
    pub fn ws_url(&self) -> String {
        format!(
            "ws://127.0.0.1:{}/ws?t={}",
            self.shared.port, self.shared.token
        )
    }

    /// The instant tvpipe's clock for the page counts from (record
    /// timestamps are µs since then).
    pub fn epoch(&self) -> Instant {
        self.shared.epoch
    }

    /// Tabs connected now.
    pub fn viewers(&self) -> usize {
        self.shared.lock().tabs.len()
    }

    fn broadcast_text(&self, st: &State, text: &str) {
        for t in &st.tabs {
            let _ = t.sender.send_text(text);
        }
    }

    /// One record for `frame`, stamped with its instant on the session
    /// clock (µs of tvpipe's clock).
    fn record(&self, frame: &EncodedFrame, at: Instant) -> Result<Vec<u8>> {
        let pts_us = self.shared.us_of(at);
        let mut out;
        match frame.track {
            TrackKind::Video => {
                let mut flags = 0u8;
                if frame.keyframe {
                    flags |= slrw::H264_KEYFRAME;
                }
                let body: std::borrow::Cow<'_, [u8]> = if self.annexb {
                    flags |= slrw::H264_ANNEX_B;
                    std::borrow::Cow::Owned(avcc_to_annexb(&frame.data)?)
                } else {
                    std::borrow::Cow::Borrowed(&frame.data[..])
                };
                out = Vec::with_capacity(14 + body.len());
                write_header(&mut out, Kind::H264, pts_us, body.len() + 1);
                out.push(flags);
                out.extend_from_slice(&body);
            }
            TrackKind::Audio(_) => {
                out = Vec::with_capacity(13 + frame.data.len());
                write_header(&mut out, Kind::Audio, pts_us, frame.data.len());
                out.extend_from_slice(&frame.data);
            }
        }
        Ok(out)
    }
}

fn write_header(out: &mut Vec<u8>, kind: Kind, pts_us: u64, len: usize) {
    out.push(kind.code());
    out.extend_from_slice(&pts_us.to_le_bytes());
    out.extend_from_slice(&(len as u32).to_le_bytes());
}

/// AVCC (4-byte lengths) to Annex B (4-byte start codes).
fn avcc_to_annexb(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() + 16);
    for nal in h264::avcc_nals(data, 4) {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal?);
    }
    Ok(out)
}

impl Sink for BrowserSink {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn set_clock(&mut self, clock: SessionClock) {
        self.clock = Some(clock);
    }

    fn start(&mut self, video: &VideoInfo, audio: Option<&AudioInfo>) -> Result<()> {
        if video.codec != VideoCodec::H264 {
            return Err(Error::unsupported(format!(
                "{}: {:?} video is not supported",
                self.name, video.codec
            )));
        }
        let audio = audio.filter(|a| {
            let ok = a.codec == AudioCodec::PcmS16le && (1..=2).contains(&a.channels);
            if !ok {
                warn!(
                    "{}: cannot play {:?} {} ch audio; video only",
                    self.name, a.codec, a.channels
                );
            }
            ok
        });
        self.audio_rate = audio.map_or(0, |a| a.sample_rate.max(1));
        self.annexb = video.avcc.is_empty();
        self.sink_clock = SinkClock::new(self.clock.take(), Tolerance::DEFAULT);
        let config = config_message(video, audio, self.latency_ms, &self.name)?;
        let status = status_message("streaming", "");
        let mut st = self.shared.lock();
        self.broadcast_text(&st, &status);
        self.broadcast_text(&st, &config);
        st.status = Some(status);
        st.config = Some(config);
        debug!(
            "{}: {}x{} H.264{}, playout delay {} ms",
            self.name,
            video.width,
            video.height,
            audio
                .map(|a| format!(", PCM {} Hz {} ch", a.sample_rate, a.channels))
                .unwrap_or_default(),
            self.latency_ms
        );
        Ok(())
    }

    fn push(&mut self, frame: &EncodedFrame) -> Result<SinkStatus> {
        let video = frame.track == TrackKind::Video;
        if !video && self.audio_rate == 0 {
            return Ok(SinkStatus::Dropped);
        }
        let rate = if video {
            VIDEO_CLOCK_RATE
        } else {
            self.audio_rate
        };
        let at = self.sink_clock.instant(frame.pts, rate, Instant::now());
        let mut st = self.shared.lock();
        let wanted = st
            .tabs
            .iter()
            .any(|t| !video || frame.keyframe || !t.waiting_key);
        if !wanted {
            return Ok(SinkStatus::Dropped);
        }
        let msg: Arc<[u8]> = self.record(frame, at)?.into();
        let mut taken = false;
        for t in &mut st.tabs {
            if video && t.waiting_key {
                if !frame.keyframe {
                    continue;
                }
                t.waiting_key = false;
            }
            if t.queue.offer(&msg) {
                taken = true;
            } else {
                t.dropped += 1;
                if video && !t.waiting_key {
                    debug!(
                        "{}: tab {} is behind; skipping to the next keyframe",
                        self.name, t.id
                    );
                    t.waiting_key = true;
                }
            }
        }
        Ok(if taken {
            SinkStatus::Accepted
        } else {
            SinkStatus::Dropped
        })
    }

    fn wants_keyframe(&self) -> bool {
        // Only while no tab plays: the fanout skips video for the whole
        // sink until the keyframe, which would freeze the others.
        let st = self.shared.lock();
        !st.tabs.is_empty() && st.tabs.iter().all(|t| t.waiting_key)
    }

    fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        let end = end_message("the stream ended");
        {
            let mut st = self.shared.lock();
            st.shutdown = true;
            st.config = None;
            for t in &st.tabs {
                t.queue.end(end.clone());
            }
            self.shared.cv.notify_all();
        }
        if let Some(mut h) = self.http.take() {
            h.shutdown();
        }
        // Give the tabs a moment to get the end (a stalled one is left
        // to its own write timeout).
        let deadline = Instant::now() + STOP_WAIT;
        let mut st = self.shared.lock();
        while !st.tabs.is_empty() {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            st = self
                .shared
                .cv
                .wait_timeout(st, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        Ok(())
    }
}

impl Drop for BrowserSink {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests;
