//! `tvpipe screen`: a Chromium-based browser of tvpipe's own captures a
//! screen or a window, with the computer's sound, and encodes it into the
//! share pipeline (`docs/screen.md`).
//!
//! [`ScreenBrowser::start`] starts the browser (`launch`, a small normal
//! window: desktop capture does not work headless, and getDisplayMedia
//! wants a visible tab) with switches that answer Chrome's screen picker
//! ([`switches`]), opens the share server's `/screen` page and, once the
//! session knows what to capture, starts `capture.js` in it as a user
//! gesture (`headless.js` with [`connect_options`]). Once the page shares,
//! the window is minimized (it would be in the picture of the screen).
//! On macOS the browser is started as an app of its own
//! (`platform::own_app_command`), so that it holds the screen and
//! system-audio recording permissions itself.
//!
//! A keeper thread watches the page until [`ScreenBrowser::shutdown`]:
//! a share that ends (the browser's "Stop sharing", the window closed)
//! ends the session, a capture that fails (the picker was cancelled, no
//! permission) fails it with a hint; when no source matched after
//! [`PICK_HINT_AFTER`] the window comes to the front, where the picker
//! waits for the user. On shutdown the browser quits and its temporary
//! profile is deleted.
//!
//! Sources: Chromium `chrome/common/chrome_switches.h` (the
//! `--auto-select-*` switches, `--system-audio-capture-default_checked`),
//! `chrome/browser/ui/views/desktop_capture/desktop_media_list_controller.cc`
//! (a switch's value matches any part of a source's name; "Entire screen"
//! with one screen, "Screen N" with several:
//! `chrome/browser/media/webrtc/native_desktop_media_list.cc`),
//! `media/base/media_switches.cc` (system and application loopback per
//! OS), the Screen Capture spec (`systemAudio`, `windowAudio`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::browser::headless::{self, START_JS};
use crate::browser::launch::{Browser, Engine, LaunchOptions, Profile, ScreenLaunch};
use crate::browser::share::{ShareHandle, ShareServer};
use crate::cli::ScreenTarget;
use crate::codec::json::Value;
use crate::error::{Error, Result};
use crate::media::input::Stop;

/// The browser window while it starts (CSS pixels).
const WINDOW: (u32, u32) = (520, 340);
/// How long the page may take to load and the session to know what to
/// capture.
const LOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// With a source given by a switch: when the window comes to the front
/// (the picker found nothing to select, or asks for a permission).
pub const PICK_HINT_AFTER: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(300);
const EVAL_TIMEOUT: Duration = Duration::from_secs(5);
const EVENT_WAIT: Duration = Duration::from_millis(100);

/// The Chrome switches that answer the picker for `target`.
pub fn switches(target: &ScreenTarget) -> Vec<String> {
    match target {
        // The first screen: with one screen its name is "Entire screen",
        // with several "Screen 1"; this switch takes either.
        ScreenTarget::Display(1) => vec!["--auto-select-screen-capture-source".into()],
        // "Screen 2" also matches "Screen 20": nobody has 20 screens.
        ScreenTarget::Display(n) => {
            vec![format!("--auto-select-desktop-capture-source=Screen {n}")]
        }
        ScreenTarget::DisplayNamed(name) => {
            vec![format!("--auto-select-desktop-capture-source={name}")]
        }
        ScreenTarget::Window(title) => {
            vec![format!(
                "--auto-select-window-capture-source-by-title={title}"
            )]
        }
        ScreenTarget::Pick => Vec::new(),
    }
}

/// capture.js's connect options for `target`: the picker's kind, the
/// window's own sound (its app's, where the system can; else the
/// system's), sound kept playing on the computer.
pub fn connect_options(target: &ScreenTarget) -> Value {
    let surface = match target {
        ScreenTarget::Window(_) => "window",
        _ => "monitor",
    };
    Value::object()
        .with("surface", surface)
        .with("windowAudio", "window")
        .with("selfBrowserSurface", "exclude")
        .with("localAudio", true)
}

