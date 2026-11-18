//! `share` end to end (decision 0012, option C): a fake page (the Rust
//! one from `browser::share::tests`, built-in H.264 + PCM over a real
//! loopback WebSocket) to a fake Cast receiver and a fake AirPlay mirror
//! receiver.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread::JoinHandle;

use super::tests::{fake_airplay, never, SharedTally, Tallied};
use super::*;
use crate::airplay::fake_mirror::{Behaviour, FakeMirror};
use crate::airplay::session::MirrorOptions;
use crate::browser::share::tests::{spawn_page, PageScript, PageSeen};
use crate::cast::fake_receiver::{FakeReceiver, Options as CastFake};

fn share_opts(to: Vec<String>) -> ShareOptions {
    ShareOptions {
        to,
        latency_ms: Some(100),
        size: None,
        fps: None,
        bitrate_kbps: None,
        no_audio: false,
        port: 0,
        open: false,
    }
}

type Page = Mutex<Option<JoinHandle<PageSeen>>>;

fn page_hook(page: &Page, script: PageScript) -> impl Fn(&ShareServer) + '_ {
    move |server: &ShareServer| {
        *page.lock().unwrap() = Some(spawn_page(server.ws_url(), script.clone()));
    }
}

fn joined(page: &Page) -> PageSeen {
    page.lock().unwrap().take().unwrap().join().unwrap()
}

#[test]
fn page_to_cast_and_airplay() {
    let rx = FakeReceiver::spawn(CastFake::default());
    let fake = FakeMirror::start(Behaviour {
        nack_after: None,
        ..Behaviour::default()
    });
    let (cast_tally, airplay_tally) = (SharedTally::default(), SharedTally::default());
    let connect = |t: &Target, c: &CastOptions, m: &MirrorOptions| -> Result<Box<dyn Sink>> {
        Ok(match t {
            Target::Cast { addr, name } => {
                Tallied::boxed(Box::new(cast::connect(*addr, name, c)?), None, &cast_tally)
            }
            Target::AirPlay { .. } => {
                let m = MirrorOptions {
                    ptp: false,
                    ..m.clone()
                };
                Tallied::boxed(
                    fake_airplay(&fake, "fake-airplay", &m)?,
                    None,
                    &airplay_tally,
                )
            }
        })
    };
    let page = Page::default();
    let script = PageScript {
        frames: 45,
        // A 4:3 capture of a large screen: scaled to fit 1920x1080.
        capture: (2880, 2160),
        ..PageScript::default()
    };
    let o = ShareOptions {
        // Small, so the built-in encoder in the fake page is quick.
        size: Some(Size {
            width: 320,
            height: 240,
        }),
        ..share_opts(vec![rx.addr.to_string(), format!("airplay:{}", fake.addr)])
    };
    share_with(o, &connect, never, &page_hook(&page, script)).unwrap();
    let seen = joined(&page);

    // The page was told what to capture, then the plan.
    let config = seen.config.unwrap();
    assert_eq!(config.get("sampleRate").unwrap().as_u32(), Some(44_100));
    let names = config.get("receivers").unwrap().as_array().unwrap();
    assert_eq!(names.len(), 2);
    let plan = seen.plan.unwrap();
    assert_eq!(plan.get("width").unwrap().as_u32(), Some(320));
    assert_eq!(plan.get("height").unwrap().as_u32(), Some(240));
    assert_eq!(plan.get("fps").unwrap().as_f64(), Some(30.0));
    // AirPlay in the session: an IDR every half second.
    assert_eq!(plan.get("keyInterval").unwrap().as_u32(), Some(15));
    let audio = plan.get("audio").unwrap();
    assert_eq!(audio.get("sampleRate").unwrap().as_u32(), Some(44_100));
    assert!(
        seen.statuses.contains(&"streaming".to_owned()),
        "{:?}",
        seen.statuses
    );
    assert_eq!(seen.frames_sent, 45);

    // Cast: every frame (nothing was lost on loopback), AAC audio.
    let state = rx.shutdown();
    let st = state.lock().unwrap();
    assert!(st.stopped, "STOP not received");
    let offer = st.offer.as_ref().unwrap().to_string();
    assert!(offer.contains("\"aac\""), "{offer}");
    let v = st.streams.iter().find(|s| s.codec == "h264").unwrap();
    let a = st.streams.iter().find(|s| s.codec == "aac").unwrap();
    let (tc, ta) = (*cast_tally.lock().unwrap(), *airplay_tally.lock().unwrap());
    assert_eq!(tc.video, 45, "{tc:?}");
    super::tests::assert_cast_frames(&v.frames, tc.video_accepted);
    assert!(!a.frames.is_empty());
    // AirPlay: video and PCM audio.
    assert_eq!(ta.video, 45, "{ta:?}");
    let r = fake.rec();
    assert!(r.teardown, "TEARDOWN not received");
    assert!(!r.audio.is_empty(), "AirPlay audio");
    assert_eq!(
        r.video.iter().filter(|p| p.kind == 0).count() as u64,
        ta.video_accepted
    );
}

