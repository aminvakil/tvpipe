//! Reconnect tests (`session::rejoin`): fake Cast and AirPlay receivers
//! that drop out mid-stream and come back (at the same address, or at
//! another one a fake resolver hands out for the receiver's name), one
//! that ends the session itself (not reconnected), one that never comes
//! back (given up after the window) and one that comes back unable to
//! take the stream.
//!
//! The sessions stream the built-in test pattern (real time, keyframes
//! on demand) and end when the sinks reach their frame limits. The
//! assertions avoid timing where they can; where they cannot, the bounds
//! are loose (a loaded CI machine).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::rejoin::{Backoff, Rejoin};
use super::tests::{fake_airplay, never, opts};
use super::*;
use crate::airplay::fake_mirror::{Behaviour, FakeMirror};
use crate::airplay::session::MirrorOptions;
use crate::cast::fake_receiver::FakeReceiver;
use crate::media::clock::SessionClock;
use crate::types::{AudioInfo, EncodedFrame, SinkStatus, VideoInfo, VIDEO_CLOCK_RATE};

fn pattern_opts(to: &[&str]) -> PlayOptions {
    PlayOptions {
        to: to.iter().map(|s| s.to_string()).collect(),
        size: Some(Size {
            width: 320,
            height: 180,
        }),
        encoder: Encoder::Builtin,
        ..opts()
    }
}

fn no_io() -> GameIo {
    GameIo {
        stdin: Box::new(std::io::empty()),
        stdout: Box::new(std::io::sink()),
        approve: Box::new(|_, _| false),
    }
}

/// A video frame a sink accepted.
#[derive(Debug, Clone, Copy)]
struct Entry {
    /// Which connection of that receiver (0, 1, ...).
    conn: usize,
    pts: u64,
    /// Its instant on the session clock.
    at: Instant,
    key: bool,
}

/// What the fakes were asked and what the sinks accepted, per receiver
/// name.
#[derive(Default)]
struct Book {
    connects: Mutex<Vec<(String, SocketAddr)>>,
    frames: Mutex<HashMap<String, Vec<Entry>>>,
}

impl Book {
    fn connects_to(&self, name: &str) -> Vec<SocketAddr> {
        let c = self.connects.lock().unwrap();
        c.iter()
            .filter(|(n, _)| n == name)
            .map(|(_, a)| *a)
            .collect()
    }

    fn frames(&self, name: &str) -> Vec<Entry> {
        self.frames
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_default()
    }
}