/// What the picker is to find, for messages.
fn wanted(target: &ScreenTarget) -> String {
    match target {
        ScreenTarget::Display(1) => "a screen".into(),
        ScreenTarget::Display(n) => format!("a screen named \"Screen {n}\""),
        ScreenTarget::DisplayNamed(n) => format!("a screen or window named \"{n}\""),
        ScreenTarget::Window(t) => format!("a window whose title contains \"{t}\""),
        ScreenTarget::Pick => "your choice".into(),
    }
}

/// What [`ScreenBrowser::start`] needs.
#[derive(Debug, Clone)]
pub struct ScreenOptions {
    pub bin: PathBuf,
    pub target: ScreenTarget,
}

struct Ctl {
    shutdown: AtomicBool,
}

/// The browser and its keeper thread.
pub struct ScreenBrowser {
    ctl: Arc<Ctl>,
    thread: Option<JoinHandle<()>>,
    profile_dir: PathBuf,
    pub product: String,
}

impl ScreenBrowser {
    /// Starts the browser on the server's screen page; the capture starts
    /// once the server is configured. Errors up to the page are returned
    /// here, later ones end the share through the server's handle.
    pub fn start(opts: ScreenOptions, server: &ShareServer, stop: Stop) -> Result<ScreenBrowser> {
        if Engine::of(&opts.bin) == Engine::Firefox {
            return Err(Error::unsupported(
                "tvpipe screen needs a Chromium-based browser, not Firefox",
            ));
        }
        let profile = Profile::temporary()?;
        let profile_dir = profile.dir().to_path_buf();
        let launch = LaunchOptions {
            bin: opts.bin.clone(),
            viewport: WINDOW,
            visible: true,
            screen: Some(ScreenLaunch {
                switches: switches(&opts.target),
                background: opts.target != ScreenTarget::Pick,
            }),
        };
        let browser = Browser::launch(&launch, &profile)?;
        let (target, session) = headless::attach(&browser, WINDOW)?;
        let url = server.screen_url();
        let r = browser.cdp().call(
            Some(&session),
            "Page.navigate",
            Value::object().with("url", url.as_str()),
        )?;
        if let Some(err) = r.get("errorText").and_then(Value::as_str) {
            return Err(Error::protocol(format!(
                "browser: cannot open its capture page: {err}"
            )));
        }
        let product = browser.product.clone();
        info!(
            "browser: {product} captures {}",
            match &opts.target {
                ScreenTarget::Pick => "what you pick in its window".to_owned(),
                t => wanted(t),
            }
        );
        let start_expr = format!(
            "({})({}, {})",
            START_JS.trim().trim_end_matches(';'),
            Value::from(server.ws_url()).to_json(),
            connect_options(&opts.target).to_json()
        );
        let ctl = Arc::new(Ctl {
            shutdown: AtomicBool::new(false),
        });
        let mut keeper = Keeper {
            browser: Some(browser),
            target,
            session,
            opts,
            share: server.handle(),
            start_expr,
            stop,
            ctl: ctl.clone(),
            _profile: profile,
        };
        let thread = std::thread::Builder::new()
            .name("screen-keeper".into())
            .spawn(move || keeper.run())
            .map_err(|e| Error::io("spawn screen keeper", e))?;
        Ok(ScreenBrowser {
            ctl,
            thread: Some(thread),
            profile_dir,
            product,
        })
    }

    /// The browser's (temporary) profile directory.
    pub fn profile_dir(&self) -> &Path {
        &self.profile_dir
    }

