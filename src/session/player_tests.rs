//! `--to browser` (W14) in whole sessions, with a Rust WebSocket client
//! as the page: `preview` of the built-in test pattern, and a browser
//! next to a fake Cast receiver on one timeline, joining late.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::tests::{opts, SharedTally, Tallied};
use super::*;
use crate::airplay::session::MirrorOptions;
use crate::browser::protocol;
use crate::cast::fake_receiver::{FakeReceiver, Options as CastFake};
use crate::codec::json::Value;
use crate::media::clock::SessionClock;
use crate::media::slrw::Kind;
use crate::net::websocket::{self, Message};
use crate::types::{AudioInfo, EncodedFrame, SinkStatus, VideoInfo, VIDEO_CLOCK_RATE};

/// One test at a time: they share the stop flag.
static SERIAL: Mutex<()> = Mutex::new(());
static STOP: AtomicBool = AtomicBool::new(false);

fn stop_flag() -> bool {
    STOP.load(Ordering::SeqCst)
}

fn never() -> bool {
    false
}

fn no_io() -> GameIo {
    GameIo {
        stdin: Box::new(std::io::empty()),
        stdout: Box::new(std::io::sink()),
    }
}

/// What the fake page got.
#[derive(Default, Debug)]
struct PageLog {
    config: Option<Value>,
    /// `(kind, pts_us, keyframe, payload length)` in arrival order.
    records: Vec<(Kind, u64, bool, usize)>,
    first_record: Option<Instant>,
    connected: Option<Instant>,
    end: Option<String>,
}

/// A page reading everything until tvpipe closes; `video` counts the
/// video records so far.
fn spawn_page(ws_url: String, video: Arc<AtomicUsize>) -> JoinHandle<PageLog> {
    std::thread::spawn(move || {
        let mut log = PageLog::default();
        let (mut rx, tx) = websocket::connect(
            &ws_url,
            Duration::from_secs(10),
            websocket::Limits::default(),
        )
        .unwrap();
        log.connected = Some(Instant::now());
        tx.with_stream(|s| s.set_read_timeout(Some(Duration::from_secs(30))))
            .unwrap();
        loop {
            match rx.read_message() {
                Ok(Message::Text(t)) => {
                    let v = Value::parse(&t).unwrap();
                    match v.get("type").and_then(Value::as_str) {
                        Some("config") => log.config = Some(v),
                        Some("end") => {
                            log.end = v.get("reason").and_then(Value::as_str).map(Into::into)
                        }
                        _ => {}
                    }
                }
                Ok(Message::Binary(b)) => {
                    let p = protocol::parse_record(&b).unwrap();
                    log.first_record.get_or_insert_with(Instant::now);
                    let key = p.kind == Kind::H264 && p.payload[0] & 1 != 0;
                    if p.kind == Kind::H264 {
                        video.fetch_add(1, Ordering::SeqCst);
                    }
                    log.records.push((p.kind, p.pts_us, key, p.payload.len()));
                }
                Ok(Message::Close(_)) | Err(_) => return log,
            }
        }
    })
}

/// Logs `(pts, instant)` of the video a sink accepted, read from the
/// session clock it was given.
struct Logged {
    inner: Box<dyn Sink>,
    clock: Option<SessionClock>,
    log: Arc<Mutex<Vec<(u64, Instant)>>>,
}

impl Sink for Logged {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn capabilities(&self) -> &Capabilities {
        self.inner.capabilities()
    }
    fn set_clock(&mut self, clock: SessionClock) {
        self.clock = Some(clock.clone());
        self.inner.set_clock(clock)
    }
    fn start(&mut self, v: &VideoInfo, a: Option<&AudioInfo>) -> Result<()> {
        self.inner.start(v, a)
    }
    fn push(&mut self, f: &EncodedFrame) -> Result<SinkStatus> {
        let r = self.inner.push(f);
        if f.track == TrackKind::Video && matches!(r, Ok(SinkStatus::Accepted)) {
            let at = self
                .clock
                .as_ref()
                .unwrap()
                .instant_for(f.pts, VIDEO_CLOCK_RATE);
            self.log.lock().unwrap().push((f.pts, at.unwrap()));
        }
        r
    }
    fn wants_keyframe(&self) -> bool {
        self.inner.wants_keyframe()
    }
    fn stop(&mut self) -> Result<()> {
        self.inner.stop()
    }
}