/// Logs what the inner sink accepts into the book; with `left`, finishes
/// after that many video frames (the test pattern never ends).
struct Logged {
    inner: Box<dyn Sink>,
    book: Arc<Book>,
    conn: usize,
    left: Option<usize>,
    clock: Option<SessionClock>,
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
        let video = f.track == TrackKind::Video;
        if video {
            match &mut self.left {
                Some(0) => return Ok(SinkStatus::Finished),
                Some(n) => *n -= 1,
                None => {}
            }
        }
        let r = self.inner.push(f);
        if video && matches!(r, Ok(SinkStatus::Accepted)) {
            let at = self
                .clock
                .as_ref()
                .and_then(|c| c.instant_for(f.pts, VIDEO_CLOCK_RATE))
                .expect("the session clock");
            let entry = Entry {
                conn: self.conn,
                pts: f.pts,
                at,
                key: f.keyframe,
            };
            let mut frames = self.book.frames.lock().unwrap();
            frames
                .entry(self.name().to_owned())
                .or_default()
                .push(entry);
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

/// Where the fake resolver finds receivers by name.
type Names = Arc<Mutex<HashMap<String, Target>>>;

/// The fake resolver: names from `names` (with or without a protocol
/// prefix), addresses as usual (no network).
fn resolver(names: &Names) -> rejoin::Resolve {
    let names = names.clone();
    Arc::new(move |q: &str| {
        let (_, rest) = parse_target(q);
        if let Some(t) = names.lock().unwrap().get(rest) {
            return Ok(t.clone());
        }
        match address_target(parse_target(q).0, rest) {
            Some(t) => Ok(t),
            None => Err(Error::not_found(format!("no receiver named {rest:?}"))),
        }
    })
}

fn cast_at(name: &str, addr: SocketAddr) -> Target {
    Target::Cast {
        addr,
        name: name.into(),
    }
}

fn airplay_at(name: &str, addr: SocketAddr) -> Target {
    Target::AirPlay {
        name: name.into(),
        addr,
        device: None,
    }
}

/// Frame limits per receiver address (`None`: unlimited).
type Limits = HashMap<SocketAddr, Option<usize>>;

/// Connects Cast targets with the real sink, AirPlay ones to the fake at
/// that address; logs every sink into `book`.
fn connector(book: &Arc<Book>, mirrors: Vec<Arc<FakeMirror>>, limits: Limits) -> rejoin::Connect {
    let book = book.clone();
    Arc::new(move |t: &Target, c: &CastOptions, m: &MirrorOptions| {
        let (name, addr) = match t {
            Target::Cast { name, addr } | Target::AirPlay { name, addr, .. } => {
                (name.clone(), *addr)
            }
            Target::Browser(_) => unreachable!(),
        };
        let conn = book.connects_to(&name).len();
        book.connects.lock().unwrap().push((name.clone(), addr));
        let inner = match t {
            Target::Cast { .. } => Box::new(cast::connect(addr, &name, c)?) as Box<dyn Sink>,
            _ => {
                let fake = mirrors.iter().find(|f| f.addr == addr).ok_or_else(|| {
                    Error::io("connect", std::io::ErrorKind::ConnectionRefused.into())
                })?;
                fake_airplay(fake, &name, m)?
            }
        };
        Ok(Box::new(Logged {
            inner,
            book: book.clone(),
            conn,
            left: limits.get(&addr).copied().flatten(),
            clock: None,
        }) as Box<dyn Sink>)
    })
}

fn rejoin(window: Duration, resolve: rejoin::Resolve, connect: rejoin::Connect) -> Rejoin {
    Rejoin {
        window,
        backoff: Backoff {
            first: Duration::from_millis(100),
            max: Duration::from_millis(400),
        },
        resolve,
        connect,
        events: crate::events::Events::none(),
    }
}

fn play(o: PlayOptions, r: &Rejoin) -> Result<()> {
    play_rejoin(o, no_io(), &*r.connect, &never(), Some(r))
}

/// Waits until `name` accepted `n` frames on connection `conn`.
fn wait_frames(book: &Book, name: &str, conn: usize, n: usize) {
    let until = Instant::now() + Duration::from_secs(20);
    while book.frames(name).iter().filter(|e| e.conn == conn).count() < n {
        assert!(Instant::now() < until, "{name} never() got {n} frames");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The frames of both connections lie on one timeline: instants follow
/// the pts, and the rejoined connection starts at a keyframe after the
/// first one's last frame.
///
/// The session clock only ever moves its anchor later (it re-anchors when
/// frames arrive more than `MAX_LATENESS` late, which happens on loaded CI
/// runners), so the wall time between the two connections' first frames is
/// at least the media time between them, never less. That the rejoined
/// connection uses the *same* clock as everyone else is checked against the
/// other receiver's instants by the callers.
fn assert_rejoined_on_the_timeline(frames: &[Entry]) {
    let first: Vec<&Entry> = frames.iter().filter(|e| e.conn == 0).collect();
    let second: Vec<&Entry> = frames.iter().filter(|e| e.conn == 1).collect();
    assert!(!first.is_empty() && !second.is_empty(), "{frames:?}");
    assert!(second[0].key, "the rejoined receiver starts at a keyframe");
    assert!(second[0].pts > first.last().unwrap().pts);
    let (a, b) = (first[0], second[0]);
    let media = Duration::from_secs_f64((b.pts - a.pts) as f64 / VIDEO_CLOCK_RATE as f64);
    let wall = b.at - a.at;
    assert!(
        wall + Duration::from_millis(2) >= media,
        "rejoined frames are earlier than their pts allow: {wall:?} vs {media:?}"
    );
    // Re-anchors move the clock by the lateness that caused them; a few
    // hundred ms on a loaded runner, not the outage (seconds) a private
    // clock for the rejoined sink would show.
    assert!(
        wall <= media + Duration::from_millis(400),
        "the rejoined receiver is off the session's timeline: {wall:?} vs {media:?}"
    );
}

/// A Cast TV moves to another address mid-stream: the connection breaks,
/// its name resolves to the new address, it rejoins at a keyframe on the
/// session's timeline; a second TV streams on undisturbed.
#[test]
fn cast_receiver_moves_and_rejoins() {
    let old = FakeReceiver::spawn(Default::default());
    let new = FakeReceiver::spawn(Default::default());
    let other = FakeReceiver::spawn(Default::default());
    let names = Names::default();
    names
        .lock()
        .unwrap()
        .insert("Office TV".into(), cast_at("Office TV", old.addr));
    let book = Arc::new(Book::default());
    let limits = Limits::from([(new.addr, Some(30)), (other.addr, Some(90))]);
    let r = rejoin(
        Duration::from_secs(20),
        resolver(&names),
        connector(&book, Vec::new(), limits),
    );
    let other_name = other.addr.to_string();
    let mover = {
        let (book, names, new_addr) = (book.clone(), names.clone(), new.addr);
        std::thread::spawn(move || {
            wait_frames(&book, "Office TV", 0, 15);
            names
                .lock()
                .unwrap()
                .insert("Office TV".into(), cast_at("Office TV", new_addr));
            old.drop_connection();
            (Instant::now(), old)
        })
    };
    play(pattern_opts(&["Office TV", &other_name]), &r).unwrap();
    let (moved, old) = mover.join().unwrap();

    // Connected once at each address, the new one found by name.
    assert_eq!(book.connects_to("Office TV"), [old.addr, new.addr]);
    let office = book.frames("Office TV");
    assert_rejoined_on_the_timeline(&office);
    let back = office.iter().find(|e| e.conn == 1).unwrap().at;
    // The new receiver got that keyframe first, its RTP timestamps are
    // the session's pts.
    let state = new.shutdown();
    let st = state.lock().unwrap();
    let v = st.streams.iter().find(|s| s.codec == "h264").unwrap();
    let f0 = &v.frames[&0];
    assert!(f0.keyframe);
    let firsts: Vec<u64> = office
        .iter()
        .filter(|e| e.conn == 1)
        .map(|e| e.pts)
        .collect();
    assert_eq!(f0.rtp_timestamp, firsts[0] as u32);

    // The other TV: all its 90 frames, at the instants the session gave
    // the same pts on the Office TV, and frames all through the outage.
    let theirs = book.frames(&other_name);
    assert!(theirs.len() >= 45, "{}", theirs.len());
    let mut common = 0;
    for e in office.iter().filter(|e| e.conn == 1) {
        if let Some(o) = theirs.iter().find(|o| o.pts == e.pts) {
            assert_eq!(o.at, e.at, "pts {}", e.pts);
            common += 1;
        }
    }
    assert!(common >= 10, "{common} frames in common");
    let during = theirs
        .iter()
        .filter(|o| o.at > moved && o.at < back)
        .count();
    let outage = back.saturating_duration_since(moved);
    if outage > Duration::from_millis(300) {
        assert!(during > 0, "the other TV stalled during {outage:?}");
    }
    drop(st);
    old.shutdown();
    other.shutdown();
}

/// The user stops the cast on the TV: that receiver is not reconnected.
#[test]
fn cast_receiver_ending_the_session_is_not_reconnected() {
    let rx = FakeReceiver::spawn(Default::default());
    let other = FakeReceiver::spawn(Default::default());
    let names = Names::default();
    names
        .lock()
        .unwrap()
        .insert("Office TV".into(), cast_at("Office TV", rx.addr));
    let book = Arc::new(Book::default());
    let r = rejoin(
        Duration::from_secs(20),
        resolver(&names),
        connector(&book, Vec::new(), Limits::from([(other.addr, Some(45))])),
    );
    let other_name = other.addr.to_string();
    let stopper = {
        let book = book.clone();
        std::thread::spawn(move || {
            wait_frames(&book, "Office TV", 0, 10);
            rx.end_session();
            rx
        })
    };
    play(pattern_opts(&["Office TV", &other_name]), &r).unwrap();
    let rx = stopper.join().unwrap();
    assert_eq!(book.connects_to("Office TV"), [rx.addr]);
    let state = rx.shutdown();
    assert_eq!(state.lock().unwrap().sessions, 1);
    other.shutdown();
}

/// A receiver given by address that does not come back: retried at that
/// address, then given up after the window; with nobody left the session
/// fails.
#[test]
fn cast_receiver_gone_for_good_is_given_up() {
    let rx = FakeReceiver::spawn(Default::default());
    let addr = rx.addr;
    let book = Arc::new(Book::default());
    let window = Duration::from_millis(1200);
    // Once it is gone, connecting fails at once: on Windows a refused
    // loopback connect takes about 2 s (it retries the SYN), which leaves
    // no room for retries in the window.
    let gone = Arc::new(AtomicBool::new(false));
    let real = connector(&book, Vec::new(), Limits::new());
    let connect: rejoin::Connect = {
        let (book, gone) = (book.clone(), gone.clone());
        Arc::new(move |t: &Target, c: &CastOptions, m: &MirrorOptions| {
            if !gone.load(Ordering::SeqCst) {
                return real(t, c, m);
            }
            if let Target::Cast { name, addr } = t {
                book.connects.lock().unwrap().push((name.clone(), *addr));
            }
            Err(Error::io(
                "connect",
                std::io::ErrorKind::ConnectionRefused.into(),
            ))
        })
    };
    let r = rejoin(window, resolver(&Names::default()), connect);
    let killer = {
        let book = book.clone();
        let name = addr.to_string();
        std::thread::spawn(move || {
            wait_frames(&book, &name, 0, 10);
            // Gone: the connection breaks and nobody listens any more.
            let t = Instant::now();
            gone.store(true, Ordering::SeqCst);
            rx.shutdown();
            t
        })
    };
    let e = play(pattern_opts(&[&addr.to_string()]), &r).unwrap_err();
    let gone = killer.join().unwrap();
    assert!(e.to_string().contains("gave up"), "{e}");
    assert!(gone.elapsed() >= window, "{:?}", gone.elapsed());
    assert!(
        gone.elapsed() < Duration::from_secs(10),
        "{:?}",
        gone.elapsed()
    );
    let tries = book.connects_to(&addr.to_string());
    assert!(tries.len() >= 3, "{tries:?}");
    assert!(tries.iter().all(|a| *a == addr), "{tries:?}");
}

/// The receiver comes back unable to take the running stream (a smaller
/// screen): it stays dropped, without further attempts.
#[test]
fn incompatible_receiver_stays_dropped() {
    let old = FakeReceiver::spawn(Default::default());
    let new = FakeReceiver::spawn(Default::default());
    let names = Names::default();
    names
        .lock()
        .unwrap()
        .insert("Office TV".into(), cast_at("Office TV", old.addr));
    let book = Arc::new(Book::default());
    let real = connector(&book, Vec::new(), Limits::new());
    let new_addr = new.addr;
    // At its new address it only takes 160x90.
    let connect: rejoin::Connect =
        Arc::new(move |t: &Target, c: &CastOptions, m: &MirrorOptions| {
            let sink = real(t, c, m)?;
            Ok(match t {
                Target::Cast { addr, .. } if *addr == new_addr => Box::new(Capped::new(sink)),
                _ => sink,
            })
        });
    let r = rejoin(Duration::from_secs(20), resolver(&names), connect);
    let mover = {
        let (book, names) = (book.clone(), names.clone());
        std::thread::spawn(move || {
            wait_frames(&book, "Office TV", 0, 10);
            names
                .lock()
                .unwrap()
                .insert("Office TV".into(), cast_at("Office TV", new_addr));
            old.drop_connection();
            old
        })
    };
    let t0 = Instant::now();
    let e = play(pattern_opts(&["Office TV"]), &r).unwrap_err();
    assert!(e.to_string().contains("at most 160x90"), "{e}");
    // Not retried until the window ran out.
    assert!(t0.elapsed() < Duration::from_secs(15), "{:?}", t0.elapsed());
    assert_eq!(book.connects_to("Office TV").len(), 2);
    mover.join().unwrap().shutdown();
    new.shutdown();
}

/// A sink whose receiver now takes at most 160x90.
struct Capped {
    inner: Box<dyn Sink>,
    caps: Capabilities,
}

impl Capped {
    fn new(inner: Box<dyn Sink>) -> Capped {
        let caps = Capabilities {
            max_width: 160,
            max_height: 90,
            ..inner.capabilities().clone()
        };
        Capped { inner, caps }
    }
}

impl Sink for Capped {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }
    fn start(&mut self, v: &VideoInfo, a: Option<&AudioInfo>) -> Result<()> {
        self.inner.start(v, a)
    }
    fn push(&mut self, f: &EncodedFrame) -> Result<SinkStatus> {
        self.inner.push(f)
    }
    fn wants_keyframe(&self) -> bool {
        self.inner.wants_keyframe()
    }
    fn stop(&mut self) -> Result<()> {
        self.inner.stop()
    }
}

/// An AirPlay receiver goes off the network and turns up at another
/// address under its name: pair-verify, SETUP again, the codec packet and
/// an IDR first, on the session's timeline.
#[test]
fn airplay_receiver_vanishes_and_rejoins() {
    let old = Arc::new(FakeMirror::start(Behaviour {
        nack_after: None,
        ..Behaviour::default()
    }));
    let new = Arc::new(FakeMirror::start(Behaviour {
        nack_after: None,
        ..Behaviour::default()
    }));
    let names = Names::default();
    names
        .lock()
        .unwrap()
        .insert("Den".into(), airplay_at("Den", old.addr));
    let book = Arc::new(Book::default());
    let r = rejoin(
        Duration::from_secs(20),
        resolver(&names),
        connector(
            &book,
            vec![old.clone(), new.clone()],
            Limits::from([(new.addr, Some(20))]),
        ),
    );
    let mover = {
        let (book, names, old, new_addr) = (book.clone(), names.clone(), old.clone(), new.addr);
        std::thread::spawn(move || {
            wait_frames(&book, "Den", 0, 10);
            names
                .lock()
                .unwrap()
                .insert("Den".into(), airplay_at("Den", new_addr));
            old.vanish();
        })
    };
    play(pattern_opts(&["airplay:Den"]), &r).unwrap();
    mover.join().unwrap();
    assert_eq!(book.connects_to("Den"), [old.addr, new.addr]);
    assert_rejoined_on_the_timeline(&book.frames("Den"));
    let rec = new.rec();
    assert_eq!(rec.video[0].kind, 1, "codec packet first");
    let first = rec.video.iter().find(|p| p.kind == 0).unwrap();
    assert!(first.idr);
    assert!(rec.teardown);
}

/// The Apple TV user stops mirroring (the receiver closes the stream):
/// not reconnected; the Cast TV streams on.
#[test]
fn airplay_receiver_ending_mirroring_is_not_reconnected() {
    let fake = Arc::new(FakeMirror::start(Behaviour {
        close_after_packets: Some(8),
        nack_after: None,
        ..Behaviour::default()
    }));
    let rx = FakeReceiver::spawn(Default::default());
    let book = Arc::new(Book::default());
    let r = rejoin(
        Duration::from_secs(20),
        resolver(&Names::default()),
        connector(
            &book,
            vec![fake.clone()],
            Limits::from([(rx.addr, Some(60))]),
        ),
    );
    let airplay = format!("airplay:{}", fake.addr);
    let cast_name = rx.addr.to_string();
    play(pattern_opts(&[&cast_name, &airplay]), &r).unwrap();
    assert_eq!(book.connects_to(&fake.addr.to_string()), [fake.addr]);
    let n = book.frames(&cast_name).len();
    assert!((30..=60).contains(&n), "{n}");
    rx.shutdown();
}
