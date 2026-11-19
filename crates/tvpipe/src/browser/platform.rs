//! The OS-specific bits of the browser modes (`cfg(target_os)` is allowed
//! here, like a config path): which command opens a URL, where
//! Chromium-based browsers and Firefox are installed, where their own
//! profiles live (which `tvpipe share URL` never uses), and how `tvpipe
//! screen` starts the browser on macOS (as an app of its own:
//! [`own_app_command`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

/// The opener command and its arguments for `url`.
fn opener(url: &str) -> (&'static str, Vec<String>) {
    if cfg!(target_os = "macos") {
        ("open", vec![url.to_owned()])
    } else if cfg!(target_os = "windows") {
        // `start` is a cmd builtin; the empty string is the window title.
        (
            "cmd",
            vec!["/C".into(), "start".into(), String::new(), url.to_owned()],
        )
    } else {
        ("xdg-open", vec![url.to_owned()])
    }
}

/// Whether [`open_url`] opens `url`: `http://` or `https://`, nothing a
/// shell (cmd's `start`) would interpret.
pub fn can_open_url(url: &str) -> bool {
    (url.starts_with("http://") || url.starts_with("https://"))
        && !url.bytes().any(|b| b <= b' ' || b"\"^&|<>%".contains(&b))
}

/// Opens `url` in the default browser (does not wait for it).
pub fn open_url(url: &str) -> Result<()> {
    if !can_open_url(url) {
        return Err(Error::usage(format!("not opening {url:?}")));
    }
    let (cmd, args) = opener(url);
    let mut child = Command::new(cmd)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::io(format!("run {cmd}"), e))?;
    // Reap it in the background; the opener exits quickly.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Well-known install locations of Chromium-based browsers on this OS,
/// most preferred first (Chrome, Edge, Chromium, Brave, Chrome Canary).
/// `env` reads an environment variable (`HOME`, `ProgramFiles`, ...).
pub fn browser_install_paths(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        const APPS: &[(&str, &str)] = &[
            ("Google Chrome.app", "Google Chrome"),
            ("Microsoft Edge.app", "Microsoft Edge"),
            ("Chromium.app", "Chromium"),
            ("Brave Browser.app", "Brave Browser"),
            ("Google Chrome Canary.app", "Google Chrome Canary"),
        ];
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = env("HOME") {
            roots.push(PathBuf::from(home).join("Applications"));
        }
        for (app, exe) in APPS {
            for root in &roots {
                out.push(root.join(app).join("Contents/MacOS").join(exe));
            }
        }
    } else if cfg!(target_os = "windows") {
        const EXES: &[&str] = &[
            r"Google\Chrome\Application\chrome.exe",
            r"Microsoft\Edge\Application\msedge.exe",
            r"Chromium\Application\chrome.exe",
            r"BraveSoftware\Brave-Browser\Application\brave.exe",
            r"Google\Chrome SxS\Application\chrome.exe",
        ];
        let roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .iter()
            .filter_map(|v| env(v))
            .map(PathBuf::from)
            .collect();
        for exe in EXES {
            for root in &roots {
                out.push(root.join(exe));
            }
        }
    } else {
        for p in [
            "/opt/google/chrome/chrome",
            "/opt/microsoft/msedge/msedge",
            "/usr/lib/chromium/chromium",
            "/opt/brave.com/brave/brave",
        ] {
            out.push(PathBuf::from(p));
        }
    }
    out
}

/// Well-known install locations of Firefox on this OS, most preferred
/// first (release, then Developer Edition and Nightly).
pub fn firefox_install_paths(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = env("HOME") {
            roots.push(PathBuf::from(home).join("Applications"));
        }
        for app in [
            "Firefox.app",
            "Firefox Developer Edition.app",
            "Firefox Nightly.app",
        ] {
            for root in &roots {
                out.push(root.join(app).join("Contents/MacOS/firefox"));
            }
        }
    } else if cfg!(target_os = "windows") {
        let roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .iter()
            .filter_map(|v| env(v))
            .map(PathBuf::from)
            .collect();
        for dir in [
            "Mozilla Firefox",
            "Firefox Developer Edition",
            "Firefox Nightly",
        ] {
            for root in &roots {
                out.push(root.join(dir).join("firefox.exe"));
            }
        }
    } else {
        // Distribution packages and Mozilla's tarball in /opt. (Snap's
        // /snap/bin/firefox is left out: its sandbox cannot read a
        // profile in /tmp.)
        for p in [
            "/usr/lib/firefox/firefox",
            "/usr/lib64/firefox/firefox",
            "/usr/lib/firefox-esr/firefox-esr",
            "/opt/firefox/firefox",
        ] {
            out.push(PathBuf::from(p));
        }
    }
    out
}

/// Names of Firefox to look for on `PATH`.
pub fn firefox_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["firefox.exe"]
    } else {
        &["firefox", "firefox-esr"]
    }
}

