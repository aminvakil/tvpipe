//! Finding and starting the browser for `tvpipe share URL`: a
//! Chromium-based one (driven over the DevTools protocol, [`Cdp`]) or
//! Firefox (WebDriver BiDi, `bidi`); [`Engine`] tells them apart.
//!
//! - [`find_browser`]: `--browser PATH` (or a name on `PATH`, or a macOS
//!   `.app`; `firefox` also finds an installed Firefox), `$TVPIPE_BROWSER`,
//!   the well-known install paths of Chromium-based browsers
//!   (`browser::platform`), then `PATH`, then the same for Firefox.
//! - [`Profile`]: the browser's own user-data directory: a fresh temp
//!   directory (deleted on drop) or `--profile DIR` (kept, for logins).
//!   Never the user's real browser profile.
//! - [`sweep_stale_profiles`]: temp profiles a killed tvpipe left behind
//!   (and the browsers still running on them). A temporary profile holds
//!   `tvpipe-owner`: the port of a loopback "beacon" its tvpipe answers on
//!   with a random nonce while it runs; no answer means its tvpipe is
//!   gone. A browser still on such a profile is asked to quit through the
//!   DevTools endpoint its own `DevToolsActivePort` names (the path holds
//!   that browser instance's id, so no other browser can answer it), or
//!   Firefox through the BiDi port its `WebDriverBiDiServer.json` names.
//! - [`Browser::launch`]: starts it with `--remote-debugging-port=0`,
//!   reads the port from `DevToolsActivePort` (Firefox:
//!   `WebDriverBiDiServer.json`) in the profile, connects [`Cdp`] to the
//!   browser endpoint (Firefox: a BiDi session). [`Browser::close`] (and
//!   drop) asks it to quit (`Browser.close`, `browser.close`), waits,
//!   then kills it.
//! - Firefox's preferences ([`FIREFOX_PREFS`], written to the profile's
//!   `user.js`): no permission prompts, the old getUserMedia tab and
//!   tab-audio capture sources on, autoplay allowed, its own sound muted.
//!
//! Sources: Chrome DevTools Protocol docs; Chromium command-line switches
//! (`--remote-debugging-port`, `--user-data-dir`, `--headless`,
//! `--auto-accept-this-tab-capture`, `--autoplay-policy`, ...); Firefox's
//! command line (`--profile`, `--headless`, `--remote-debugging-port`) and
//! preferences (modules/libpref/init/StaticPrefList.yaml).

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::browser::cdp::Cdp;
use crate::browser::{bidi, platform};
use crate::codec::json::Value;
use crate::error::{Error, Result};

/// The environment variable naming the browser (like `--browser`).
pub const BROWSER_ENV: &str = "TVPIPE_BROWSER";
/// How long the browser may take to open its DevTools port.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a browser asked to quit may take before it is killed.
const QUIT_TIMEOUT: Duration = Duration::from_secs(5);
/// Lines of the browser's stderr kept for error messages.
const STDERR_TAIL: usize = 20;

/// The kind of browser, which decides how tvpipe drives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// Chrome, Edge, Chromium, Brave: the DevTools protocol.
    Chromium,
    /// Firefox: WebDriver BiDi.
    Firefox,
}

impl Engine {
    /// By the executable's name: Firefox's has `firefox` in it
    /// (`firefox`, `firefox-esr`, `firefox.exe`); anything else is taken
    /// for a Chromium-based browser.
    pub fn of(bin: &Path) -> Engine {
        let name = bin
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if name.contains("firefox") {
            Engine::Firefox
        } else {
            Engine::Chromium
        }
    }
}

/// Finds a Chromium-based browser or Firefox (see the module docs for the
/// order).
pub fn find_browser(explicit: Option<&Path>) -> Result<PathBuf> {
    find_with(explicit, &|k| std::env::var_os(k), &|p| p.is_file())
}

fn find_with(
    explicit: Option<&Path>,
    env: &dyn Fn(&str) -> Option<OsString>,
    exists: &dyn Fn(&Path) -> bool,
) -> Result<PathBuf> {
    let path_dirs: Vec<PathBuf> = env("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let on_path = |name: &Path| -> Option<PathBuf> {
        path_dirs
            .iter()
            .map(|d| d.join(name))
            .find(|p| exists(p))
            .or_else(|| {
                // `msedge` for `msedge.exe` on Windows.
                let suffix = std::env::consts::EXE_SUFFIX;
                let mut with = name.as_os_str().to_owned();
                with.push(suffix);
                (!suffix.is_empty() && name.extension().is_none())
                    .then(|| path_dirs.iter().map(|d| d.join(&with)).find(|p| exists(p)))
                    .flatten()
            })
    };
    let firefox = || {
        platform::firefox_install_paths(env)
            .into_iter()
            .find(|p| exists(p))
            .or_else(|| {
                platform::firefox_names()
                    .iter()
                    .find_map(|n| on_path(Path::new(n)))
            })
    };
    let given = |p: &Path, what: &str| -> Result<PathBuf> {
        if exists(p) {
            return Ok(p.to_path_buf());
        }
        if let Some(exe) = app_bundle_executable(p).into_iter().find(|e| exists(e)) {
            return Ok(exe);
        }
        if p.components().count() == 1 {
            if let Some(found) = on_path(p) {
                return Ok(found);
            }
            if p.as_os_str().eq_ignore_ascii_case("firefox") {
                if let Some(found) = firefox() {
                    return Ok(found);
                }
            }
        }
        Err(Error::not_found(format!(
            "{what} {}: no such browser executable",
            p.display()
        )))
    };
    if let Some(p) = explicit {
        return given(p, "--browser");
    }
    if let Some(p) = env(BROWSER_ENV).filter(|v| !v.is_empty()) {
        return given(Path::new(&p), &format!("${BROWSER_ENV}"));
    }
    if let Some(p) = platform::browser_install_paths(env)
        .into_iter()
        .find(|p| exists(p))
    {
        return Ok(p);
    }
    if let Some(p) = platform::browser_names()
        .iter()
        .find_map(|n| on_path(Path::new(n)))
    {
        return Ok(p);
    }
    if let Some(p) = firefox() {
        return Ok(p);
    }
    Err(Error::not_found(format!(
        "no browser found (Chrome, Edge, Chromium, Brave or Firefox); \
         install one or name it with --browser PATH or ${BROWSER_ENV}"
    )))
}

