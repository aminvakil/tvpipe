//! `tvpipe bundle --smoke`: runs a game the way `tvpipe playshare` does,
//! without TVs or a tunnel, and watches its pages in a headless browser
//! of tvpipe's own (Chromium-based, over DevTools):
//!
//! 1. the launcher (the start URL sets the session cookie), then the host
//!    page with a pretend receiver chosen; the host page must attach the
//!    relay;
//! 2. the TV page the host page casts (the cast goes to a stand-in that
//!    records the page), opened in a tab as the headless cast would;
//! 3. the phone page through the phones' listener, at the join link the
//!    TV page's view carries (a screen client asks for it);
//! 4. a fake phone joins over the relay (the party protocol's `hello`)
//!    and must be welcomed.
//!
//! Uncaught exceptions, failed requests (HTTP ≥ 400, network errors) and
//! steps that do not happen are errors; `console.error` and requests to
//! the internet that fail are warnings. Then everything is shut down.

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::backend::{Backend, Cast, Discover, Pair};
use super::check::{Level, Problem};
use super::{Bundle, Hub, Options, Running, TunnelChoice};
use crate::browser::launch::{find_browser, Browser, Engine, LaunchOptions, Profile};
use crate::codec::json::Value;
use crate::error::{Error, Result};
use crate::net::websocket::{self, Message, Role, WsUrl};
use crate::util::stop::Stop;

/// The pretend receiver's name.
pub const RECEIVER: &str = "tvpipe-smoke";

/// What `--smoke` takes.
#[derive(Debug, Clone)]
pub struct SmokeOptions {
    /// The browser (default: the one `playshare` finds).
    pub browser: Option<PathBuf>,
    /// How long each step may take.
    pub step: Duration,
}

impl Default for SmokeOptions {
    fn default() -> SmokeOptions {
        SmokeOptions {
            browser: None,
            step: Duration::from_secs(15),
        }
    }
}

/// What the smoke run found: problems by page, and what worked.
#[derive(Debug, Default)]
pub struct SmokeReport {
    pub problems: Vec<Problem>,
    pub passed: Vec<String>,
}

impl SmokeReport {
    pub fn errors(&self) -> usize {
        self.problems
            .iter()
            .filter(|p| p.level == Level::Error)
            .count()
    }

    fn add(&mut self, level: Level, at: &str, message: impl Into<String>) {
        let p = Problem {
            level,
            at: at.to_owned(),
            message: message.into(),
        };
        if !self.problems.contains(&p) {
            self.problems.push(p);
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for p in &self.passed {
            out.push_str(&format!("smoke: {p}\n"));
        }
        for p in &self.problems {
            out.push_str(&format!("smoke {p}\n"));
        }
        out
    }
}

/// The stand-in backend: one pretend receiver, casts recorded.
struct Stand {
    cast: Arc<Mutex<Option<String>>>,
}

struct NoDiscover;
impl Discover for NoDiscover {
    fn devices(&self) -> Vec<Value> {
        vec![Value::object()
            .with("name", RECEIVER)
            .with("kind", "cast")
            .with("needsPairing", false)]
    }
}

struct Recorded;
impl Cast for Recorded {
    fn status(&mut self) -> Value {
        Value::object().with("state", "streaming").with(
            "receivers",
            vec![Value::object()
                .with("name", RECEIVER)
                .with("state", "streaming")],
        )
    }
    fn stop(self: Box<Self>) {}
}

impl Backend for Stand {
    fn discover(&self) -> Result<Box<dyn Discover>> {
        Ok(Box::new(NoDiscover))
    }
    fn pair(&self, _device: &str) -> Result<Box<dyn Pair>> {
        Err(Error::unsupported("no pairing in a smoke run"))
    }
    fn cast(&self, url: &str, _to: &[String], _size: Option<(u32, u32)>) -> Result<Box<dyn Cast>> {
        *self.cast.lock().unwrap_or_else(|e| e.into_inner()) = Some(url.to_owned());
        Ok(Box::new(Recorded))
    }
}

/// One tab: its DevTools session and label.
struct Tab {
    session: String,
    label: &'static str,
    /// Errors here are warnings (a page opened without its link).
    lenient: bool,
}

struct Smoke {
    browser: Browser,
    _profile: Profile,
    tabs: Vec<Tab>,
    /// Request ids to URLs, per session.
    requests: std::collections::HashMap<(String, String), String>,
    local: String,
    report: SmokeReport,
}

impl Smoke {
    fn tab(&mut self, label: &'static str, url: &str, lenient: bool) -> Result<usize> {
        let cdp = self.browser.cdp();
        let target = cdp
            .call(
                None,
                "Target.createTarget",
                Value::object().with("url", "about:blank"),
            )?
            .require_str("targetId")?
            .to_owned();
        let session = cdp
            .call(
                None,
                "Target.attachToTarget",
                Value::object()
                    .with("targetId", target.as_str())
                    .with("flatten", true),
            )?
            .require_str("sessionId")?
            .to_owned();
        for m in [
            "Page.enable",
            "Runtime.enable",
            "Network.enable",
            "Log.enable",
        ] {
            cdp.call(Some(&session), m, Value::object())?;
        }
        self.tabs.push(Tab {
            session,
            label,
            lenient,
        });
        let i = self.tabs.len() - 1;
        self.navigate(i, url)?;
        Ok(i)
    }