/// Names of Chromium-based browsers to look for on `PATH`.
pub fn browser_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["chrome.exe", "msedge.exe", "chromium.exe", "brave.exe"]
    } else {
        &[
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
            "microsoft-edge-stable",
            "brave-browser",
            "brave",
        ]
    }
}

/// The browsers' own default profile directories (`--user-data-dir`
/// defaults), which `tvpipe share URL --profile` refuses: tvpipe never
/// touches the user's real browser profile.
pub fn default_profile_dirs(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        if let Some(home) = env("HOME") {
            let support = PathBuf::from(home).join("Library/Application Support");
            for d in [
                "Google/Chrome",
                "Google/Chrome Canary",
                "Microsoft Edge",
                "Chromium",
                "BraveSoftware/Brave-Browser",
                "Firefox",
            ] {
                out.push(support.join(d));
            }
        }
    } else if cfg!(target_os = "windows") {
        if let Some(local) = env("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            for d in [
                r"Google\Chrome\User Data",
                r"Google\Chrome SxS\User Data",
                r"Microsoft\Edge\User Data",
                r"Chromium\User Data",
                r"BraveSoftware\Brave-Browser\User Data",
            ] {
                out.push(local.join(d));
            }
        }
        if let Some(roaming) = env("APPDATA") {
            out.push(PathBuf::from(roaming).join(r"Mozilla\Firefox"));
        }
    } else {
        let config = env("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".config")));
        if let Some(config) = config {
            for d in [
                "google-chrome",
                "google-chrome-beta",
                "chromium",
                "microsoft-edge",
                "BraveSoftware/Brave-Browser",
                "mozilla/firefox",
            ] {
                out.push(config.join(d));
            }
        }
        if let Some(home) = env("HOME") {
            out.push(PathBuf::from(home).join(".mozilla/firefox"));
        }
    }
    out
}

/// The `.app` bundle `bin` is the executable of
/// (`X.app/Contents/MacOS/NAME` → `X.app`).
pub fn app_bundle_of(bin: &Path) -> Option<PathBuf> {
    let macos = bin.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    let is_app = app
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("app"));
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && is_app)
        .then(|| app.to_path_buf())
}

/// `open -n [-g] -a APP --stderr FILE --args ARGS...`: starts a new
/// instance of the app bundle as an app of its own.
pub fn open_app_args(
    app: &Path,
    args: &[OsString],
    background: bool,
    stderr: &Path,
) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["-n".into()];
    if background {
        a.push("-g".into());
    }
    a.extend(["-a".into(), app.into(), "--stderr".into(), stderr.into()]);
    a.push("--args".into());
    a.extend(args.iter().cloned());
    a
}

/// macOS: the command that starts the browser `bin` through LaunchServices
/// (`open`), for `tvpipe screen`. macOS attributes a process's screen and
/// system-audio recording to its "responsible" app: a browser started
/// directly is the terminal's child, so the terminal would need the
/// permissions, and without the terminal's "System Audio Recording"
/// permission Chrome's system audio is silent. Started by `open`, the
/// browser asks for and keeps the permissions itself, as when the user
/// starts it. `None` on other systems, or when `bin` is not in an app
/// bundle (it is then started directly).
pub fn own_app_command(
    bin: &Path,
    args: &[OsString],
    background: bool,
    stderr: &Path,
) -> Option<Command> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let app = app_bundle_of(bin)?;
    let mut c = Command::new("open");
    c.args(open_app_args(&app, args, background, stderr));
    Some(c)
}

/// Kills process `pid` (a browser started by [`own_app_command`] that does
/// not quit; it is no child of ours).
pub fn kill_pid(pid: u32) {
    if cfg!(unix) {
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openable_urls() {
        assert!(can_open_url("http://127.0.0.1:8765/?t=abc"));
        assert!(can_open_url("https://example.trycloudflare.com/x"));
        for u in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "http://x/?a=1&b=2",
            "http://x/%20",
            "http://x/ y",
            "https://x/\"q\"",
        ] {
            assert!(!can_open_url(u), "{u}");
        }
    }

    #[test]
    fn app_bundles() {
        let chrome = Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
        assert_eq!(
            app_bundle_of(chrome),
            Some(PathBuf::from("/Applications/Google Chrome.app"))
        );
        assert_eq!(app_bundle_of(Path::new("/usr/bin/chromium")), None);
        assert_eq!(app_bundle_of(Path::new("Google Chrome")), None);
        assert_eq!(
            app_bundle_of(Path::new("/x/Chromium.app/Contents/Resources/chromium")),
            None
        );
        let a: Vec<String> = open_app_args(
            Path::new("/A/Chrome.app"),
            &["--x=1".into(), "about:blank".into()],
            true,
            Path::new("/p/stderr.log"),
        )
        .into_iter()
        .map(|s| s.into_string().unwrap())
        .collect();
        assert_eq!(
            a,
            [
                "-n",
                "-g",
                "-a",
                "/A/Chrome.app",
                "--stderr",
                "/p/stderr.log",
                "--args",
                "--x=1",
                "about:blank"
            ]
        );
    }
}
