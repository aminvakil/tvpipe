//! The Firefox side of `tvpipe share URL`: its tab driven over WebDriver
//! BiDi (`bidi`) where the Chromium side uses the DevTools protocol.
//!
//! - [`attach`]: the tab's browsing context; events on; responses
//!   intercepted so their Content-Security-Policy can be dropped (the
//!   reader thread answers them, `bidi::csp_hook`); the viewport set to
//!   the stream size at scale 1 (`browsingContext.setViewport`, which the
//!   tab capture follows, headless or not).
//! - [`event`]: BiDi events as the keeper's [`Seen`] (a new document, a
//!   crash: Firefox shows `about:tabcrashed` in the tab, the tab closed,
//!   the page's errors and status).
//!
//! The capture itself is capture.js's Firefox path: getUserMedia's
//! `browser` (this tab) and `audioCapture` (its sound) sources, which the
//! profile's preferences enable (`launch::FIREFOX_PREFS`).
//!
//! Sources: W3C WebDriver BiDi (browsingContext, script, network, log,
//! session modules); Firefox's BiDi implementation notes (events it emits
//! for crashed tabs and failed navigations, as observed in Firefox 160).

use std::time::Duration;

use crate::browser::bidi;
use crate::browser::cdp::{Cdp, Event};
use crate::browser::launch::Browser;
use crate::codec::json::Value;
use crate::error::{Error, Result};

/// The events the keeper follows.
const EVENTS: [&str; 7] = [
    "browsingContext.domContentLoaded",
    "browsingContext.navigationCommitted",
    "browsingContext.contextDestroyed",
    "browsingContext.userPromptOpened",
    "log.entryAdded",
    "network.responseStarted",
    "network.fetchError",
];

/// Sets the browser's tab up for sharing itself; returns its browsing
/// context.
pub fn attach(browser: &Browser, viewport: (u32, u32)) -> Result<String> {
    let rpc = browser.cdp();
    let tree = rpc.call(
        None,
        "browsingContext.getTree",
        Value::object().with("maxDepth", 0),
    )?;
    let contexts = tree
        .get("contexts")
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    let url = |c: &Value| {
        c.get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    // The tab it started with (about:blank), else the first.
    let context = contexts
        .iter()
        .find(|c| url(c) == "about:blank")
        .or_else(|| contexts.first())
        .and_then(|c| c.get("context").and_then(Value::as_str))
        .ok_or_else(|| Error::protocol("browser: no tab to show the page in"))?
        .to_owned();
    rpc.call(
        None,
        "session.subscribe",
        Value::object().with("events", Value::array(EVENTS)),
    )?;
    // capture.js's worker and worklet are blob: URLs and it connects to
    // ws://127.0.0.1: a page's CSP must not stop either.
    rpc.set_event_hook(bidi::csp_hook());
    if let Err(e) = rpc.call(
        None,
        "network.addIntercept",
        Value::object()
            .with("phases", Value::array(["responseStarted"]))
            .with("contexts", Value::array([context.as_str()])),
    ) {
        warn!(
            "browser: cannot lift pages' Content-Security-Policy ({e}); pages that forbid \
             connections to 127.0.0.1 cannot share"
        );
    }
    set_viewport(rpc, &context, viewport)?;
    Ok(context)
}

/// Makes the page's viewport exactly `want` CSS pixels at scale 1.
pub fn set_viewport(rpc: &Cdp, context: &str, want: (u32, u32)) -> Result<()> {
    rpc.call(
        None,
        "browsingContext.setViewport",
        Value::object()
            .with("context", context)
            .with(
                "viewport",
                Value::object().with("width", want.0).with("height", want.1),
            )
            .with("devicePixelRatio", 1),
    )?;
    Ok(())
}

/// Opens `url` in the tab (returns once it is committed; a page that
/// cannot be loaded is an error).
pub fn navigate(rpc: &Cdp, context: &str, url: &str) -> Result<()> {
    rpc.call_timeout(
        None,
        "browsingContext.navigate",
        Value::object()
            .with("context", context)
            .with("url", url)
            .with("wait", "none"),
        Duration::from_secs(30),
    )?;
    Ok(())
}

pub fn reload(rpc: &Cdp, context: &str) -> Result<()> {
    rpc.call(
        None,
        "browsingContext.reload",
        Value::object()
            .with("context", context)
            .with("wait", "none"),
    )?;
    Ok(())
}

/// What an event means for the keeper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seen {
    /// A new document is ready for the capture script.
    Ready {
        url: String,
    },
    /// A new document (not yet ready).
    Committed {
        url: String,
    },
    /// The tab's content process died (Firefox shows its crash page).
    Crashed,
    /// The tab is gone.
    Closed,
    /// The document itself answered with an error status.
    Status {
        url: String,
        status: u64,
        text: String,
    },
    Nothing,
}