/// Sets the stop flag once `cond` holds (or after `limit`).
fn stop_when(limit: Duration, cond: impl Fn() -> bool + Send + 'static) -> JoinHandle<bool> {
    std::thread::spawn(move || {
        let t0 = Instant::now();
        let ok = loop {
            if cond() {
                break true;
            }
            if t0.elapsed() > limit {
                break false;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        STOP.store(true, Ordering::SeqCst);
        ok
    })
}

#[test]
fn preview_plays_the_test_pattern_in_a_page() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    STOP.store(false, Ordering::SeqCst);
    let video = Arc::new(AtomicUsize::new(0));
    let page = Mutex::new(None);
    let connect = |t: &Target, _: &CastOptions, _: &MirrorOptions| -> Result<Box<dyn Sink>> {
        let Target::Browser(o) = t else {
            panic!("{t:?}")
        };
        let sink = BrowserSink::bind(o, never)?;
        *page.lock().unwrap() = Some((spawn_page(sink.ws_url(), video.clone()), *o.clone()));
        Ok(Box::new(sink))
    };
    let v = video.clone();
    let stopper = stop_when(Duration::from_secs(60), move || {
        v.load(Ordering::SeqCst) >= 20
    });
    // What `tvpipe preview --encoder builtin --size 320x180` parses to.
    let o = PlayOptions {
        to: vec!["browser".into()],
        size: Some(Size {
            width: 320,
            height: 180,
        }),
        encoder: Encoder::Builtin,
        ..opts()
    };
    play_with(o, no_io(), &connect, stop_flag).unwrap();
    assert!(stopper.join().unwrap(), "the page got too little video");
    let (page, popts) = page.lock().unwrap().take().unwrap();
    let log = page.join().unwrap();
    assert_eq!(popts.latency_ms, BROWSER_LATENCY_MS);
    assert!(popts.audio);

    let cfg = log.config.expect("config");
    assert_eq!(
        cfg.get("latency").unwrap().as_u32(),
        Some(BROWSER_LATENCY_MS)
    );
    let v = cfg.get("video").unwrap();
    assert_eq!(v.get("width").unwrap().as_u32(), Some(320));
    assert!(v
        .get("codec")
        .unwrap()
        .as_str()
        .unwrap()
        .starts_with("avc1.42"));
    let a = cfg.get("audio").unwrap();
    assert_eq!(a.get("sampleRate").unwrap().as_u32(), Some(48_000));
    assert_eq!(log.end.as_deref(), Some("the stream ended"));

    let vids: Vec<_> = log.records.iter().filter(|r| r.0 == Kind::H264).collect();
    assert!(vids.len() >= 20, "{}", vids.len());
    assert!(vids[0].2, "the first video record is a keyframe");
    // Video instants increase by about a frame (30 fps).
    for w in vids.windows(2) {
        assert!(w[1].1 > w[0].1, "{:?}", w);
    }
    let span = vids[vids.len() - 1].1 - vids[0].1;
    let expect = (vids.len() as u64 - 1) * 1_000_000 / 30;
    assert!(span.abs_diff(expect) < 2_000, "{span} vs {expect}");
    // PCM in s16le stereo frames.
    let audio: Vec<_> = log.records.iter().filter(|r| r.0 == Kind::Audio).collect();
    assert!(!audio.is_empty());
    assert!(audio.iter().all(|r| r.3 > 0 && r.3 % 4 == 0));
}

#[test]
fn a_late_tab_shares_the_cast_timeline() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    STOP.store(false, Ordering::SeqCst);
    let rx = FakeReceiver::spawn(CastFake::default());
    let cast_tally = SharedTally::default();
    type Log = Arc<Mutex<Vec<(u64, Instant)>>>;
    let (cast_log, browser_log): (Log, Log) = (Arc::default(), Arc::default());
    let browser = Mutex::new(None);
    let connect = |t: &Target, c: &CastOptions, _: &MirrorOptions| -> Result<Box<dyn Sink>> {
        Ok(match t {
            Target::Cast { addr, name } => Box::new(Logged {
                inner: Tallied::boxed(Box::new(cast::connect(*addr, name, c)?), None, &cast_tally),
                clock: None,
                log: cast_log.clone(),
            }),
            Target::Browser(o) => {
                let sink = BrowserSink::bind(o, never)?;
                *browser.lock().unwrap() = Some((sink.ws_url(), sink.epoch(), *o.clone()));
                Box::new(Logged {
                    inner: Box::new(sink),
                    clock: None,
                    log: browser_log.clone(),
                })
            }
            Target::AirPlay { .. } => panic!("no AirPlay here"),
        })
    };
    let video = Arc::new(AtomicUsize::new(0));
    let o = PlayOptions {
        to: vec![rx.addr.to_string(), "browser".into()],
        size: Some(Size {
            width: 320,
            height: 180,
        }),
        encoder: Encoder::Builtin,
        ..opts()
    };
    let (tally, b, v) = (cast_tally.clone(), &browser, video.clone());
    let page = std::thread::scope(|s| {
        // The page connects once the Cast receiver got some video.
        let joiner = s.spawn(move || {
            let t0 = Instant::now();
            while tally.lock().unwrap().video < 15 {
                assert!(t0.elapsed() < Duration::from_secs(60), "no Cast video");
                std::thread::sleep(Duration::from_millis(10));
            }
            let url = b.lock().unwrap().as_ref().unwrap().0.clone();
            let page = spawn_page(url, v.clone());
            let vv = v.clone();
            let stopped = stop_when(Duration::from_secs(60), move || {
                vv.load(Ordering::SeqCst) >= 15
            });
            (page, stopped)
        });
        play_with(o, no_io(), &connect, stop_flag).unwrap();
        joiner.join().unwrap()
    });
    assert!(page.1.join().unwrap(), "the page got too little video");
    let log = page.0.join().unwrap();
    let (_, epoch, popts) = browser.lock().unwrap().take().unwrap();
    // One budget: the browser shows frames when the Cast TV does.
    assert_eq!(popts.latency_ms, DEFAULT_LATENCY_MS);
    let cfg = log.config.expect("config");
    assert_eq!(
        cfg.get("latency").unwrap().as_u32(),
        Some(DEFAULT_LATENCY_MS)
    );
    let state = rx.shutdown();
    let st = state.lock().unwrap();
    assert!(st.stopped, "STOP not received");

    // The late tab started on a keyframe, soon (the built-in encoder makes
    // one on request, or the next GOP comes within half a second).
    let vids: Vec<_> = log.records.iter().filter(|r| r.0 == Kind::H264).collect();
    assert!(vids[0].2, "the first video record is a keyframe");
    let waited = log.first_record.unwrap() - log.connected.unwrap();
    assert!(waited < Duration::from_secs(5), "{waited:?}");

    // Every record carries the instant the session gave that frame, the
    // same the Cast sink got.
    let browser_log = browser_log.lock().unwrap().clone();
    let cast_log = cast_log.lock().unwrap().clone();
    assert_eq!(browser_log.len(), vids.len());
    let us = |t: Instant| t.saturating_duration_since(epoch).as_micros() as u64;
    let mut common = 0;
    for ((pts, at), rec) in browser_log.iter().zip(&vids) {
        assert_eq!(rec.1, us(*at), "pts {pts}");
        if let Some((_, c)) = cast_log.iter().find(|(p, _)| p == pts) {
            assert_eq!(at, c, "pts {pts}");
            common += 1;
        }
    }
    assert!(common >= 10, "{common} frames in common");
}
