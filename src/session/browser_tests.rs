//! `share` with the real capture script in real browsers (opt-in,
//! `--ignored`): headless Chrome driven over the DevTools protocol, and
//! Safari over WebDriver (`safaridriver`). To fake Cast + AirPlay
//! receivers by default, or to a real one.
//!
//! ```sh
//! TVPIPE_TEST_CHROME=/path/to/chrome cargo test --release chrome_share -- --ignored --nocapture
//! TVPIPE_TEST_SAFARI=1 cargo test --release safari_share -- --ignored --nocapture
//! ```
//!
//! Options (both): `TVPIPE_TEST_BROWSER_SECS` (streaming time, default 6),
//! `_TO=ADDR` (a real receiver), `_SIZE=WxH` (`--size`), `_DUMP=PATH`
//! (what the fake Cast receiver got: H.264 Annex B, and `PATH.aac`),
//! `_DEBUG=1` (debug logs). Chrome: `TVPIPE_TEST_CHROME_VIEWPORT=WxH`
//! (default 1920x1080), `TVPIPE_TEST_CHROME_HEADED=1`,
//! `TVPIPE_TEST_CHROME_NO_MSTP=1` (the `<video>` path of Firefox).
//!
//! Safari cannot be made to accept the screen-sharing picker, so it
//! shares a synthetic stream (an animated 16:10 canvas and a tone)
//! through the same `share()`; everything after getDisplayMedia is the
//! real path. Safari needs Develop > Allow Remote Automation (once).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::tests::{fake_airplay, SharedTally, Tallied};
use super::*;
use crate::airplay::fake_mirror::{Behaviour, FakeMirror};
use crate::airplay::session::MirrorOptions;
use crate::cast::fake_receiver::{FakeReceiver, Options as CastFake};
use crate::codec::json::Value;
use crate::net::http::{Conn, Request, Version};

type Report = Arc<Mutex<Vec<String>>>;

fn env(name: &str) -> Option<String> {
    std::env::var(format!("TVPIPE_TEST_BROWSER_{name}")).ok()
}

fn secs() -> u64 {
    env("SECS").and_then(|s| s.parse().ok()).unwrap_or(6)
}

/// One browser test at a time (they share the stop flag).
static SERIAL: Mutex<()> = Mutex::new(());
static STOP: AtomicBool = AtomicBool::new(false);

fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Runs `share` while `drive(url, report)` works the browser on another
/// thread; the session stops when `drive` returns.
fn share_in_browser(drive: impl FnOnce(String, Report) + Send + 'static) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    STOP.store(false, Ordering::SeqCst);
    if env("DEBUG").is_some() {
        crate::log::set_level(crate::log::Level::Debug);
    }
    let rx = FakeReceiver::spawn(CastFake::default());
    let fake = FakeMirror::start(Behaviour {
        nack_after: None,
        ..Behaviour::default()
    });
    let (cast_tally, airplay_tally) = (SharedTally::default(), SharedTally::default());
    let connect = |t: &Target, c: &CastOptions, m: &MirrorOptions| -> Result<Box<dyn Sink>> {
        Ok(match t {
            Target::Browser(_) => connect_one(t, c, m)?,
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
    let report: Report = Arc::default();
    let driver = Mutex::new(None);
    let drive = Mutex::new(Some(drive));
    let hook = |server: &ShareServer| {
        let url = server.url().to_owned();
        let report = report.clone();
        let drive = drive.lock().unwrap().take().unwrap();
        *driver.lock().unwrap() = Some(std::thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drive(url, report)));
            STOP.store(true, Ordering::SeqCst);
            r
        }));
    };
    let real = env("TO");
    let to = match &real {
        Some(addr) => vec![addr.clone()],
        None => vec![rx.addr.to_string(), format!("airplay:{}", fake.addr)],
    };
    let o = ShareOptions {
        to,
        latency_ms: None,
        size: env("SIZE").map(|s| s.parse().unwrap()),
        fps: None,
        bitrate_kbps: None,
        no_audio: false,
        port: 0,
        open: false,
    };
    let r = if real.is_some() {
        share_with(o, &connect_one, stop_requested, &hook)
    } else {
        share_with(o, &connect, stop_requested, &hook)
    };
    let drove = driver.lock().unwrap().take().map(|h| h.join());
    for line in report.lock().unwrap().iter() {
        eprintln!("page: {line}");
    }
    if let Some(Ok(Err(panic))) = drove {
        std::panic::resume_unwind(panic);
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
    for s in &st.streams {
        eprintln!("cast stream {}: {} frames", s.codec, s.frames.len());
    }
    if let Some(path) = env("DUMP") {
        let all: Vec<u8> = v.frames.values().flat_map(|f| f.data.clone()).collect();
        std::fs::write(&path, all).unwrap();
        if let Some(a) = st.streams.iter().find(|s| s.codec == "aac") {
            let adts: Vec<u8> = a.frames.values().flat_map(|f| f.data.clone()).collect();
            std::fs::write(format!("{path}.aac"), adts).unwrap();
        }
        eprintln!("wrote {path}");
    }
    assert!(tc.video > secs() * 10, "{tc:?}");
    assert!(ta.audio > 0 && tc.audio > 0, "{tc:?} {ta:?}");
}