/// `X.app` → `X.app/Contents/MacOS/X`, or `.../firefox` (Firefox's
/// bundles, `Firefox Nightly.app` too, name it so): a macOS bundle given
/// as the browser.
fn app_bundle_executable(p: &Path) -> Vec<PathBuf> {
    let Some(name) = p
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".app"))
    else {
        return Vec::new();
    };
    let dir = p.join("Contents/MacOS");
    vec![dir.join(name), dir.join("firefox")]
}

/// Name prefix of temporary profiles (the only directories the sweep
/// touches).
const TEMP_PREFIX: &str = "tvpipe-browser-";
/// In a temporary profile: `PID PORT NONCE` of its tvpipe's beacon.
const OWNER_FILE: &str = "tvpipe-owner";
/// A temporary profile without an owner file is left alone this long
/// (its tvpipe may be creating it).
const ORPHAN_AGE: Duration = Duration::from_secs(120);

/// The browser's user-data directory.
#[derive(Debug)]
pub struct Profile {
    dir: PathBuf,
    temporary: bool,
    beacon: Option<Beacon>,
}

impl Profile {
    /// A fresh directory under the system's temp directory, deleted when
    /// dropped.
    pub fn temporary() -> Result<Profile> {
        Profile::temporary_in(&std::env::temp_dir())
    }

    fn temporary_in(root: &Path) -> Result<Profile> {
        let nonce = crate::util::hex::encode(&crate::crypto::ring::random_array::<16>()?);
        let dir = root.join(format!("{TEMP_PREFIX}{}", &nonce[..12]));
        std::fs::create_dir(&dir).map_err(|e| Error::io(format!("create {}", dir.display()), e))?;
        let mut p = Profile {
            dir,
            temporary: true,
            beacon: None,
        };
        let beacon = Beacon::start(nonce.clone())?;
        let owner = format!("{} {} {nonce}\n", std::process::id(), beacon.port);
        p.beacon = Some(beacon);
        std::fs::write(p.dir.join(OWNER_FILE), owner)
            .map_err(|e| Error::io(format!("write {}", p.dir.display()), e))?;
        Ok(p)
    }