    /// Closes the browser and deletes its profile; returns when both are
    /// done.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.ctl.shutdown.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ScreenBrowser {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The page's capture (`globalThis.__tvpipe`): `state \n error \n
/// stopped \n surface label`.
const STATE_JS: &str = "(function () { var s = globalThis.__tvpipe; if (!s) \
     return document.readyState === 'complete' && globalThis.tvpipeCapture ? 'ready' : 'loading'; \
     var c = s.capture && s.capture.captureInfo; \
     return [s.state, s.error || '', s.stopped ? '1' : '', \
     c ? (c.surface || '') + ' ' + (c.label || '') : ''].join('\\n'); })()";

/// What the page's capture is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PageState {
    /// The page or capture.js is still loading.
    Loading,
    /// Loaded, no capture started.
    Ready,
    Starting,
    /// Sharing (`surface label`).
    Sharing(String),
    /// Ended: stopped in the browser (`true`), or why it failed.
    Ended(bool, String),
}

fn parse_state(v: &Value) -> PageState {
    let s = v.as_str().unwrap_or("loading");
    let mut parts = s.split('\n');
    let state = parts.next().unwrap_or("");
    let error = parts.next().unwrap_or("").to_owned();
    let stopped = parts.next() == Some("1");
    let what = parts.next().unwrap_or("").trim().to_owned();
    match state {
        "ready" => PageState::Ready,
        "connecting" | "capturing" => PageState::Starting,
        "sharing" => PageState::Sharing(what),
        "ended" => PageState::Ended(stopped, error),
        _ => PageState::Loading,
    }
}

/// The error for a capture that failed with `why` (the page's error).
fn capture_error(why: &str, target: &ScreenTarget, browser: &str) -> Error {
    let hint = if why.starts_with("NotAllowedError") {
        if cfg!(target_os = "macos") {
            format!(
                " (the picker was cancelled, or {browser} may not record the screen: allow it in \
                 System Settings > Privacy & Security > Screen & System Audio Recording, then \
                 run tvpipe again)"
            )
        } else {
            " (the picker was cancelled, or the system refused)".to_owned()
        }
    } else if why.starts_with("NotFoundError") || why.starts_with("NotReadableError") {
        format!(" (no {} to capture)", wanted(target))
    } else {
        String::new()
    };
    Error::protocol(format!("the browser cannot capture: {why}{hint}"))
}

struct Keeper {
    browser: Option<Browser>,
    target: String,
    session: String,
    opts: ScreenOptions,
    share: ShareHandle,
    start_expr: String,
    stop: Stop,
    ctl: Arc<Ctl>,
    /// Deleted (temporary) when the keeper ends, after the browser quit.
    _profile: Profile,
}

/// How the keeper ends.
enum Exit {
    /// tvpipe is stopping.
    Shutdown,
    /// The share is over (stopped in the browser, the window closed).
    Ended(String),
    Failed(Error),
}

impl Keeper {
    fn stopping(&self) -> bool {
        self.ctl.shutdown.load(Ordering::SeqCst) || self.stop.requested()
    }

    fn run(&mut self) {
        let exit = self.watch();
        if let Some(mut b) = self.browser.take() {
            b.close();
        }
        match exit {
            Exit::Shutdown => {}
            Exit::Ended(why) => {
                info!("browser: {why}");
                self.share.end(&why);
            }
            Exit::Failed(e) => {
                error!("browser: {e}");
                self.share.fail(e);
            }
        }
    }

    fn evaluate(&self, expr: &str, gesture: bool) -> Result<Value> {
        let b = self
            .browser
            .as_ref()
            .ok_or_else(|| Error::closed("no browser"))?;
        b.cdp()
            .evaluate(&self.session, expr, gesture, false, EVAL_TIMEOUT)
    }

    fn window_state(&self, state: &str) {
        let Some(b) = &self.browser else { return };
        let cdp = b.cdp();
        let r = cdp
            .call(
                None,
                "Browser.getWindowForTarget",
                Value::object().with("targetId", self.target.as_str()),
            )
            .and_then(|w| w.require_u64("windowId"))
            .and_then(|id| {
                cdp.call(
                    None,
                    "Browser.setWindowBounds",
                    Value::object()
                        .with("windowId", id)
                        .with("bounds", Value::object().with("windowState", state)),
                )
            });
        if let Err(e) = r {
            debug!("browser: window {state}: {e}");
        }
    }