/// The page's letterbox arithmetic must be the Rust one.
fn check_place(eval: &mut dyn FnMut(&str) -> Value) {
    for (sw, sh, dw, dh) in [
        (1920u32, 1200u32, 1920u32, 1080u32),
        (800, 600, 1066, 600),
        (1920, 800, 1440, 1080),
        (333, 777, 1920, 1080),
        (1919, 1201, 1280, 720),
        (1, 99999, 2, 2),
    ] {
        let js = eval(&format!(
            "return JSON.stringify(tvpipeCapture.place({sw}, {sh}, {dw}, {dh}))"
        ));
        let r = crate::browser::layout::place((sw, sh), (dw, dh));
        let rust = format!(
            r#"{{"x":{},"y":{},"width":{},"height":{}}}"#,
            r.x, r.y, r.width, r.height
        );
        assert_eq!(js.as_str(), Some(rust.as_str()), "{sw}x{sh} in {dw}x{dh}");
    }
}

/// Streams for a while, sampling the page's stats; asks for a keyframe
/// halfway and checks the encoder made one.
fn watch(eval: &mut dyn FnMut(&str) -> Value, report: &Report) {
    let t0 = Instant::now();
    let total = Duration::from_secs(secs());
    let mut asked = false;
    while t0.elapsed() < total {
        std::thread::sleep(Duration::from_millis(1000));
        if !asked && t0.elapsed() >= total / 2 {
            asked = true;
            eval("tvpipeShare._message({type: 'keyframe'}); return 1");
        }
        let s = eval(
            "return JSON.stringify({stats: window.tvpipeLastStats || null, \
             log: document.getElementById('log').innerText, \
             capture: document.getElementById('capture').innerText, \
             plan: document.getElementById('plan').innerText})",
        );
        report
            .lock()
            .unwrap()
            .push(s.as_str().unwrap_or("?").to_owned());
    }
    // Stats come every 2 s: wait for the one covering the request.
    std::thread::sleep(Duration::from_millis(2200));
    let s = eval("return JSON.stringify(window.tvpipeKeyLatency === undefined ? null : window.tvpipeKeyLatency)");
    report
        .lock()
        .unwrap()
        .push(format!("keyframe request answered in {s} ms"));
    assert!(
        s.as_str().is_some_and(|v| v != "null"),
        "the keyframe request was not answered"
    );
}

// ---------------------------------------------------------------------------
// Chrome (DevTools protocol)

/// A minimal DevTools-protocol client over our WebSocket (enough to drive
/// the share page; W13 owns the real one).
pub(super) struct Cdp {
    rx: crate::net::websocket::Receiver<std::net::TcpStream, std::net::TcpStream>,
    tx: crate::net::websocket::Sender<std::net::TcpStream>,
    id: i64,
}

impl Cdp {
    pub(super) fn call(&mut self, method: &str, params: Value) -> Value {
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

    /// Evaluates a function body as if the user clicked (transient
    /// activation); awaits a returned promise.
    pub(super) fn eval(&mut self, body: &str) -> Value {
        let r = self.call(
            "Runtime.evaluate",
            Value::object()
                .with("expression", format!("(async () => {{ {body} }})()"))
                .with("awaitPromise", true)
                .with("userGesture", true)
                .with("returnByValue", true),
        );
        r.get("result")
            .and_then(|r| r.get("result"))
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or_else(|| {
                eprintln!("eval {body}: {r}");
                Value::Null
            })
    }
}

/// TVPIPE_TEST_CHROME_VIEWPORT=WxH, default 1920x1080.
fn viewport() -> (u32, u32) {
    let v: Size = std::env::var("TVPIPE_TEST_CHROME_VIEWPORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(Size {
            width: 1920,
            height: 1080,
        });
    (v.width, v.height)
}