    fn navigate(&mut self, i: usize, url: &str) -> Result<()> {
        let s = self.tabs[i].session.clone();
        let r =
            self.browser
                .cdp()
                .call(Some(&s), "Page.navigate", Value::object().with("url", url))?;
        if let Some(e) = r.get("errorText").and_then(Value::as_str) {
            let label = self.tabs[i].label;
            self.report.add(
                Level::Error,
                label,
                format!("could not open {}: {e}", self.short(url)),
            );
        }
        Ok(())
    }

    /// A URL for messages: the local origin and secrets left out.
    fn short(&self, url: &str) -> String {
        let u = url.strip_prefix(&self.local).unwrap_or(url);
        // The phones' listener too.
        let u = match u.strip_prefix("http://127.0.0.1:") {
            Some(rest) => rest.find('/').map_or(u, |i| &rest[i..]),
            None => u,
        };
        let u = u.split('#').next().unwrap_or(u);
        match u.split_once("t=") {
            Some((a, _)) => format!("{a}t=…"),
            None => u.to_owned(),
        }
    }

    /// Reads browser events for `d` (or until `until` holds), noting
    /// problems.
    fn pump(&mut self, d: Duration, until: &mut dyn FnMut(&mut Smoke) -> bool) -> bool {
        let deadline = Instant::now() + d;
        loop {
            if until(self) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            match self
                .browser
                .cdp()
                .next_event(left.min(Duration::from_millis(100)))
            {
                Ok(Some(ev)) => self.event(&ev),
                Ok(None) => {}
                Err(e) => {
                    self.report.add(
                        Level::Error,
                        "browser",
                        format!("the browser went away: {e}"),
                    );
                    return false;
                }
            }
        }
    }

