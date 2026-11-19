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
    /// Pages the window's pages open (links with `target="_blank"`,
    /// `window.open`) go to the system's browser (app windows only).
    pass_out: bool,
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
                        scale: 1.0,
                        visible: mode == WindowMode::App,
                        screen: None,
                        app: Some(url.to_owned()),
                    };
                    let profile = Profile::temporary()?;
                    let browser = Browser::launch(&opts, &profile)?;
                    debug!("playshare: window in {}", browser.product);
                    let pass_out = mode == WindowMode::App
                        && browser
                            .cdp()
                            .call(
                                None,
                                "Target.setDiscoverTargets",
                                Value::object().with("discover", true),
                            )
                            .inspect_err(|e| debug!("playshare: target discovery: {e}"))
                            .is_ok();
                    return Ok((
                        Window {
                            browser: Some((browser, profile)),
                            pass_out,
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
        let none = Window {
            browser: None,
            pass_out: false,
        };
        if matches!(mode, WindowMode::App | WindowMode::Open) {
            crate::browser::platform::open_url(url)?;
            return Ok((none, WindowMode::Open));
        }
        Ok((none, WindowMode::None))
    }

    /// Opens the pages the window's pages opened ("Open the TV view")
    /// in the system's browser instead and closes them there: tvpipe's
    /// browser is no place to keep or share them from. Only pages with an
    /// opener: what the system routes to this browser (it may count as
    /// the user's, docs/cross-platform.md) stays, else it would come back.
    pub fn pass_out_new_pages(&mut self) {
        if !self.pass_out {
            return;
        }
        let Some((b, _)) = &self.browser else { return };
        let cdp = b.cdp();
        while let Ok(Some(ev)) = cdp.next_event(std::time::Duration::ZERO) {
            let Some((id, url)) = opened_page(&ev.method, &ev.params) else {
                continue;
            };
            if !crate::browser::platform::can_open_url(&url) {
                debug!("playshare: the window opened {url:?}; leaving it there");
                continue;
            }
            match crate::browser::platform::open_url(&url) {
                Ok(()) => {
                    info!("playshare: opened {url} in the system's browser");
                    let _ = cdp.call(
                        None,
                        "Target.closeTarget",
                        Value::object().with("targetId", id),
                    );
                }
                Err(e) => warn!("playshare: cannot open {url} in the system's browser: {e}"),
            }
        }
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

/// A page another page opened, once it has its URL (Chrome reports it
/// first without one): its target and URL.
fn opened_page(method: &str, params: &Value) -> Option<(String, String)> {
    if !matches!(method, "Target.targetCreated" | "Target.targetInfoChanged") {
        return None;
    }
    let info = params.get("targetInfo")?;
    let s = |k: &str| info.get(k).and_then(Value::as_str).unwrap_or("");
    if s("type") != "page" || s("openerId").is_empty() {
        return None;
    }
    let url = s("url");
    if url.is_empty() || url == "about:blank" {
        return None;
    }
    Some((s("targetId").to_owned(), url.to_owned()))
}

impl Drop for Window {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(url: &str, opener: Option<&str>, kind: &str) -> Value {
        let mut info = Value::object()
            .with("targetId", "T2")
            .with("type", kind)
            .with("url", url);
        if let Some(o) = opener {
            info = info.with("openerId", o);
        }
        Value::object().with("targetInfo", info)
    }

    /// As Chrome 157 reports a link with target="_blank": created without
    /// a URL, then changed to it.
    #[test]
    fn opened_pages() {
        let player = "http://127.0.0.1:8765/?t=abc";
        assert_eq!(
            opened_page("Target.targetCreated", &ev("", Some("T1"), "page")),
            None
        );
        assert_eq!(
            opened_page("Target.targetInfoChanged", &ev(player, Some("T1"), "page")),
            Some(("T2".into(), player.into()))
        );
        assert_eq!(
            opened_page("Target.targetCreated", &ev(player, Some("T1"), "page")),
            Some(("T2".into(), player.into()))
        );
        // No opener: the window itself, or a URL the system sent here.
        assert_eq!(
            opened_page("Target.targetInfoChanged", &ev(player, None, "page")),
            None
        );
        assert_eq!(
            opened_page(
                "Target.targetInfoChanged",
                &ev("about:blank", Some("T1"), "page")
            ),
            None
        );
        assert_eq!(
            opened_page(
                "Target.targetInfoChanged",
                &ev(player, Some("T1"), "worker")
            ),
            None
        );
        assert_eq!(
            opened_page("Target.targetDestroyed", &ev(player, Some("T1"), "page")),
            None
        );
    }
}
