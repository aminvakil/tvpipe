//! The window the launcher and the host page run in (§10): an app window
//! of tvpipe's own Chromium-based browser (a temporary profile, over
//! DevTools, so its closing is noticed), else the system's browser.

use std::path::PathBuf;

use crate::browser::launch::{find_browser, Browser, Engine, LaunchOptions, Profile};
use crate::codec::json::Value;
use crate::error::Result;

/// How the window is opened (`--window`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowMode {
    /// An app window of tvpipe's own browser (the default).
    #[default]
    App,
    /// The system's browser (`open`, `xdg-open`, `start`).
    Open,
    /// Nothing: the URL is printed.
    None,
    /// tvpipe's own browser, headless (tests).
    Headless,
}

impl WindowMode {
    pub fn parse(s: &str) -> Option<WindowMode> {
        match s {
            "app" => Some(WindowMode::App),
            "open" => Some(WindowMode::Open),
            "none" => Some(WindowMode::None),
            "headless" => Some(WindowMode::Headless),
            _ => None,
        }
    }
}

/// The window's size (CSS pixels).
pub const SIZE: (u32, u32) = (1100, 760);

/// An open window.
pub struct Window {
    browser: Option<(Browser, Profile)>,
}

impl Window {
    /// Opens `url`; returns what was actually done (an app window may
    /// fall back to the system's browser).
    pub fn open(
        mode: WindowMode,
        url: &str,
        bin: Option<&PathBuf>,
    ) -> Result<(Window, WindowMode)> {
        if matches!(mode, WindowMode::App | WindowMode::Headless) {
            match find_browser(bin.map(|p| p.as_path())) {
                Ok(bin) if Engine::of(&bin) == Engine::Chromium => {
                    let opts = LaunchOptions {
                        bin,
                        viewport: SIZE,
                        visible: mode == WindowMode::App,
                        screen: None,
                        app: Some(url.to_owned()),
                    };
                    let profile = Profile::temporary()?;
                    let browser = Browser::launch(&opts, &profile)?;
                    debug!("playshare: window in {}", browser.product);
                    return Ok((
                        Window {
                            browser: Some((browser, profile)),
                        },
                        mode,
                    ));
                }
                Ok(bin) => info!(
                    "playshare: {} has no app windows; opening the system's browser",
                    bin.display()
                ),
                Err(e) => info!("playshare: {e}; opening the system's browser"),
            }
            if mode == WindowMode::Headless {
                return Err(crate::error::Error::not_found(
                    "playshare: --window headless needs a Chromium-based browser",
                ));
            }
        }
        if matches!(mode, WindowMode::App | WindowMode::Open) {
            crate::browser::platform::open_url(url)?;
            return Ok((Window { browser: None }, WindowMode::Open));
        }
        Ok((Window { browser: None }, WindowMode::None))
    }

    /// Whether we own the window (its closing is noticed).
    pub fn owned(&self) -> bool {
        self.browser.is_some()
    }

    /// Whether the window was closed (no page left, or the browser quit).
    pub fn closed(&mut self) -> bool {
        let Some((b, _)) = &mut self.browser else {
            return false;
        };
        if b.exited().is_some() || b.cdp().closed().is_some() {
            return true;
        }
        match b.cdp().call_timeout(
            None,
            "Target.getTargets",
            Value::object(),
            std::time::Duration::from_secs(5),
        ) {
            Ok(v) => !v
                .get("targetInfos")
                .and_then(Value::as_array)
                .is_some_and(|t| {
                    t.iter()
                        .any(|i| i.get("type").and_then(Value::as_str) == Some("page"))
                }),
            Err(_) => b.cdp().closed().is_some(),
        }
    }

    /// The DevTools connection (tests drive the window with it).
    pub fn browser(&self) -> Option<&Browser> {
        self.browser.as_ref().map(|(b, _)| b)
    }

    /// Quits the browser and removes its profile.
    pub fn close(&mut self) {
        if let Some((mut b, profile)) = self.browser.take() {
            b.close();
            drop(profile);
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.close();
    }
}