    /// `--profile DIR`: kept across runs (logins, cookies). Refuses the
    /// browsers' own default profiles.
    pub fn persistent(dir: &Path) -> Result<Profile> {
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::io(format!("create {}", dir.display()), e))?;
        let dir = std::fs::canonicalize(dir)
            .map_err(|e| Error::io(format!("resolve {}", dir.display()), e))?;
        let real = platform::default_profile_dirs(&|k| std::env::var_os(k));
        check_not_real(&dir, &real)?;
        Ok(Profile {
            dir,
            temporary: false,
            beacon: None,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// Refuses `dir` if it is (or is inside, or holds) a browser's own
/// profile directory.
fn check_not_real(dir: &Path, real: &[PathBuf]) -> Result<()> {
    for r in real {
        let r = std::fs::canonicalize(r).unwrap_or_else(|_| r.clone());
        if dir.starts_with(&r) || r.starts_with(dir) {
            return Err(Error::usage(format!(
                "--profile {}: that is (or holds) a browser's own profile ({}); \
                 tvpipe uses a separate one: pick an empty directory",
                dir.display(),
                r.display()
            )));
        }
    }
    Ok(())
}

impl Drop for Profile {
    fn drop(&mut self) {
        self.beacon = None;
        if !self.temporary {
            return;
        }
        // Helper processes may still write for a moment after the
        // browser quit.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match std::fs::remove_dir_all(&self.dir) {
                Ok(()) => return,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
                Err(e) if Instant::now() >= deadline => {
                    warn!("cannot delete {}: {e}", self.dir.display());
                    return;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }
}

/// Answers connections on a loopback port with `nonce` while its
/// tvpipe runs (see the module docs).
#[derive(Debug)]
struct Beacon {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Beacon {
    fn start(nonce: String) -> Result<Beacon> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|e| Error::io("bind the profile beacon", e))?;
        let port = listener
            .local_addr()
            .map_err(|e| Error::io("profile beacon address", e))?
            .port();
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let thread = std::thread::Builder::new()
            .name("profile-beacon".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    if st.load(Ordering::SeqCst) {
                        return;
                    }
                    if let Ok(mut c) = conn {
                        let _ = c.set_write_timeout(Some(Duration::from_secs(1)));
                        let _ = c.write_all(nonce.as_bytes());
                    }
                }
            })
            .map_err(|e| Error::io("spawn the profile beacon", e))?;
        Ok(Beacon {
            port,
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Beacon {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept (if that fails, the thread is left
        // behind rather than hanging the exit).
        let _ = TcpStream::connect_timeout(
            &(Ipv4Addr::LOCALHOST, self.port).into(),
            Duration::from_secs(1),
        );
        if let Some(t) = self.thread.take() {
            crate::util::thread::join_until(t, Instant::now() + Duration::from_secs(2));
        }
    }
}

/// Removes the temporary profiles in the system's temp directory whose
/// tvpipe is gone (killed before it could clean up), after asking a
/// browser still running on one to quit. Returns how many it removed.
pub fn sweep_stale_profiles() -> usize {
    sweep_in(&std::env::temp_dir(), ORPHAN_AGE)
}

fn sweep_in(root: &Path, orphan_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for e in entries.flatten() {
        if !e.file_name().to_string_lossy().starts_with(TEMP_PREFIX)
            || !e.file_type().is_ok_and(|t| t.is_dir())
        {
            continue;
        }
        let dir = e.path();
        let closed = match owner(&dir, orphan_age) {
            Owner::Alive => continue,
            // Made by a tvpipe without beacons (or being made): only
            // removed when no browser runs on it.
            Owner::Unknown if browser_answers(&dir) => continue,
            Owner::Unknown => Ok(false),
            Owner::Gone => close_stale_browser(&dir),
        };
        let closed = match closed {
            Ok(closed) => closed,
            Err(e) => {
                warn!(
                    "browser: {} was left by an earlier run, but its browser {e}; leaving it",
                    dir.display()
                );
                continue;
            }
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        let gone = loop {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => break true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break true,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100))
                }
                Err(e) => {
                    debug!("browser: cannot delete {}: {e}", dir.display());
                    break false;
                }
            }
        };
        if gone {
            removed += 1;
            info!(
                "browser: removed {} left by an earlier run{}",
                dir.display(),
                if closed {
                    " (and quit its browser)"
                } else {
                    ""
                }
            );
        }
    }
    removed
}

/// Whether the tvpipe that made a temporary profile still runs.
#[derive(Debug, PartialEq, Eq)]
enum Owner {
    /// Its beacon answers (or the directory is being made).
    Alive,
    /// Its beacon does not answer, or the owner file is garbage.
    Gone,
    /// No owner file and older than the orphan age.
    Unknown,
}

fn owner(dir: &Path, orphan_age: Duration) -> Owner {
    let Ok(owner) = std::fs::read_to_string(dir.join(OWNER_FILE)) else {
        let age = std::fs::metadata(dir)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok());
        return if age.is_none_or(|a| a < orphan_age) {
            Owner::Alive
        } else {
            Owner::Unknown
        };
    };
    let mut f = owner.split_whitespace();
    let (_pid, port, nonce) = (f.next(), f.next(), f.next());
    match (port.and_then(|p| p.parse::<u16>().ok()), nonce) {
        (Some(port), Some(nonce)) if beacon_answers(port, nonce) => Owner::Alive,
        _ => Owner::Gone,
    }
}

/// The endpoint of a browser that may still run on profile `dir`: its
/// engine, port and WebSocket URL (Chromium's `DevToolsActivePort`,
/// Firefox's `WebDriverBiDiServer.json`).
fn stale_endpoint(dir: &Path) -> Option<(Engine, u16, String)> {
    if let Some((port, path)) = std::fs::read_to_string(dir.join("DevToolsActivePort"))
        .ok()
        .and_then(|s| parse_active_port(&s))
    {
        return Some((
            Engine::Chromium,
            port,
            format!("ws://127.0.0.1:{port}{path}"),
        ));
    }
    let port = bidi::server_file(dir)?;
    Some((Engine::Firefox, port, bidi::session_url(port)))
}

/// Whether a browser answers on the endpoint `dir` names.
fn browser_answers(dir: &Path) -> bool {
    stale_endpoint(dir)
        .is_some_and(|(_, _, url)| Cdp::connect(&url, Duration::from_secs(2)).is_ok())
}

fn beacon_answers(port: u16, nonce: &str) -> bool {
    let addr = (Ipv4Addr::LOCALHOST, port).into();
    let Ok(mut c) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return false;
    };
    let _ = c.set_read_timeout(Some(Duration::from_secs(1)));
    let mut got = Vec::new();
    let _ = Read::by_ref(&mut c).take(64).read_to_end(&mut got);
    got == nonce.as_bytes()
}