    fn event(&mut self, ev: &crate::browser::cdp::Event) {
        let Some(s) = &ev.session else { return };
        let Some(tab) = self.tabs.iter().find(|t| &t.session == s) else {
            return;
        };
        let (label, lenient) = (tab.label, tab.lenient);
        let level = |l: Level| if lenient { Level::Warning } else { l };
        let p = &ev.params;
        match ev.method.as_str() {
            "Runtime.exceptionThrown" => {
                let d = p.get("exceptionDetails").cloned().unwrap_or(Value::Null);
                let text = d
                    .get("exception")
                    .and_then(|e| e.get("description"))
                    .and_then(Value::as_str)
                    .or_else(|| d.get("text").and_then(Value::as_str))
                    .unwrap_or("an exception");
                let first = text.lines().next().unwrap_or(text).to_owned();
                let at = match (
                    d.get("url").and_then(Value::as_str),
                    d.get("lineNumber").and_then(Value::as_u64),
                ) {
                    (Some(u), Some(l)) if !u.is_empty() => {
                        format!(" ({}:{})", self.short(u), l + 1)
                    }
                    _ => String::new(),
                };
                self.report
                    .add(level(Level::Error), label, format!("uncaught {first}{at}"));
            }
            "Runtime.consoleAPICalled" => {
                if p.get("type").and_then(Value::as_str) == Some("error") {
                    let text: Vec<String> = p
                        .get("args")
                        .and_then(Value::as_array)
                        .unwrap_or(&[])
                        .iter()
                        .map(|a| {
                            a.get("value")
                                .map(|v| v.as_str().map_or_else(|| v.to_json(), str::to_owned))
                                .or_else(|| {
                                    a.get("description")
                                        .and_then(Value::as_str)
                                        .map(str::to_owned)
                                })
                                .unwrap_or_default()
                        })
                        .collect();
                    self.report.add(
                        Level::Warning,
                        label,
                        format!(
                            "console.error: {}",
                            text.join(" ").chars().take(200).collect::<String>()
                        ),
                    );
                }
            }
            "Network.requestWillBeSent" => {
                if let (Some(id), Some(url)) = (
                    p.get("requestId").and_then(Value::as_str),
                    p.get("request")
                        .and_then(|r| r.get("url"))
                        .and_then(Value::as_str),
                ) {
                    self.requests
                        .insert((s.clone(), id.to_owned()), url.to_owned());
                }
            }
            "Network.responseReceived" => {
                let r = p.get("response").cloned().unwrap_or(Value::Null);
                let status = r.get("status").and_then(Value::as_u64).unwrap_or(0);
                let url = r.get("url").and_then(Value::as_str).unwrap_or("");
                if status >= 400 && !url.ends_with("/favicon.ico") {
                    let remote = !url.starts_with("http://127.0.0.1");
                    let l = if remote { Level::Warning } else { Level::Error };
                    let msg = format!("{} answered {status}", self.short(url));
                    self.report.add(level(l), label, msg);
                }
            }
            "Network.loadingFailed" => {
                let id = p.get("requestId").and_then(Value::as_str).unwrap_or("");
                let url = self
                    .requests
                    .get(&(s.clone(), id.to_owned()))
                    .cloned()
                    .unwrap_or_default();
                let err = p.get("errorText").and_then(Value::as_str).unwrap_or("");
                let canceled = p.get("canceled").and_then(Value::as_bool) == Some(true);
                if canceled || url.ends_with("/favicon.ico") || url.is_empty() {
                    return;
                }
                let remote =
                    !url.starts_with("http://127.0.0.1") && !url.starts_with("ws://127.0.0.1");
                let l = if remote { Level::Warning } else { Level::Error };
                let msg = format!("loading {} failed: {err}", self.short(&url));
                self.report.add(level(l), label, msg);
            }
            _ => {}
        }
    }