    fn to_front(&self) {
        self.window_state("normal");
        if let Some(b) = &self.browser {
            let _ = b
                .cdp()
                .call(Some(&self.session), "Page.bringToFront", Value::object());
        }
    }

    fn watch(&mut self) -> Exit {
        let started = Instant::now();
        let mut injected: Option<Instant> = None;
        let mut hinted = self.opts.target == ScreenTarget::Pick;
        let mut sharing = false;
        let mut next_poll = Instant::now();
        loop {
            if self.stopping() {
                return Exit::Shutdown;
            }
            let Some(b) = &self.browser else {
                return Exit::Shutdown;
            };
            match b.cdp().next_event(EVENT_WAIT) {
                Ok(Some(ev)) => {
                    let p = &ev.params;
                    let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
                    match ev.method.as_str() {
                        "Inspector.targetCrashed" | "Target.targetCrashed"
                            if s("targetId") == self.target
                                || ev.session.as_deref() == Some(self.session.as_str()) =>
                        {
                            return Exit::Failed(Error::process("the capture page crashed"));
                        }
                        "Target.targetDestroyed" if s("targetId") == self.target => {
                            return self.gone(sharing);
                        }
                        "Target.detachedFromTarget" if s("sessionId") == self.session => {
                            return self.gone(sharing);
                        }
                        _ => {}
                    }
                    continue;
                }
                Ok(None) => {}
                Err(_) => {
                    if self.stopping() {
                        return Exit::Shutdown;
                    }
                    return self.gone(sharing);
                }
            }
            let now = Instant::now();
            if now < next_poll {
                continue;
            }
            next_poll = now + POLL;
            let state = match self.evaluate(STATE_JS, false) {
                Ok(v) => parse_state(&v),
                Err(e) => {
                    trace!("browser: state: {e}");
                    PageState::Loading
                }
            };
            match state {
                PageState::Loading | PageState::Ready if injected.is_none() => {
                    let ready = state == PageState::Ready && self.share.configured();
                    if ready {
                        match self.evaluate(&self.start_expr, true) {
                            Ok(_) => {
                                debug!("browser: capture started in the page");
                                injected = Some(now);
                            }
                            Err(e) => debug!("browser: cannot start the capture yet: {e}"),
                        }
                    } else if now.duration_since(started) > LOAD_TIMEOUT {
                        return Exit::Failed(Error::timeout(
                            "the browser's capture page did not load",
                        ));
                    }
                }
                PageState::Loading | PageState::Ready => {
                    // The page was loaded again (it should not be).
                    return Exit::Failed(Error::protocol("the capture page was reloaded"));
                }
                PageState::Starting => {
                    let waited = injected.map_or(Duration::ZERO, |t| now.duration_since(t));
                    if !hinted && waited > PICK_HINT_AFTER {
                        hinted = true;
                        warn!(
                            "browser: nothing matches {} yet; pick it in the browser's window{}",
                            wanted(&self.opts.target),
                            if cfg!(target_os = "macos") {
                                " (if it asks for permission: System Settings > Privacy & \
                                 Security > Screen & System Audio Recording)"
                            } else {
                                ""
                            }
                        );
                        self.to_front();
                    }
                }
                PageState::Sharing(what) => {
                    if !sharing {
                        sharing = true;
                        info!("browser: sharing {what}");
                        // Out of the picture (and out of the way).
                        self.window_state("minimized");
                    }
                }
                PageState::Ended(true, _) => {
                    return Exit::Ended("sharing was stopped in the browser".into());
                }
                PageState::Ended(false, why) => {
                    let browser = self.browser.as_ref().map_or("", |b| b.product.as_str());
                    let name = browser.split('/').next().unwrap_or("the browser");
                    return Exit::Failed(capture_error(&why, &self.opts.target, name));
                }
            }
        }
    }