/// What `ev` means for the tab `context`; logs the page's errors and
/// dialogs.
pub fn event(ev: &Event, context: &str) -> Seen {
    let p = &ev.params;
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let ours = |v: &Value| v.get("context").and_then(Value::as_str) == Some(context);
    let navigation = p.get("navigation").is_some_and(|n| !n.is_null());
    match ev.method.as_str() {
        "browsingContext.domContentLoaded" if ours(p) => {
            let url = s(p, "url");
            if url.starts_with("about:tabcrashed") {
                Seen::Nothing
            } else {
                Seen::Ready { url }
            }
        }
        "browsingContext.navigationCommitted" if ours(p) => {
            let url = s(p, "url");
            if url.starts_with("about:tabcrashed") {
                Seen::Crashed
            } else {
                Seen::Committed { url }
            }
        }
        "browsingContext.contextDestroyed" if ours(p) => Seen::Closed,
        "browsingContext.userPromptOpened" if ours(p) => {
            info!(
                "browser: the page opened a {} dialog ({:?}); dismissing it",
                s(p, "type"),
                s(p, "message")
            );
            Seen::Nothing
        }
        "log.entryAdded" => {
            let src = p.get("source").cloned().unwrap_or(Value::Null);
            let level = s(p, "level");
            if ours(&src) && matches!(level.as_str(), "error" | "warn") {
                debug!("page {level}: {}", s(p, "text"));
            }
            Seen::Nothing
        }
        "network.responseStarted" if ours(p) && navigation => {
            let r = p.get("response").cloned().unwrap_or(Value::Null);
            let status = r.get("status").and_then(Value::as_u64).unwrap_or(0);
            if status >= 400 {
                Seen::Status {
                    url: s(&r, "url"),
                    status,
                    text: s(&r, "statusText"),
                }
            } else {
                Seen::Nothing
            }
        }
        "network.fetchError" if ours(p) && navigation => {
            debug!("browser: a document did not load: {}", s(p, "errorText"));
            Seen::Nothing
        }
        _ => Seen::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(method: &str, params: &str) -> Event {
        Event {
            method: method.into(),
            params: Value::parse(params).unwrap(),
            session: None,
        }
    }

    #[test]
    fn events() {
        let ctx = "c1";
        let seen = |m: &str, p: &str| event(&ev(m, p), ctx);
        assert_eq!(
            seen(
                "browsingContext.domContentLoaded",
                r#"{"context":"c1","url":"https://x/","navigation":"n"}"#
            ),
            Seen::Ready {
                url: "https://x/".into()
            }
        );
        assert_eq!(
            seen(
                "browsingContext.domContentLoaded",
                r#"{"context":"other","url":"https://x/"}"#
            ),
            Seen::Nothing
        );
        assert_eq!(
            seen(
                "browsingContext.navigationCommitted",
                r#"{"context":"c1","url":"about:tabcrashed?e=tabcrashed&u=x"}"#
            ),
            Seen::Crashed
        );
        assert_eq!(
            seen(
                "browsingContext.navigationCommitted",
                r#"{"context":"c1","url":"https://y/"}"#
            ),
            Seen::Committed {
                url: "https://y/".into()
            }
        );
        assert_eq!(
            seen(
                "browsingContext.domContentLoaded",
                r#"{"context":"c1","url":"about:tabcrashed?e=tabcrashed"}"#
            ),
            Seen::Nothing
        );
        assert_eq!(
            seen("browsingContext.contextDestroyed", r#"{"context":"c1"}"#),
            Seen::Closed
        );
        assert_eq!(
            seen("browsingContext.contextDestroyed", r#"{"context":"c2"}"#),
            Seen::Nothing
        );
        assert_eq!(
            seen(
                "network.responseStarted",
                r#"{"context":"c1","navigation":"n","response":{"url":"https://x/","status":403,"statusText":"Forbidden"}}"#
            ),
            Seen::Status {
                url: "https://x/".into(),
                status: 403,
                text: "Forbidden".into()
            }
        );
        // A subresource's error is not the page's.
        assert_eq!(
            seen(
                "network.responseStarted",
                r#"{"context":"c1","navigation":null,"response":{"url":"https://x/a.png","status":404}}"#
            ),
            Seen::Nothing
        );
        assert_eq!(seen("log.entryAdded", "{}"), Seen::Nothing);
        assert_eq!(seen("anything", "[]"), Seen::Nothing);
    }
}