    /// Waits for the tab's document to be complete (and a little more,
    /// for module scripts and their first requests).
    fn settle(&mut self, i: usize, step: Duration) -> bool {
        let s = self.tabs[i].session.clone();
        let ok = self.pump(step, &mut |me: &mut Smoke| {
            me.browser
                .cdp()
                .evaluate(
                    &s,
                    "document.readyState",
                    false,
                    false,
                    Duration::from_secs(2),
                )
                .ok()
                .and_then(|v| v.as_str().map(|r| r == "complete"))
                .unwrap_or(false)
        });
        self.pump(Duration::from_millis(1500), &mut |_| false);
        ok
    }
}

/// Runs the smoke test on a bundle (folder or zip).
pub fn smoke(path: &Path, opts: &SmokeOptions) -> Result<SmokeReport> {
    let bin = find_browser(opts.browser.as_deref())?;
    if Engine::of(&bin) != Engine::Chromium {
        return Err(Error::unsupported(format!(
            "--smoke needs a Chromium-based browser (Chrome, Edge, Chromium, Brave), not {}",
            bin.display()
        )));
    }
    let bundle = Bundle::open(path)?;
    let m = bundle.manifest.clone();
    let stop = Stop::new();
    let cast = Arc::new(Mutex::new(None));
    let running = Running::start(
        bundle,
        Box::new(Stand { cast: cast.clone() }),
        &Options {
            tunnel: TunnelChoice::Off,
            ..Options::default()
        },
        stop.clone(),
    )?;
    let hub = running.hub.clone();
    let ticker = {
        let (h, s) = (hub.clone(), stop.clone());
        std::thread::Builder::new()
            .name("smoke-tick".into())
            .spawn(move || {
                while !s.wait(Duration::from_millis(50)) {
                    h.tick();
                }
            })
            .map_err(|e| Error::io("spawn", e))?
    };
    let result = (|| -> Result<SmokeReport> {
        let profile = Profile::temporary()?;
        let browser = Browser::launch(
            &LaunchOptions {
                bin,
                viewport: (1280, 720),
                visible: false,
                screen: None,
                app: None,
            },
            &profile,
        )?;
        let ports = *hub.ports.get().ok_or_else(|| Error::closed("no ports"))?;
        let mut s = Smoke {
            browser,
            _profile: profile,
            tabs: Vec::new(),
            requests: Default::default(),
            local: format!("http://127.0.0.1:{}", ports.local),
            report: SmokeReport::default(),
        };
        run_steps(&mut s, &running, &hub, &cast, &m, ports.public, opts.step)?;
        s.browser.close();
        Ok(s.report)
    })();
    stop.request();
    let _ = ticker.join();
    running.end();
    result
}

fn run_steps(
    s: &mut Smoke,
    running: &Running,
    hub: &Arc<Hub>,
    cast: &Arc<Mutex<Option<String>>>,
    m: &super::bundle::Manifest,
    public: u16,
    step: Duration,
) -> Result<()> {
    // 1. The launcher (it sets the cookie), then the host page.
    let w = s.tab("launcher", &running.start_url(), false)?;
    if !s.settle(w, step) {
        s.report
            .add(Level::Error, "launcher", "did not finish loading");
    }
    let launcher = if m.pages.launcher.is_some() {
        "the game's launcher"
    } else {
        "tvpipe's launcher"
    };
    s.report.passed.push(format!("{launcher} loaded"));
    hub.call(
        u64::MAX,
        "select",
        &Value::object().with("devices", vec![RECEIVER]),
    )?;
    s.tabs[w].label = "host page";
    let host_url = format!("{}/{}", s.local, m.pages.host);
    s.navigate(w, &host_url)?;
    s.settle(w, step);
    let attached = s.pump(step, &mut |_| hub.host_attached());
    if !attached {
        s.report.add(
            Level::Error,
            "host page",
            format!(
                "never attached the relay within {} s: phones cannot join (Tvpipe.connect(), then tvpipe.party())",
                step.as_secs()
            ),
        );
        return Ok(());
    }
    s.report
        .passed
        .push("the host page attached the relay".into());
    // 2. The TV page, as cast.
    let cast_url = {
        let c = cast.clone();
        s.pump(step, &mut |_| {
            c.lock().unwrap_or_else(|e| e.into_inner()).is_some()
        });
        let v = cast.lock().unwrap_or_else(|e| e.into_inner()).clone();
        v
    };
    let Some(tv_url) = cast_url else {
        s.report.add(
            Level::Warning,
            "host page",
            format!(
                "did not cast the TV page within {} s (party.cast()): the TVs the launcher chose stay dark; opening the TV page without its link",
                step.as_secs()
            ),
        );
        let t = s.tab("tv page", &format!("{}/{}", s.local, m.pages.tv), true)?;
        s.settle(t, step);
        return Ok(());
    };
    let t = s.tab("tv page", &tv_url, false)?;
    if s.settle(t, step) {
        s.report
            .passed
            .push(format!("the TV page loaded ({})", s.short(&tv_url)));
    }
    // 3. The join link from a screen's view.
    let frag = tv_url.split_once('#').map(|(_, f)| f).unwrap_or("");
    let param = |text: &str, key: &str| {
        text.split('&')
            .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
            .map(str::to_owned)
    };
    let (Some(room), Some(screen_key)) = (param(frag, "r"), param(frag, "k")) else {
        s.report.add(
            Level::Warning,
            "host page",
            "cast a TV page without #r=…&k=… (not a party.links.tv link): the join check is skipped",
        );
        return Ok(());
    };
    let welcome = hello(
        ports_local(hub),
        Value::object()
            .with("t", "hello")
            .with("v", 1u64)
            .with("room", room.as_str())
            .with("role", "screen")
            .with("key", screen_key.as_str()),
        step,
    );
    let join = match welcome {
        Ok(w) => w
            .get("view")
            .and_then(|v| v.get("join"))
            .and_then(|j| j.get("url"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        Err(e) => {
            s.report.add(
                Level::Error,
                "host page",
                format!("a screen could not join: {e}"),
            );
            return Ok(());
        }
    };
    let Some(join) = join else {
        s.report.add(
            Level::Warning,
            "host page",
            "the screen's view has no join link (view.join.url): the phone check is skipped",
        );
        return Ok(());
    };
    // The link as phones get it, on this computer.
    let (base, rest) = join.split_once("/?").unwrap_or((join.as_str(), ""));
    let local_join = format!("http://127.0.0.1:{public}/?{rest}");
    let _ = base;
    let p = s.tab("phone page", &local_join, false)?;
    if s.settle(p, step) {
        s.report.passed.push("the phone page loaded".into());
    }
    // 4. A fake phone joins.
    let (q, f) = rest.split_once('#').unwrap_or((rest, ""));
    let (Some(room), Some(key)) = (param(q, "r"), param(f, "k")) else {
        s.report.add(
            Level::Warning,
            "host page",
            format!("the join link {join:?} has no ?r=…#k=…: the join check is skipped"),
        );
        return Ok(());
    };
    match hello(
        public,
        Value::object()
            .with("t", "hello")
            .with("v", 1u64)
            .with("room", room.as_str())
            .with("role", "player")
            .with("key", key.as_str())
            .with("name", "Smoke")
            .with("agent", "tvpipe smoke"),
        step,
    ) {
        Ok(w) => {
            let players = w
                .get("view")
                .and_then(|v| v.get("roster"))
                .and_then(Value::as_array)
                .map_or(0, <[Value]>::len);
            s.report.passed.push(format!(
                "a phone joined room {room} as Smoke ({players} in the roster)"
            ));
        }
        Err(e) => s.report.add(
            Level::Error,
            "phone page",
            format!("a fake phone could not join: {e}"),
        ),
    }
    // Late errors (the host reacting to the join).
    s.pump(Duration::from_millis(1000), &mut |_| false);
    Ok(())
}

fn ports_local(hub: &Arc<Hub>) -> u16 {
    hub.ports.get().map_or(0, |p| p.local)
}

/// Says `hello` on `/_tvpipe/ws` and returns the `welcome`; a `bye` or
/// a close is an error with its reason.
fn hello(port: u16, msg: Value, step: Duration) -> Result<Value> {
    let url = WsUrl::parse(&format!("ws://127.0.0.1:{port}/_tvpipe/ws"))?;
    let tcp = TcpStream::connect(("127.0.0.1", port))
        .map_err(|e| Error::io("connect to the relay", e))?;
    tcp.set_read_timeout(Some(step))
        .map_err(|e| Error::io("timeout", e))?;
    let (tcp, rest) = websocket::client_handshake(tcp, &url, &[("User-Agent", "tvpipe smoke")])?;
    let (mut rx, tx) = websocket::pair(tcp, rest, Role::Client, websocket::Limits::default())?;
    tx.send_text(&msg.to_json())?;
    let deadline = Instant::now() + step;
    while Instant::now() < deadline {
        match rx.read_message()? {
            Message::Text(t) => {
                let v = Value::parse(&t)?;
                match v.get("t").and_then(Value::as_str) {
                    Some("welcome") => {
                        let _ = tx.close(1000, "smoke done");
                        return Ok(v);
                    }
                    Some("bye") => {
                        return Err(Error::protocol(format!(
                            "bye {}{}",
                            v.get("reason").and_then(Value::as_str).unwrap_or("?"),
                            v.get("message")
                                .and_then(Value::as_str)
                                .map(|m| format!(": {m}"))
                                .unwrap_or_default()
                        )))
                    }
                    _ => {}
                }
            }
            Message::Binary(_) => {}
            Message::Close(c) => {
                return Err(Error::protocol(format!(
                    "closed {}",
                    c.map(|(code, r)| format!("{code} {r}")).unwrap_or_default()
                )))
            }
        }
    }
    Err(Error::timeout("no welcome"))
}