static STOP: AtomicBool = AtomicBool::new(false);

fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Ctrl-C while streaming: the receiver gets its STOP and the page is
/// told that tvpipe stopped.
#[test]
fn stop_tells_the_page() {
    let rx = FakeReceiver::spawn(CastFake::default());
    let tally = SharedTally::default();
    let connect = |t: &Target, c: &CastOptions, _: &MirrorOptions| -> Result<Box<dyn Sink>> {
        let Target::Cast { addr, name } = t else {
            panic!("cast only")
        };
        Ok(Tallied::boxed(
            Box::new(cast::connect(*addr, name, c)?),
            None,
            &tally,
        ))
    };
    let page = Page::default();
    let script = PageScript {
        frames: 300,
        audio: false,
        capture: (320, 180),
        ..PageScript::default()
    };
    let stopper = std::thread::spawn({
        let tally = tally.clone();
        move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while tally.lock().unwrap().video < 10 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            STOP.store(true, Ordering::SeqCst);
        }
    });
    let o = ShareOptions {
        no_audio: true,
        ..share_opts(vec![rx.addr.to_string()])
    };
    share_with(o, &connect, stop_requested, &page_hook(&page, script)).unwrap();
    stopper.join().unwrap();
    let seen = joined(&page);
    assert_eq!(seen.end.as_deref(), Some("tvpipe stopped"));
    assert!(seen.frames_sent < 300);
    assert!(seen.plan.unwrap().get("audio").unwrap().is_null());
    let state = rx.shutdown();
    assert!(state.lock().unwrap().stopped, "STOP not received");
}

/// No receiver can be reached: the page hears why.
#[test]
fn unreachable_receiver_ends_the_page() {
    let page = Page::default();
    let o = share_opts(vec!["cast:127.0.0.1:1".into()]);
    let script = PageScript {
        frames: 5,
        ..PageScript::default()
    };
    assert!(share_with(o, &connect_one, never, &page_hook(&page, script)).is_err());
    let seen = joined(&page);
    assert!(seen.plan.is_none());
    let end = seen.end.unwrap();
    assert!(end.contains("connect") || end.contains("refused"), "{end}");
}

/// A minimal DevTools-protocol client over our WebSocket (enough to drive
/// the share page in the opt-in Chrome test; W13 owns the real one).
struct Cdp {
    rx: crate::net::websocket::Receiver<std::net::TcpStream, std::net::TcpStream>,
    tx: crate::net::websocket::Sender<std::net::TcpStream>,
    id: i64,
}

impl Cdp {
    fn call(
        &mut self,
        method: &str,
        params: crate::codec::json::Value,
    ) -> crate::codec::json::Value {
        use crate::codec::json::Value;
        use crate::net::websocket::Message;
        self.id += 1;
        let msg = Value::object()
            .with("id", self.id)
            .with("method", method)
            .with("params", params);
        self.tx.send_text(&msg.to_json()).unwrap();
        loop {
            let Message::Text(t) = self.rx.read_message().unwrap() else {
                continue;
            };
            let v = Value::parse(&t).unwrap();
            if v.get("id").and_then(Value::as_i64) == Some(self.id) {
                return v;
            }
        }
    }

    /// Evaluates `expr` as if the user clicked (transient activation).
    fn eval(&mut self, expr: &str) -> crate::codec::json::Value {
        use crate::codec::json::Value;
        let r = self.call(
            "Runtime.evaluate",
            Value::object()
                .with("expression", expr)
                .with("awaitPromise", true)
                .with("userGesture", true)
                .with("returnByValue", true),
        );
        r.get("result")
            .and_then(|r| r.get("result"))
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or_else(|| {
                eprintln!("eval {expr}: {r}");
                Value::Null
            })
    }
}