    /// The browser or its tab went away.
    fn gone(&self, sharing: bool) -> Exit {
        if sharing {
            Exit::Ended("the browser was closed".into())
        } else {
            Exit::Failed(Error::process(
                "the browser was closed before it shared anything",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_switches() {
        assert_eq!(
            switches(&ScreenTarget::Display(1)),
            ["--auto-select-screen-capture-source"]
        );
        assert_eq!(
            switches(&ScreenTarget::Display(2)),
            ["--auto-select-desktop-capture-source=Screen 2"]
        );
        assert_eq!(
            switches(&ScreenTarget::DisplayNamed("Bildschirm 2".into())),
            ["--auto-select-desktop-capture-source=Bildschirm 2"]
        );
        assert_eq!(
            switches(&ScreenTarget::Window("Keynote".into())),
            ["--auto-select-window-capture-source-by-title=Keynote"]
        );
        assert!(switches(&ScreenTarget::Pick).is_empty());
    }

    #[test]
    fn capture_options() {
        let o = connect_options(&ScreenTarget::Window("x".into()));
        assert_eq!(o.get("surface").and_then(Value::as_str), Some("window"));
        assert_eq!(o.get("windowAudio").and_then(Value::as_str), Some("window"));
        assert_eq!(o.get("localAudio").and_then(Value::as_bool), Some(true));
        let o = connect_options(&ScreenTarget::Display(1));
        assert_eq!(o.get("surface").and_then(Value::as_str), Some("monitor"));
        assert_eq!(
            o.get("selfBrowserSurface").and_then(Value::as_str),
            Some("exclude")
        );
    }

    #[test]
    fn page_states() {
        let st = |s: &str| parse_state(&Value::from(s));
        assert_eq!(st("loading"), PageState::Loading);
        assert_eq!(parse_state(&Value::Null), PageState::Loading);
        assert_eq!(st("ready"), PageState::Ready);
        assert_eq!(st("connecting\n\n\n"), PageState::Starting);
        assert_eq!(st("capturing\n\n\n"), PageState::Starting);
        assert_eq!(
            st("sharing\n\n\nmonitor screen:1:0"),
            PageState::Sharing("monitor screen:1:0".into())
        );
        assert_eq!(
            st("ended\nthe share was stopped in the browser\n1\nmonitor x"),
            PageState::Ended(true, "the share was stopped in the browser".into())
        );
        assert_eq!(
            st("ended\nNotAllowedError: Permission denied\n\n"),
            PageState::Ended(false, "NotAllowedError: Permission denied".into())
        );
    }

    #[test]
    fn capture_errors_have_hints() {
        let e = capture_error(
            "NotAllowedError: Permission denied",
            &ScreenTarget::Display(1),
            "Chrome",
        );
        assert!(e.to_string().contains("cancelled"), "{e}");
        let e = capture_error(
            "NotFoundError: x",
            &ScreenTarget::Window("Keynote".into()),
            "C",
        );
        assert!(e.to_string().contains("\"Keynote\""), "{e}");
    }

    #[test]
    fn start_expression_takes_options() {
        let expr = format!(
            "({})({}, {})",
            START_JS.trim().trim_end_matches(';'),
            Value::from("ws://127.0.0.1:1/ws?t=x").to_json(),
            connect_options(&ScreenTarget::Pick).to_json()
        );
        assert!(
            expr.ends_with(
                "}))(\"ws://127.0.0.1:1/ws?t=x\", {\"surface\":\"monitor\",\
                 \"windowAudio\":\"window\",\"selfBrowserSurface\":\"exclude\",\
                 \"localAudio\":true})"
            ),
            "{expr}"
        );
    }
}