/// Starts Chrome on `url` (headless, tab capture auto-accepted) and
/// connects to the page's DevTools target.
pub(super) fn chrome(bin: &str, url: &str, dir: &std::path::Path) -> (std::process::Child, Cdp) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let headless = std::env::var("TVPIPE_TEST_CHROME_HEADED").is_err();
    let (vw, vh) = viewport();
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
        // A WxH viewport at scale 1 on a WxH screen (headless tab capture
        // is clamped to the screen, 800x600 by default; the window has 87
        // px of chrome above the content).
        .args([
            format!("--window-size={vw},{}", vh + 87),
            format!("--screen-info={{0,0 {vw}x{vh}}}"),
            "--force-device-scale-factor=1".into(),
        ])
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

/// The real capture script in headless Chrome: tab capture of the share
/// page itself (an animation and a tone injected).
#[test]
#[ignore]
fn chrome_share_to_fakes() {
    let Ok(bin) = std::env::var("TVPIPE_TEST_CHROME") else {
        eprintln!("TVPIPE_TEST_CHROME not set");
        return;
    };
    share_in_browser(move |url, report| {
        let dir = std::env::temp_dir().join(format!("tvpipe-chrome-{}", std::process::id()));
        let (mut child, mut cdp) = chrome(&bin, &url, &dir);
        let mut eval = |s: &str| cdp.eval(s);
        let deadline = Instant::now() + Duration::from_secs(20);
        while eval("return !!(window.tvpipeShare && window.tvpipeShare.config)")
            != Value::Bool(true)
        {
            assert!(Instant::now() < deadline, "the page got no config");
            std::thread::sleep(Duration::from_millis(100));
        }
        let geo = eval(
            "return JSON.stringify([innerWidth, innerHeight, outerWidth, outerHeight, \
             devicePixelRatio, screen.width, screen.height])",
        );
        report.lock().unwrap().push(format!("geometry: {geo}"));
        let (vw, vh) = viewport();
        let g = geo.as_str().unwrap_or("");
        assert!(
            g.starts_with(&format!("[{vw},{vh},")),
            "viewport is not {vw}x{vh}: {geo}"
        );
        assert!(g.contains(",1,"), "scale: {geo}");
        check_place(&mut eval);
        // Something that moves and something that sounds.
        eval(
            "const c = document.createElement('canvas'); c.width = 640; c.height = 200; \
             document.body.prepend(c); const g = c.getContext('2d'); let n = 0; \
             (function draw() { g.fillStyle = `hsl(${n % 360},80%,50%)`; g.fillRect(0, 0, 640, 200); \
             g.fillStyle = '#fff'; g.font = '48px sans-serif'; g.fillText(String(n++), 20, 120); \
             requestAnimationFrame(draw); })(); \
             const a = new AudioContext(); const o = a.createOscillator(); o.frequency.value = 440; \
             o.connect(a.destination); o.start(); return a.state;",
        );
        if std::env::var("TVPIPE_TEST_CHROME_NO_MSTP").is_ok() {
            eval("window.MediaStreamTrackProcessor = undefined; tvpipeShare.opts.worker = false; return 1");
        }
        let r = eval(
            "tvpipeShare.opts.preferCurrentTab = true; tvpipeShare.opts.selfBrowserSurface = 'include'; \
             return tvpipeShare.captureDisplay().then(() => 'ok', e => 'error: ' + e.name + ': ' + e.message)",
        );
        report.lock().unwrap().push(format!("captureDisplay: {r}"));
        watch(&mut eval, &report);
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    });
}

// ---------------------------------------------------------------------------
// Safari (WebDriver)

pub(super) struct WebDriver {
    port: u16,
    session: String,
    child: std::process::Child,
}

pub(super) fn webdriver_call(port: u16, method: &str, path: &str, body: Option<&Value>) -> Value {
    let tcp = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut c = Conn::new(tcp);
    let mut req = Request::new(method, path, Version::Http11)
        .header("Host", format!("127.0.0.1:{port}"))
        .header("Connection", "close");
    if let Some(b) = body {
        req = req.body("application/json", b.to_json().into_bytes());
    }
    let resp = c.request(&req).unwrap();
    let v = Value::parse(std::str::from_utf8(&resp.body).unwrap()).unwrap();
    v.get("value").cloned().unwrap_or(Value::Null)
}