/// Starts Chrome on `url` (headless, tab capture auto-accepted) and
/// connects to the page's DevTools target.
fn chrome(bin: &str, url: &str, dir: &std::path::Path) -> (std::process::Child, Cdp) {
    use crate::codec::json::Value;
    use crate::net::http::{Conn, Request, Version};
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let headless = std::env::var("TVPIPE_TEST_CHROME_HEADED").is_err();
    let mut cmd = std::process::Command::new(bin);
    if headless {
        cmd.arg("--headless=new");
    }
    let child = cmd
        .arg("--remote-debugging-port=0")
        .arg(format!("--user-data-dir={}", dir.display()))
        .args(["--no-first-run", "--no-default-browser-check"])
        .args([
            "--auto-accept-this-tab-capture",
            "--autoplay-policy=no-user-gesture-required",
        ])
        .arg("--window-size=1280,720")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let port_file = dir.join("DevToolsActivePort");
    let deadline = Instant::now() + Duration::from_secs(20);
    let port: u16 = loop {
        if let Some(p) = std::fs::read_to_string(&port_file)
            .ok()
            .and_then(|s| s.lines().next().and_then(|l| l.parse().ok()))
        {
            break p;
        }
        assert!(Instant::now() < deadline, "Chrome did not start");
        std::thread::sleep(Duration::from_millis(50));
    };
    let ws = loop {
        let tcp = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut c = Conn::new(tcp);
        let resp = c
            .request(
                &Request::new("GET", "/json/list", Version::Http11)
                    .header("Host", format!("127.0.0.1:{port}")),
            )
            .unwrap();
        let list = Value::parse(std::str::from_utf8(&resp.body).unwrap()).unwrap();
        let found = list.as_array().unwrap().iter().find_map(|t| {
            let u = t.get("url")?.as_str()?;
            (t.get("type")?.as_str()? == "page" && u.starts_with("http://127.0.0.1"))
                .then(|| t.get("webSocketDebuggerUrl")?.as_str().map(str::to_owned))?
        });
        if let Some(ws) = found {
            break ws;
        }
        assert!(Instant::now() < deadline, "no page target");
        std::thread::sleep(Duration::from_millis(100));
    };
    let (rx, tx) = crate::net::websocket::connect(
        &ws,
        Duration::from_secs(10),
        crate::net::websocket::Limits::default(),
    )
    .unwrap();
    (child, Cdp { rx, tx, id: 0 })
}

static CHROME_STOP: AtomicBool = AtomicBool::new(false);

fn chrome_stop() -> bool {
    CHROME_STOP.load(Ordering::SeqCst)
}

