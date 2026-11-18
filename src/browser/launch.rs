//! Finding and starting a Chromium-based browser for `tvpipe browser`.
//!
//! - [`find_browser`]: `--browser PATH` (or a name on `PATH`, or a macOS
//!   `.app`), `$TVPIPE_BROWSER`, the well-known install paths
//!   (`browser::platform`), then `PATH`.
//! - [`Profile`]: the browser's own user-data directory: a fresh temp
//!   directory (deleted on drop) or `--profile DIR` (kept, for logins).
//!   Never the user's real browser profile.
//! - [`Browser::launch`]: starts it with `--remote-debugging-port=0`,
//!   reads the port from `DevToolsActivePort` in the profile, connects
//!   [`Cdp`] to the browser endpoint. [`Browser::close`] (and drop) asks
//!   it to quit (`Browser.close`), waits, then kills it.
//!
//! Sources: Chrome DevTools Protocol docs; Chromium command-line switches
//! (`--remote-debugging-port`, `--user-data-dir`, `--headless`,
//! `--auto-accept-this-tab-capture`, `--autoplay-policy`, ...).

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::browser::cdp::Cdp;
use crate::browser::platform;
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

/// Finds a Chromium-based browser (see the module docs for the order).
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
    let given = |p: &Path, what: &str| -> Result<PathBuf> {
        if exists(p) {
            return Ok(p.to_path_buf());
        }
        if let Some(exe) = app_bundle_executable(p).filter(|e| exists(e)) {
            return Ok(exe);
        }
        if p.components().count() == 1 {
            if let Some(found) = on_path(p) {
                return Ok(found);
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
    Err(Error::not_found(format!(
        "no Chromium-based browser found (Chrome, Edge, Chromium or Brave); \
         install one or name it with --browser PATH or ${BROWSER_ENV}"
    )))
}

/// `X.app` → `X.app/Contents/MacOS/X` (a macOS bundle given as the
/// browser).
fn app_bundle_executable(p: &Path) -> Option<PathBuf> {
    let name = p.file_name()?.to_str()?.strip_suffix(".app")?;
    Some(p.join("Contents/MacOS").join(name))
}

/// The browser's user-data directory.
#[derive(Debug)]
pub struct Profile {
    dir: PathBuf,
    temporary: bool,
}

impl Profile {
    /// A fresh directory under the system's temp directory, deleted when
    /// dropped.
    pub fn temporary() -> Result<Profile> {
        let name = format!(
            "tvpipe-browser-{}",
            crate::util::hex::encode(&crate::crypto::ring::random_array::<6>()?)
        );
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir(&dir).map_err(|e| Error::io(format!("create {}", dir.display()), e))?;
        Ok(Profile {
            dir,
            temporary: true,
        })
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

/// How to start the browser.
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub bin: PathBuf,
    /// The page's viewport (CSS pixels at scale 1): the stream size.
    pub viewport: (u32, u32),
    /// A normal window instead of `--headless=new`.
    pub visible: bool,
}

/// The command line for `opts` with profile `dir`, opening `about:blank`
/// (the caller sets the page up before it loads the real URL).
pub fn args(opts: &LaunchOptions, dir: &Path) -> Vec<OsString> {
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

/// A running browser and its DevTools connection.
pub struct Browser {
    child: Option<Child>,
    cdp: Cdp,
    /// The product (`Chrome/157.0...`, `HeadlessChrome/...`).
    pub product: String,
    pub user_agent: String,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

impl Browser {
    /// Starts the browser with `profile` and connects to it.
    pub fn launch(opts: &LaunchOptions, profile: &Profile) -> Result<Browser> {
        let port_file = profile.dir().join("DevToolsActivePort");
        // A persistent profile may hold the last run's.
        let _ = std::fs::remove_file(&port_file);
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
        let (port, path) = loop {
            if let Some(p) = std::fs::read_to_string(&port_file)
                .ok()
                .and_then(|s| parse_active_port(&s))
            {
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
                    "the browser did not open its DevTools port within {} s{}",
                    START_TIMEOUT.as_secs(),
                    tail(&stderr)
                )));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let url = format!("ws://127.0.0.1:{port}{path}");
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
            product: String::new(),
            user_agent: String::new(),
            stderr,
        };
        let v = b.cdp.call(None, "Browser.getVersion", Value::object())?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
        b.product = s("product");
        b.user_agent = s("userAgent");
        debug!("browser: {} on DevTools port {port}", b.product);
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
            let _ = self.cdp.call_timeout(
                None,
                "Browser.close",
                Value::object(),
                Duration::from_secs(2),
            );
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
    fn app_bundles() {
        let exists = files(&["/Apps/Brave Browser.app/Contents/MacOS/Brave Browser"]);
        let env = env_of(&[]);
        assert_eq!(
            find_with(Some(Path::new("/Apps/Brave Browser.app")), &env, &exists).unwrap(),
            Path::new("/Apps/Brave Browser.app/Contents/MacOS/Brave Browser")
        );
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
        let env = env_of(&[("HOME", "/Users/me"), ("LOCALAPPDATA", r"C:\L")]);
        let dirs = platform::default_profile_dirs(&env);
        assert!(!dirs.is_empty());
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
        let visible: Vec<OsString> =
            args(&LaunchOptions { visible: true, ..o }, Path::new("/tmp/p"));
        assert!(!visible
            .iter()
            .any(|x| x.to_string_lossy().starts_with("--headless")
                || x.to_string_lossy().starts_with("--screen-info")));
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