/// Asks a browser still running on the stale profile `dir` to quit, by
/// the DevTools endpoint in its `DevToolsActivePort`. `Ok(true)`: one
/// quit; `Ok(false)`: none answers; an error: one answers but does not
/// quit.
fn close_stale_browser(dir: &Path) -> Result<bool> {
    let Some((engine, port, url)) = stale_endpoint(dir) else {
        return Ok(false);
    };
    let Ok(cdp) = Cdp::connect(&url, Duration::from_secs(2)) else {
        return Ok(false);
    };
    // Firefox still holds the dead tvpipe's BiDi session: Marionette.
    let marionette = (engine == Engine::Firefox)
        .then(|| bidi::marionette_port(dir))
        .flatten();
    match marionette {
        Some(m) => {
            if let Err(e) = bidi::marionette_quit(m, Duration::from_secs(2)) {
                debug!("browser: marionette: {e}");
            }
        }
        None => ask_to_quit(&cdp, engine),
    }
    drop(cdp);
    // Gone once its DevTools port refuses connections.
    let deadline = Instant::now() + QUIT_TIMEOUT;
    let addr = (Ipv4Addr::LOCALHOST, port).into();
    while TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
        if Instant::now() >= deadline {
            return Err(Error::timeout("does not quit"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Its helper processes follow it.
    std::thread::sleep(Duration::from_millis(300));
    Ok(true)
}

/// Asks the browser on `rpc` to quit (without waiting for it). Firefox
/// takes the command in a session only (ours, or a new one if it has
/// none).
fn ask_to_quit(rpc: &Cdp, engine: Engine) {
    let wait = Duration::from_secs(2);
    match engine {
        Engine::Chromium => {
            let _ = rpc.call_timeout(None, "Browser.close", Value::object(), wait);
        }
        Engine::Firefox => {
            // In a session of ours, or a new one.
            if rpc
                .call_timeout(None, "browser.close", Value::object(), wait)
                .is_err()
                && bidi::new_session(rpc).is_ok()
            {
                let _ = rpc.call_timeout(None, "browser.close", Value::object(), wait);
            }
        }
    }
}

/// How to start the browser.
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub bin: PathBuf,
    /// The page's viewport (CSS pixels at scale 1): the stream size.
    pub viewport: (u32, u32),
    /// A normal window instead of `--headless=new`.
    pub visible: bool,
}

impl LaunchOptions {
    pub fn engine(&self) -> Engine {
        Engine::of(&self.bin)
    }
}

/// Firefox's preferences for tvpipe's profile (`user.js`). Firefox's
/// remote agent adds its own automation defaults (no first-run pages, no
/// update checks, ...).
pub const FIREFOX_PREFS: &[(&str, &str)] = &[
    // The Marionette port in MarionetteActivePort (for the sweep,
    // `bidi::marionette_quit`).
    ("marionette.port", "0"),
    // getUserMedia without a prompt, and its sources for this tab's
    // picture ('browser') and sound ('audioCapture'): Firefox's
    // getDisplayMedia has no tab capture and no audio (capture.js).
    ("media.navigator.permission.disabled", "true"),
    ("media.getusermedia.browser.enabled", "true"),
    ("media.getusermedia.audio.capture.enabled", "true"),
    // Sound and video play without a click.
    ("media.autoplay.default", "0"),
    ("media.autoplay.blocking_policy", "0"),
    // The captured sound is taken before the output: mute the output
    // (the TV plays it).
    ("media.volume_scale", "\"0.0\""),
    // Pages from the internet may reach tvpipe on 127.0.0.1 (Local
    // Network Access).
    ("network.lna.enabled", "false"),
    ("network.lna.blocking", "false"),
    // Navigations and reloads never wait for a "leave this page?" prompt.
    ("dom.disable_beforeunload", "true"),
    // No hidden preloaded new-tab page beside the page's tab.
    ("browser.newtab.preload", "false"),
    // A restarted browser opens the page, not a restore or safe-mode
    // question.
    ("browser.sessionstore.resume_from_crash", "false"),
    ("toolkit.startup.max_resumed_crashes", "-1"),
    // The page keeps its timers and painting in a window behind others.
    ("dom.timeout.enable_budget_timer_throttling", "false"),
    ("widget.windows.window_occlusion_tracking.enabled", "false"),
];

/// `user.js` with [`FIREFOX_PREFS`].
pub fn firefox_user_js() -> String {
    let mut s = String::from("// Written by tvpipe for 'tvpipe share URL'.\n");
    for (k, v) in FIREFOX_PREFS {
        s.push_str(&format!("user_pref(\"{k}\", {v});\n"));
    }
    s
}

/// The command line for `opts` with profile `dir`, opening `about:blank`
/// (the caller sets the page up before it loads the real URL).
pub fn args(opts: &LaunchOptions, dir: &Path) -> Vec<OsString> {
    if opts.engine() == Engine::Firefox {
        return firefox_args(opts, dir);
    }
    let (w, h) = opts.viewport;
    let mut a: Vec<OsString> = Vec::new();
    if !opts.visible {
        a.push("--headless=new".into());
        // Headless tab capture is clamped to the screen (800x600 unless
        // told otherwise).
        a.push(format!("--screen-info={{0,0 {w}x{h}}}").into());
    }
    let mut dir_arg = OsString::from("--user-data-dir=");
    dir_arg.push(dir);
    a.push(dir_arg);
    for s in [
        "--remote-debugging-port=0",
        // A profile of its own: no first-run UI, no default-browser
        // question, no keychain prompt (macOS) or keyring (Linux).
        "--no-first-run",
        "--no-default-browser-check",
        "--use-mock-keychain",
        "--password-store=basic",
        // getDisplayMedia({preferCurrentTab}) shares this tab without a
        // picker; sound plays without a click.
        "--auto-accept-this-tab-capture",
        "--autoplay-policy=no-user-gesture-required",
        // The captured tab keeps its timers and frame rate when hidden
        // or covered (a visible window behind others).
        "--disable-background-timer-throttling",
        "--disable-renderer-backgrounding",
        "--disable-backgrounding-occluded-windows",
        "--force-device-scale-factor=1",
    ] {
        a.push(s.into());
    }
    // The window is resized to the exact viewport once it is up.
    a.push(format!("--window-size={w},{h}").into());
    a.push("about:blank".into());
    a
}

/// Firefox's command line (the viewport is set over BiDi).
fn firefox_args(opts: &LaunchOptions, dir: &Path) -> Vec<OsString> {
    let (w, h) = opts.viewport;
    let mut a: Vec<OsString> = Vec::new();
    if !opts.visible {
        a.push("--headless".into());
    }
    a.push("--profile".into());
    a.push(dir.into());
    for s in [
        "--no-remote",
        "--new-instance",
        "--remote-debugging-port=0",
        "--marionette",
    ] {
        a.push(s.into());
    }
    a.push(format!("--window-size={w},{h}").into());
    a.push("about:blank".into());
    a
}

/// A running browser and its DevTools (Firefox: WebDriver BiDi)
/// connection.
pub struct Browser {
    child: Option<Child>,
    cdp: Cdp,
    pub engine: Engine,
    /// The product (`Chrome/157.0...`, `HeadlessChrome/...`,
    /// `Firefox/160.0`).
    pub product: String,
    pub user_agent: String,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

impl Browser {
    /// Starts the browser with `profile` and connects to it.
    pub fn launch(opts: &LaunchOptions, profile: &Profile) -> Result<Browser> {
        let engine = opts.engine();
        let port_file = profile.dir().join(match engine {
            Engine::Chromium => "DevToolsActivePort",
            Engine::Firefox => bidi::SERVER_FILE,
        });
        // A persistent profile may hold the last run's.
        let _ = std::fs::remove_file(&port_file);
        if engine == Engine::Firefox {
            let _ = std::fs::remove_file(profile.dir().join(bidi::MARIONETTE_FILE));
            let user_js = profile.dir().join("user.js");
            std::fs::write(&user_js, firefox_user_js())
                .map_err(|e| Error::io(format!("write {}", user_js.display()), e))?;
        }
        debug!(
            "browser: starting {} with profile {}",
            opts.bin.display(),
            profile.dir().display()
        );
        let mut child = Command::new(&opts.bin)
            .args(args(opts, profile.dir()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::io(format!("start {}", opts.bin.display()), e))?;
        let stderr: Arc<Mutex<VecDeque<String>>> = Arc::default();
        if let Some(err) = child.stderr.take() {
            let tail = stderr.clone();
            let _ = std::thread::Builder::new()
                .name("browser-stderr".into())
                .spawn(move || {
                    for line in BufReader::new(err).lines() {
                        let Ok(line) = line else { return };
                        trace!("browser stderr: {line}");
                        let mut t = tail.lock().unwrap_or_else(|e| e.into_inner());
                        if t.len() >= STDERR_TAIL {
                            t.pop_front();
                        }
                        t.push_back(line);
                    }
                });
        }
        let tail = |stderr: &Arc<Mutex<VecDeque<String>>>| {
            let t = stderr.lock().unwrap_or_else(|e| e.into_inner());
            t.iter()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| format!(": {}", l.trim()))
                .unwrap_or_default()
        };
        let deadline = Instant::now() + START_TIMEOUT;
        let (port, url) = loop {
            let found = match engine {
                Engine::Chromium => std::fs::read_to_string(&port_file)
                    .ok()
                    .and_then(|s| parse_active_port(&s))
                    .map(|(port, path)| (port, format!("ws://127.0.0.1:{port}{path}"))),
                Engine::Firefox => {
                    bidi::server_file(profile.dir()).map(|port| (port, bidi::session_url(port)))
                }
            };
            if let Some(p) = found {
                break p;
            }
            if let Ok(Some(status)) = child.try_wait() {
                // Give the stderr reader a moment to catch the last line.
                std::thread::sleep(Duration::from_millis(100));
                return Err(Error::process(format!(
                    "the browser exited at once ({status}){}; is another browser \
                     using the same --profile?",
                    tail(&stderr)
                )));
            }
            if Instant::now() >= deadline {
                kill(&mut child);
                return Err(Error::timeout(format!(
                    "the browser did not open its remote debugging port within {} s{}",
                    START_TIMEOUT.as_secs(),
                    tail(&stderr)
                )));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let cdp = match Cdp::connect(&url, Duration::from_secs(10)) {
            Ok(c) => c,
            Err(e) => {
                kill(&mut child);
                return Err(e);
            }
        };
        let mut b = Browser {
            child: Some(child),
            cdp,
            engine,
            product: String::new(),
            user_agent: String::new(),
            stderr,
        };
        match engine {
            Engine::Chromium => {
                let v = b.cdp.call(None, "Browser.getVersion", Value::object())?;
                let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
                b.product = s("product");
                b.user_agent = s("userAgent");
                debug!("browser: {} on DevTools port {port}", b.product);
            }
            Engine::Firefox => {
                let info = bidi::new_session(&b.cdp)?;
                b.product = info.product;
                b.user_agent = info.user_agent;
                debug!("browser: {} on WebDriver BiDi port {port}", b.product);
            }
        }
        Ok(b)
    }

    pub fn cdp(&self) -> &Cdp {
        &self.cdp
    }

    /// The browser's exit status, if it exited.
    pub fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.child
            .as_mut()
            .and_then(|c| c.try_wait().ok().flatten())
    }

    /// The last line the browser wrote to stderr.
    pub fn last_error_line(&self) -> Option<String> {
        let t = self.stderr.lock().unwrap_or_else(|e| e.into_inner());
        t.iter().rev().find(|l| !l.trim().is_empty()).cloned()
    }

    /// Asks the browser to quit, waits for it, kills it if it does not.
    pub fn close(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if matches!(child.try_wait(), Ok(None)) {
            ask_to_quit(&self.cdp, self.engine);
        }
        let deadline = Instant::now() + QUIT_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    debug!("browser: exited ({status})");
                    return;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                _ => {
                    warn!("browser: did not quit; killing it");
                    kill(&mut child);
                    return;
                }
            }
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.close();
    }
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// `DevToolsActivePort`: the port on the first line, the browser
/// endpoint's path on the second.
fn parse_active_port(s: &str) -> Option<(u16, String)> {
    let mut lines = s.lines();
    let port: u16 = lines.next()?.trim().parse().ok().filter(|&p| p > 0)?;
    let path = lines.next()?.trim();
    path.starts_with("/devtools/browser/")
        .then(|| (port, path.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: HashMap<String, OsString> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |k| m.get(k).cloned()
    }

    fn files(list: &[&str]) -> impl Fn(&Path) -> bool {
        let set: Vec<PathBuf> = list.iter().map(PathBuf::from).collect();
        move |p| set.iter().any(|f| f == p)
    }

    fn path_var(dirs: &[&str]) -> String {
        std::env::join_paths(dirs).unwrap().into_string().unwrap()
    }

    #[test]
    fn explicit_and_env_come_first() {
        let path = path_var(&["/usr/bin"]);
        let env = env_of(&[("PATH", &path), (BROWSER_ENV, "/opt/env/chrome")]);
        let exists = files(&["/opt/mine/chrome", "/opt/env/chrome", "/usr/bin/chromium"]);
        let mine = Path::new("/opt/mine/chrome");
        assert_eq!(find_with(Some(mine), &env, &exists).unwrap(), mine);
        assert_eq!(
            find_with(None, &env, &exists).unwrap(),
            Path::new("/opt/env/chrome")
        );
        // A missing explicit browser is an error, not a fallback.
        let e = find_with(Some(Path::new("/nope/chrome")), &env, &exists).unwrap_err();
        assert!(e.to_string().contains("--browser /nope/chrome"), "{e}");
        let env2 = env_of(&[("PATH", &path), (BROWSER_ENV, "/nope")]);
        assert!(find_with(None, &env2, &exists)
            .unwrap_err()
            .to_string()
            .contains(BROWSER_ENV));
        // A bare name is looked up on PATH.
        assert_eq!(
            find_with(Some(Path::new("chromium")), &env, &exists).unwrap(),
            Path::new("/usr/bin/chromium")
        );
    }

    #[test]
    fn engines() {
        for (p, e) in [
            (
                "/Applications/Firefox.app/Contents/MacOS/firefox",
                Engine::Firefox,
            ),
            ("/usr/lib/firefox-esr/firefox-esr", Engine::Firefox),
            (
                r"C:\Program Files\Mozilla Firefox\firefox.exe",
                Engine::Firefox,
            ),
            ("FIREFOX", Engine::Firefox),
            ("/opt/google/chrome/chrome", Engine::Chromium),
            (
                "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
                Engine::Chromium,
            ),
            ("msedge.exe", Engine::Chromium),
        ] {
            assert_eq!(Engine::of(Path::new(p)), e, "{p}");
        }
    }

    #[test]
    fn firefox_is_found() {
        let home = if cfg!(windows) {
            r"C:\Users\me"
        } else {
            "/home/me"
        };
        let vars = [
            ("HOME", home),
            ("ProgramFiles", r"C:\Program Files"),
            ("LOCALAPPDATA", r"C:\Users\me\AppData\Local"),
        ];
        let installed = platform::firefox_install_paths(&env_of(&vars));
        assert!(!installed.is_empty());
        let ff = installed[0].to_string_lossy().into_owned();
        let path = path_var(&["/usr/bin"]);
        let mut with_path = vars.to_vec();
        with_path.push(("PATH", &path));
        let env = env_of(&with_path);
        // Only Firefox installed: auto-detected.
        assert_eq!(find_with(None, &env, &files(&[&ff])).unwrap(), installed[0]);
        // A Chromium-based browser comes first.
        let chrome = platform::browser_install_paths(&env)[0].clone();
        let both = files(&[&ff, &chrome.to_string_lossy()]);
        assert_eq!(find_with(None, &env, &both).unwrap(), chrome);
        // `--browser firefox`: on PATH, else installed.
        let name = platform::firefox_names()[0];
        let on_path = Path::new("/usr/bin").join(name);
        let on_path = on_path.to_string_lossy().into_owned();
        assert_eq!(
            find_with(Some(Path::new("firefox")), &env, &both).unwrap(),
            installed[0]
        );
        assert_eq!(
            find_with(
                Some(Path::new("Firefox")),
                &env,
                &files(&[&ff, &on_path, &chrome.to_string_lossy()])
            )
            .unwrap(),
            installed[0]
        );
        if !cfg!(windows) {
            assert_eq!(
                find_with(Some(Path::new("firefox")), &env, &files(&[&on_path])).unwrap(),
                Path::new(&on_path)
            );
        }
        let e = find_with(Some(Path::new("firefox")), &env, &files(&[])).unwrap_err();
        assert!(e.to_string().contains("--browser firefox"), "{e}");
        let e = find_with(None, &env, &files(&[])).unwrap_err();
        assert!(e.to_string().contains("Firefox"), "{e}");
    }

    #[test]
    fn app_bundles() {
        let exists = files(&["/Apps/Brave Browser.app/Contents/MacOS/Brave Browser"]);
        let env = env_of(&[]);
        assert_eq!(
            find_with(Some(Path::new("/Apps/Brave Browser.app")), &env, &exists).unwrap(),
            Path::new("/Apps/Brave Browser.app/Contents/MacOS/Brave Browser")
        );
        let exists = files(&["/Apps/Firefox Nightly.app/Contents/MacOS/firefox"]);
        let found = find_with(Some(Path::new("/Apps/Firefox Nightly.app")), &env, &exists).unwrap();
        assert_eq!(
            found,
            Path::new("/Apps/Firefox Nightly.app/Contents/MacOS/firefox")
        );
        assert_eq!(Engine::of(&found), Engine::Firefox);
    }

    #[test]
    fn install_paths_then_path() {
        let home = if cfg!(windows) {
            r"C:\Users\me"
        } else {
            "/home/me"
        };
        let env = env_of(&[
            ("HOME", home),
            ("ProgramFiles", r"C:\Program Files"),
            ("LOCALAPPDATA", r"C:\Users\me\AppData\Local"),
        ]);
        let installed = platform::browser_install_paths(&env);
        assert!(!installed.is_empty());
        // The second well-known path wins over PATH and later ones.
        let second = installed[1].to_string_lossy().into_owned();
        let last = installed.last().unwrap().to_string_lossy().into_owned();
        let path = path_var(&["/usr/bin"]);
        let name = platform::browser_names()[0];
        let on_path = Path::new("/usr/bin").join(name);
        let on_path = on_path.to_string_lossy().into_owned();
        let env = env_of(&[
            ("HOME", home),
            ("ProgramFiles", r"C:\Program Files"),
            ("LOCALAPPDATA", r"C:\Users\me\AppData\Local"),
            ("PATH", &path),
        ]);
        let exists = files(&[&last, &second, &on_path]);
        assert_eq!(find_with(None, &env, &exists).unwrap(), installed[1]);
        // Nothing installed: PATH.
        let exists = files(&[&on_path]);
        assert_eq!(find_with(None, &env, &exists).unwrap(), Path::new(&on_path));
        let e = find_with(None, &env, &files(&[])).unwrap_err();
        assert!(e.to_string().contains("--browser PATH"), "{e}");
    }

    #[test]
    fn known_install_paths() {
        let env = env_of(&[
            ("HOME", "/Users/me"),
            ("ProgramFiles", r"C:\Program Files"),
            ("ProgramFiles(x86)", r"C:\Program Files (x86)"),
            ("LOCALAPPDATA", r"C:\Users\me\AppData\Local"),
        ]);
        let all: Vec<String> = platform::browser_install_paths(&env)
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let firefox: Vec<String> = platform::firefox_install_paths(&env)
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        if cfg!(target_os = "macos") {
            assert_eq!(
                firefox[0],
                "/Applications/Firefox.app/Contents/MacOS/firefox"
            );
            assert!(firefox.contains(
                &"/Users/me/Applications/Firefox Nightly.app/Contents/MacOS/firefox".into()
            ));
        } else if cfg!(target_os = "windows") {
            assert_eq!(firefox[0], r"C:\Program Files\Mozilla Firefox\firefox.exe");
            assert!(
                firefox.contains(&r"C:\Users\me\AppData\Local\Mozilla Firefox\firefox.exe".into())
            );
        } else {
            assert_eq!(firefox[0], "/usr/lib/firefox/firefox");
        }
        if cfg!(target_os = "macos") {
            assert_eq!(
                all[0],
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
            );
            assert!(all.contains(
                &"/Users/me/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".into()
            ));
        } else if cfg!(target_os = "windows") {
            assert_eq!(
                all[0],
                r"C:\Program Files\Google\Chrome\Application\chrome.exe"
            );
            assert!(all
                .iter()
                .any(|p| p.ends_with(r"Edge\Application\msedge.exe")));
        } else {
            assert_eq!(all[0], "/opt/google/chrome/chrome");
        }
    }

    #[test]
    fn real_profiles_are_refused() {
        let real = vec![PathBuf::from("/home/me/.config/google-chrome")];
        assert!(check_not_real(Path::new("/home/me/.config/google-chrome"), &real).is_err());
        assert!(
            check_not_real(Path::new("/home/me/.config/google-chrome/Default"), &real).is_err()
        );
        assert!(check_not_real(Path::new("/home/me/.config"), &real).is_err());
        assert!(check_not_real(Path::new("/home/me/tvpipe-profile"), &real).is_ok());
        let env = env_of(&[
            ("HOME", "/Users/me"),
            ("LOCALAPPDATA", r"C:\L"),
            ("APPDATA", r"C:\R"),
        ]);
        let dirs = platform::default_profile_dirs(&env);
        assert!(!dirs.is_empty());
        // Firefox's profiles too.
        let firefox = if cfg!(target_os = "macos") {
            "/Users/me/Library/Application Support/Firefox"
        } else if cfg!(target_os = "windows") {
            r"C:\R\Mozilla\Firefox"
        } else {
            "/Users/me/.mozilla/firefox"
        };
        assert!(dirs.contains(&PathBuf::from(firefox)), "{dirs:?}");
    }

    #[test]
    fn temporary_profiles_are_deleted() {
        let p = Profile::temporary().unwrap();
        let dir = p.dir().to_path_buf();
        assert!(dir.is_dir());
        assert!(dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("tvpipe-browser-"));
        std::fs::write(dir.join("Cookies"), b"x").unwrap();
        std::fs::create_dir(dir.join("Default")).unwrap();
        drop(p);
        assert!(!dir.exists());
        // A persistent one stays.
        let keep = std::env::temp_dir().join(format!("tvpipe-keep-{}", std::process::id()));
        let p = Profile::persistent(&keep).unwrap();
        drop(p);
        assert!(keep.is_dir());
        std::fs::remove_dir_all(&keep).unwrap();
    }

    #[test]
    fn stale_profiles_are_swept() {
        let root = std::env::temp_dir().join(format!("tvpipe-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let mk = |name: &str, owner: Option<String>| {
            let d = root.join(name);
            std::fs::create_dir(&d).unwrap();
            if let Some(o) = owner {
                std::fs::write(d.join(OWNER_FILE), o).unwrap();
            }
            d
        };
        // Its tvpipe runs.
        let live = Profile::temporary_in(&root).unwrap();
        let owner = std::fs::read_to_string(live.dir().join(OWNER_FILE)).unwrap();
        assert!(
            owner.starts_with(&format!("{} ", std::process::id())),
            "{owner}"
        );
        // Its tvpipe was killed: no beacon, the directory and a
        // DevToolsActivePort of a browser that is gone as well.
        let mut dead = Profile::temporary_in(&root).unwrap();
        dead.beacon = None;
        dead.temporary = false;
        let dead_dir = dead.dir().to_path_buf();
        drop(dead);
        let closed = TcpListener::bind("127.0.0.1:0").unwrap();
        let gone_port = closed.local_addr().unwrap().port();
        drop(closed);
        std::fs::write(
            dead_dir.join("DevToolsActivePort"),
            format!("{gone_port}\n/devtools/browser/x\n"),
        )
        .unwrap();
        // Something else on the beacon's port (it does not say the nonce).
        let other = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = other.local_addr().unwrap().port();
        let squatted = mk(
            &format!("{TEMP_PREFIX}squatted"),
            Some(format!("1 {port} 00ff")),
        );
        let garbage = mk(&format!("{TEMP_PREFIX}garbage"), Some("x".into()));
        // Being made (no owner file yet), and not ours.
        let fresh = mk(&format!("{TEMP_PREFIX}fresh"), None);
        let unrelated = mk("tvpipe-session-1", Some("1 1 1".into()));

        assert_eq!(sweep_in(&root, ORPHAN_AGE), 3);
        for d in [&dead_dir, &squatted, &garbage] {
            assert!(!d.exists(), "{} was kept", d.display());
        }
        for d in [live.dir(), &fresh, &unrelated] {
            assert!(d.exists(), "{} was removed", d.display());
        }
        // An old directory without an owner file goes too.
        assert_eq!(sweep_in(&root, Duration::ZERO), 1);
        assert!(!fresh.exists() && live.dir().exists());
        drop(other);
        let live_dir = live.dir().to_path_buf();
        drop(live);
        assert!(!live_dir.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A browser left running by a killed tvpipe is quit, its profile
    /// removed (opt-in: TVPIPE_TEST_CHROME=/path/to/chrome).
    #[test]
    #[ignore]
    fn chrome_left_by_a_killed_tvpipe_is_swept() {
        let Ok(bin) = std::env::var("TVPIPE_TEST_CHROME") else {
            eprintln!("TVPIPE_TEST_CHROME not set");
            return;
        };
        left_by_a_killed_tvpipe_is_swept(&bin);
    }

    /// The same with Firefox (opt-in: TVPIPE_TEST_FIREFOX=/path/to/firefox).
    #[test]
    #[ignore]
    fn firefox_left_by_a_killed_tvpipe_is_swept() {
        let Ok(bin) = std::env::var("TVPIPE_TEST_FIREFOX") else {
            eprintln!("TVPIPE_TEST_FIREFOX not set");
            return;
        };
        left_by_a_killed_tvpipe_is_swept(&bin);
    }

    fn left_by_a_killed_tvpipe_is_swept(bin: &str) {
        let root = std::env::temp_dir().join(format!(
            "tvpipe-sweep-{}-{}",
            if Engine::of(Path::new(bin)) == Engine::Firefox {
                "firefox"
            } else {
                "chrome"
            },
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let mut profile = Profile::temporary_in(&root).unwrap();
        let opts = LaunchOptions {
            bin: bin.into(),
            viewport: (640, 360),
            visible: false,
        };
        let mut browser = Browser::launch(&opts, &profile).unwrap();
        // tvpipe dies: nobody closes the browser or deletes the profile.
        let mut child = browser.child.take().unwrap();
        drop(browser);
        profile.beacon = None;
        profile.temporary = false;
        let dir = profile.dir().to_path_buf();
        drop(profile);
        assert!(
            child.try_wait().unwrap().is_none(),
            "the browser should still run"
        );

        // Without an owner file (a tvpipe from before the beacons) a
        // profile with a running browser is left alone.
        let owner = std::fs::read(dir.join(OWNER_FILE)).unwrap();
        std::fs::remove_file(dir.join(OWNER_FILE)).unwrap();
        assert_eq!(sweep_in(&root, Duration::ZERO), 0);
        assert!(child.try_wait().unwrap().is_none());
        std::fs::write(dir.join(OWNER_FILE), owner).unwrap();

        assert_eq!(sweep_in(&root, ORPHAN_AGE), 1);
        assert!(!dir.exists());
        let status = child.try_wait().unwrap();
        assert!(status.is_some(), "the browser still runs");
        if !cfg!(windows) {
            let out = Command::new("ps")
                .args(["-A", "-o", "command="])
                .output()
                .unwrap();
            let needle = dir.to_string_lossy().into_owned();
            let left: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|l| l.contains(&needle))
                .map(str::to_owned)
                .collect();
            assert!(left.is_empty(), "browser processes left: {left:#?}");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn command_line() {
        let o = LaunchOptions {
            bin: "chrome".into(),
            viewport: (1280, 720),
            visible: false,
        };
        let a: Vec<String> = args(&o, Path::new("/tmp/p"))
            .into_iter()
            .map(|s| s.into_string().unwrap())
            .collect();
        for want in [
            "--headless=new",
            "--screen-info={0,0 1280x720}",
            "--user-data-dir=/tmp/p",
            "--remote-debugging-port=0",
            "--auto-accept-this-tab-capture",
            "--autoplay-policy=no-user-gesture-required",
            "--window-size=1280,720",
            "--force-device-scale-factor=1",
        ] {
            assert!(a.iter().any(|x| x == want), "{want} missing: {a:?}");
        }
        assert_eq!(a.last().unwrap(), "about:blank");
        let visible: Vec<OsString> = args(
            &LaunchOptions {
                visible: true,
                ..o.clone()
            },
            Path::new("/tmp/p"),
        );
        assert!(!visible
            .iter()
            .any(|x| x.to_string_lossy().starts_with("--headless")
                || x.to_string_lossy().starts_with("--screen-info")));

        // Firefox: its own flags, the profile as a separate argument.
        let ff = LaunchOptions {
            bin: "/opt/firefox/firefox".into(),
            ..o
        };
        assert_eq!(ff.engine(), Engine::Firefox);
        let a: Vec<String> = args(&ff, Path::new("/tmp/my p"))
            .into_iter()
            .map(|s| s.into_string().unwrap())
            .collect();
        assert_eq!(
            a,
            [
                "--headless",
                "--profile",
                "/tmp/my p",
                "--no-remote",
                "--new-instance",
                "--remote-debugging-port=0",
                "--marionette",
                "--window-size=1280,720",
                "about:blank"
            ]
        );
    }

    #[test]
    fn firefox_preferences() {
        let js = firefox_user_js();
        for want in [
            "user_pref(\"media.navigator.permission.disabled\", true);",
            "user_pref(\"media.getusermedia.browser.enabled\", true);",
            "user_pref(\"media.getusermedia.audio.capture.enabled\", true);",
            "user_pref(\"media.autoplay.default\", 0);",
            "user_pref(\"media.volume_scale\", \"0.0\");",
        ] {
            assert!(js.lines().any(|l| l == want), "{want} missing:\n{js}");
        }
        // Every line a comment or a pref with a JSON value.
        for l in js.lines().skip(1) {
            let inner = l
                .strip_prefix("user_pref(")
                .and_then(|r| r.strip_suffix(");"))
                .unwrap_or_else(|| panic!("{l}"));
            let (k, v) = inner.split_once(", ").unwrap();
            assert!(Value::parse(k).unwrap().as_str().is_some(), "{l}");
            Value::parse(v).unwrap_or_else(|e| panic!("{l}: {e}"));
        }
    }

    #[test]
    fn active_port_file() {
        assert_eq!(
            parse_active_port("53211\n/devtools/browser/0b5f-11\n"),
            Some((53211, "/devtools/browser/0b5f-11".into()))
        );
        assert_eq!(parse_active_port("53211\n"), None);
        assert_eq!(parse_active_port("0\n/devtools/browser/x"), None);
        assert_eq!(parse_active_port("x\n/devtools/browser/x"), None);
        assert_eq!(parse_active_port("1\n/etc/passwd"), None);
        assert_eq!(parse_active_port(""), None);
    }
}