/// The real capture script in a real Chrome (headless, tab capture of
/// the share page itself with an animation and a tone injected), to a
/// fake Cast receiver and a fake AirPlay receiver.
///
/// `TVPIPE_TEST_CHROME=/path/to/chrome cargo test chrome_share -- --ignored --nocapture`
/// (`TVPIPE_TEST_CHROME_HEADED=1` shows the window).
#[test]
#[ignore]
fn chrome_share_to_fakes() {
    let Ok(bin) = std::env::var("TVPIPE_TEST_CHROME") else {
        eprintln!("TVPIPE_TEST_CHROME not set");
        return;
    };
    if std::env::var("TVPIPE_TEST_CHROME_DEBUG").is_ok() {
        crate::log::set_level(crate::log::Level::Debug);
    }
    let secs: u64 = std::env::var("TVPIPE_TEST_CHROME_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    let rx = FakeReceiver::spawn(CastFake::default());
    let fake = FakeMirror::start(Behaviour {
        nack_after: None,
        ..Behaviour::default()
    });
    let (cast_tally, airplay_tally) = (SharedTally::default(), SharedTally::default());
    let connect = |t: &Target, c: &CastOptions, m: &MirrorOptions| -> Result<Box<dyn Sink>> {
        Ok(match t {
            Target::Cast { addr, name } => {
                Tallied::boxed(Box::new(cast::connect(*addr, name, c)?), None, &cast_tally)
            }
            Target::AirPlay { .. } => {
                let m = MirrorOptions {
                    ptp: false,
                    ..m.clone()
                };
                Tallied::boxed(
                    fake_airplay(&fake, "fake-airplay", &m)?,
                    None,
                    &airplay_tally,
                )
            }
        })
    };
    let driver: Page = Mutex::new(None);
    let report = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
    let dir = std::env::temp_dir().join(format!("tvpipe-chrome-{}", std::process::id()));
    let hook = |server: &ShareServer| {
        let url = server.url().to_owned();
        let (bin, dir) = (bin.clone(), dir.clone());
        let report = report.clone();
        *driver.lock().unwrap() = Some(std::thread::spawn(move || {
            let (mut child, mut cdp) = chrome(&bin, &url, &dir);
            let deadline = Instant::now() + Duration::from_secs(20);
            while cdp.eval("!!(window.tvpipeShare && window.tvpipeShare.config)")
                != crate::codec::json::Value::Bool(true)
            {
                assert!(Instant::now() < deadline, "the page got no config");
                std::thread::sleep(Duration::from_millis(100));
            }
            // Something that moves and something that sounds.
            cdp.eval(
                "(() => { const c = document.createElement('canvas'); c.width = 640; c.height = 200; \
                 document.body.prepend(c); const g = c.getContext('2d'); let n = 0; \
                 (function draw() { g.fillStyle = `hsl(${n % 360},80%,50%)`; g.fillRect(0, 0, 640, 200); \
                 g.fillStyle = '#fff'; g.font = '48px sans-serif'; g.fillText(String(n++), 20, 120); \
                 requestAnimationFrame(draw); })(); \
                 const a = new AudioContext(); const o = a.createOscillator(); o.frequency.value = 440; \
                 o.connect(a.destination); o.start(); return a.state; })()",
            );
            if std::env::var("TVPIPE_TEST_CHROME_NO_MSTP").is_ok() {
                // The <video> path Safari and Firefox take.
                cdp.eval("window.MediaStreamTrackProcessor = undefined; 1");
            }
            let r = cdp.eval(
                "tvpipeShare.opts.preferCurrentTab = true; tvpipeShare.opts.selfBrowserSurface = 'include'; \
                 tvpipeShare.captureDisplay().then(() => 'ok', e => 'error: ' + e.name + ': ' + e.message)",
            );
            report.lock().unwrap().push(format!("captureDisplay: {r}"));
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(secs) {
                std::thread::sleep(Duration::from_millis(2100));
                let s = cdp.eval(
                    "JSON.stringify({log: document.getElementById('log').innerText, \
                     fps: document.getElementById('fps').innerText, kbps: document.getElementById('kbps').innerText, \
                     delay: document.getElementById('delay').innerText, codec: document.getElementById('codec').innerText, \
                     capture: document.getElementById('capture').innerText, plan: document.getElementById('plan').innerText})",
                );
                report
                    .lock()
                    .unwrap()
                    .push(s.as_str().unwrap_or("?").to_owned());
            }
            CHROME_STOP.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(500));
            let _ = child.kill();
            let _ = child.wait();
            PageSeen::default()
        }));
    };
    // TVPIPE_TEST_CHROME_TO: a real receiver instead of the fakes.
    let real = std::env::var("TVPIPE_TEST_CHROME_TO").ok();
    let to = match &real {
        Some(name) => vec![name.clone()],
        None => vec![rx.addr.to_string(), format!("airplay:{}", fake.addr)],
    };
    let o = ShareOptions {
        latency_ms: None,
        size: std::env::var("TVPIPE_TEST_CHROME_SIZE")
            .ok()
            .map(|s| s.parse().unwrap()),
        ..share_opts(to)
    };
    let r = if real.is_some() {
        share_with(o, &connect_one, chrome_stop, &hook)
    } else {
        share_with(o, &connect, chrome_stop, &hook)
    };
    joined(&driver);
    let _ = std::fs::remove_dir_all(&dir);
    for line in report.lock().unwrap().iter() {
        eprintln!("page: {line}");
    }
    r.unwrap();
    if real.is_some() {
        return;
    }
    let state = rx.shutdown();
    let st = state.lock().unwrap();
    let (tc, ta) = (*cast_tally.lock().unwrap(), *airplay_tally.lock().unwrap());
    eprintln!("cast {tc:?}\nairplay {ta:?}");
    let v = st.streams.iter().find(|s| s.codec == "h264").unwrap();
    eprintln!("cast receiver: {} video frames", v.frames.len());
    for s in &st.streams {
        eprintln!("cast stream {}: {} frames", s.codec, s.frames.len());
    }
    if let Ok(path) = std::env::var("TVPIPE_TEST_CHROME_DUMP") {
        // Annex B as the Cast receiver got it (decode with ffmpeg).
        let all: Vec<u8> = v.frames.values().flat_map(|f| f.data.clone()).collect();
        std::fs::write(&path, all).unwrap();
        if let Some(a) = st.streams.iter().find(|s| s.codec == "aac") {
            let adts: Vec<u8> = a.frames.values().flat_map(|f| f.data.clone()).collect();
            std::fs::write(format!("{path}.aac"), adts).unwrap();
        }
        eprintln!("wrote {path}");
    }
    assert!(tc.video > secs * 10, "{tc:?}");
    assert!(ta.audio > 0 && tc.audio > 0, "{tc:?} {ta:?}");
}