impl WebDriver {
    pub(super) fn start() -> WebDriver {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let child = std::process::Command::new("safaridriver")
            .args(["-p", &port.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("safaridriver");
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "safaridriver did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        let caps = Value::object().with(
            "capabilities",
            Value::object().with(
                "alwaysMatch",
                Value::object()
                    .with("browserName", "safari")
                    .with("webkit:alwaysAllowAutoplay", true),
            ),
        );
        let v = webdriver_call(port, "POST", "/session", Some(&caps));
        let session = v
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("no WebDriver session (Develop > Allow Remote Automation?): {v}")
            })
            .to_owned();
        WebDriver {
            port,
            session,
            child,
        }
    }

    pub(super) fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Value {
        webdriver_call(
            self.port,
            method,
            &format!("/session/{}{path}", self.session),
            body,
        )
    }

    /// Runs a function body (`return` a value; promises are awaited).
    pub(super) fn eval(&self, body: &str) -> Value {
        let script = format!(
            "const done = arguments[arguments.length - 1]; \
             (async () => {{ {body} }})().then(done, e => done('error: ' + e));"
        );
        let b = Value::object()
            .with("script", script)
            .with("args", Value::array(Vec::<Value>::new()));
        self.call("POST", "/execute/async", Some(&b))
    }
}

impl Drop for WebDriver {
    fn drop(&mut self) {
        webdriver_call(
            self.port,
            "DELETE",
            &format!("/session/{}", self.session),
            None,
        );
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The real capture script in Safari, sharing a synthetic 16:10 stream
/// (getDisplayMedia needs a human): the worker engine, Safari's H.264
/// encoder, the padding, the audio worklet, keyframe requests.
#[test]
#[ignore]
fn safari_share_to_fakes() {
    if std::env::var("TVPIPE_TEST_SAFARI").is_err() {
        eprintln!("TVPIPE_TEST_SAFARI not set");
        return;
    }
    share_in_browser(move |url, report| {
        let wd = WebDriver::start();
        wd.call(
            "POST",
            "/url",
            Some(&Value::object().with("url", url.as_str())),
        );
        let mut eval = |s: &str| wd.eval(s);
        let deadline = Instant::now() + Duration::from_secs(20);
        while eval("return !!(window.tvpipeShare && window.tvpipeShare.config)")
            != Value::Bool(true)
        {
            assert!(Instant::now() < deadline, "the page got no config");
            std::thread::sleep(Duration::from_millis(100));
        }
        let f = eval("return JSON.stringify(tvpipeCapture.features())");
        report.lock().unwrap().push(format!("features: {f}"));
        let howto = eval("return document.getElementById('howto').hidden ? '' : document.getElementById('howto').innerText");
        report
            .lock()
            .unwrap()
            .push(format!("how-to shown: {howto}"));
        assert!(
            howto
                .as_str()
                .is_some_and(|h| h.contains("not a single tab")),
            "Safari's how-to is missing: {howto}"
        );
        check_place(&mut eval);
        let r = eval(
            "const c = document.createElement('canvas'); c.width = 1280; c.height = 800; \
             document.body.prepend(c); const g = c.getContext('2d'); let n = 0; \
             (function draw() { g.fillStyle = `hsl(${n % 360},80%,50%)`; g.fillRect(0, 0, 1280, 800); \
             g.fillStyle = '#fff'; g.font = '96px sans-serif'; g.fillText(String(n++), 40, 200); \
             requestAnimationFrame(draw); })(); \
             const a = new AudioContext({sampleRate: tvpipeShare.config.sampleRate}); await a.resume(); \
             const o = a.createOscillator(); o.frequency.value = 440; \
             const d = a.createMediaStreamDestination(); o.connect(d); o.start(); \
             tvpipeShare.audioCtx = new AudioContext({sampleRate: tvpipeShare.config.sampleRate}); \
             await tvpipeShare.audioCtx.resume(); \
             const s = new MediaStream([...c.captureStream(30).getVideoTracks(), ...d.stream.getAudioTracks()]); \
             return tvpipeShare.share(s).then(() => 'ok', e => 'error: ' + e);",
        );
        report.lock().unwrap().push(format!("share: {r}"));
        watch(&mut eval, &report);
        let mode = eval("return window.tvpipeLastStats && window.tvpipeLastStats.mode");
        report.lock().unwrap().push(format!("engine: {mode}"));
        assert_eq!(
            mode.as_str(),
            Some("worker"),
            "Safari should encode in a worker"
        );
        drop(wd);
    });
}
