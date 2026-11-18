//! The OS-specific bits of the browser modes (`cfg(target_os)` is allowed
//! here, like a config path): which command opens a URL, where
//! Chromium-based browsers are installed, and where their own profiles
//! live (which `tvpipe browser` never uses).

use std::ffi::OsString;
use std::path::PathBuf;
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

/// Opens `url` in the default browser (does not wait for it).
pub fn open_url(url: &str) -> Result<()> {
    // Nothing a shell (cmd's `start`) would interpret.
    if !url.starts_with("http://") || url.bytes().any(|b| b <= b' ' || b"\"^&|<>%".contains(&b)) {
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
/// defaults), which `tvpipe browser --profile` refuses: tvpipe never
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
            ] {
                out.push(config.join(d));
            }
        }
    }
    out
}
