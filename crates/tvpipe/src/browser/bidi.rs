//! WebDriver BiDi, the protocol tvpipe drives Firefox with (`tvpipe share
//! URL` with Firefox; Firefox dropped its DevTools protocol).
//!
//! The JSON-RPC connection is [`Cdp`] (the message shapes match, see
//! `cdp`); this module has what is BiDi's own:
//!
//! - the endpoint: Firefox started with `--remote-debugging-port=0`
//!   writes `WebDriverBiDiServer.json` (`{"ws_host", "ws_port"}`) into its
//!   profile; the BiDi-only session lives at `ws://HOST:PORT/session` and
//!   ends when that connection closes ([`server_file`], [`session_url`]);
//! - `session.new` and the browser's name and user agent ([`new_session`]);
//! - `script.evaluate` and its serialized results ([`evaluate`]);
//! - the response-header filter that drops a page's
//!   Content-Security-Policy (BiDi has no `setBypassCSP`; tvpipe
//!   intercepts responses instead, [`csp_hook`]);
//! - [`marionette_quit`]: Firefox keeps a BiDi-only session after its
//!   connection closed (seen in Firefox 160) and takes one session at a
//!   time, so a Firefox whose tvpipe was killed cannot be asked to quit
//!   over BiDi. Its Marionette port (`--marionette`, `marionette.port` 0:
//!   the port in `MarionetteActivePort` in the profile) takes
//!   `Marionette:Quit` without a session.
//!
//! Sources: W3C WebDriver BiDi (session, browsingContext, script, network,
//! log modules; remote value serialization); Firefox's remote agent docs
//! (`WebDriverBiDiServer.json`, `--remote-debugging-port`); Firefox's
//! Marionette protocol docs (`LEN:JSON` packets, `[0, id, command,
//! params]`, `Marionette:Quit`).

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::path::Path;
use std::time::Duration;

use crate::browser::cdp::{Cdp, Event, EventHook};
use crate::codec::json::Value;
use crate::error::{Error, Result};

/// The file Firefox writes into its profile once the BiDi server listens.
pub const SERVER_FILE: &str = "WebDriverBiDiServer.json";

/// The BiDi port Firefox wrote into profile `dir`, if any.
pub fn server_file(dir: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(dir.join(SERVER_FILE)).ok()?;
    parse_server_file(&text)
}

/// `{"ws_host": "127.0.0.1", "ws_port": N}`: the port, when the host is
/// loopback.
pub fn parse_server_file(text: &str) -> Option<u16> {
    let v = Value::parse(text).ok()?;
    let host = v.get("ws_host").and_then(Value::as_str)?;
    if !matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]") {
        return None;
    }
    let port = v.get("ws_port").and_then(Value::as_u64)?;
    u16::try_from(port).ok().filter(|&p| p > 0)
}

/// The file Firefox started with `--marionette` writes into its profile.
pub const MARIONETTE_FILE: &str = "MarionetteActivePort";

/// The Marionette port Firefox wrote into profile `dir`, if any.
pub fn marionette_port(dir: &Path) -> Option<u16> {
    std::fs::read_to_string(dir.join(MARIONETTE_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|&p| p > 0)
}

/// Largest Marionette packet read.
const MARIONETTE_MAX: usize = 1 << 20;

/// One Marionette packet (`LEN:JSON`).
fn marionette_read(r: &mut impl Read) -> Result<Value> {
    let mut len = 0usize;
    let mut digits = 0;
    loop {
        let mut b = [0u8; 1];
        r.read_exact(&mut b)
            .map_err(|e| Error::io("marionette: read", e))?;
        match b[0] {
            b'0'..=b'9' if digits < 8 => {
                len = len * 10 + (b[0] - b'0') as usize;
                digits += 1;
            }
            b':' if digits > 0 => break,
            _ => return Err(Error::protocol("marionette: bad packet length")),
        }
    }
    if len > MARIONETTE_MAX {
        return Err(Error::protocol("marionette: packet too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)
        .map_err(|e| Error::io("marionette: read", e))?;
    crate::codec::json::parse_bytes(&body)
}

/// Asks the Firefox on Marionette port `port` to quit (no session
/// needed); returns once it said it will.
pub fn marionette_quit(port: u16, timeout: Duration) -> Result<()> {
    let mut c = TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, port).into(), timeout)
        .map_err(|e| Error::io("marionette: connect", e))?;
    let _ = c.set_read_timeout(Some(timeout));
    let _ = c.set_write_timeout(Some(timeout));
    // `{"applicationType": "gecko", "marionetteProtocol": 3}`
    let hello = marionette_read(&mut c)?;
    if hello.get("applicationType").and_then(Value::as_str) != Some("gecko") {
        return Err(Error::protocol("marionette: not Firefox"));
    }
    c.write_all(&marionette_packet(
        1,
        "Marionette:Quit",
        Value::object().with("flags", Value::array(["eForceQuit"])),
    ))
    .map_err(|e| Error::io("marionette: write", e))?;
    // `[1, 1, error, result]`
    let reply = marionette_read(&mut c)?;
    match reply.at(2) {
        Some(Value::Null) | None => Ok(()),
        Some(e) => Err(Error::protocol(format!(
            "marionette: {}",
            e.get("message").and_then(Value::as_str).unwrap_or("error")
        ))),
    }
}

/// A Marionette command packet: `LEN:[0, id, command, params]`.
fn marionette_packet(id: u64, command: &str, params: Value) -> Vec<u8> {
    let body = Value::array([
        Value::from(0),
        Value::from(id),
        Value::from(command),
        params,
    ])
    .to_json();
    format!("{}:{body}", body.len()).into_bytes()
}

/// The URL of a new BiDi-only session on `port`.
pub fn session_url(port: u16) -> String {
    format!("ws://127.0.0.1:{port}/session")
}

/// What `session.new` tells about the browser.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionInfo {
    /// `Firefox/160.0.1`.
    pub product: String,
    pub user_agent: String,
}

/// Starts the session (one per connection). User prompts (alerts,
/// `beforeunload`) are dismissed by the browser and reported as events.
pub fn new_session(rpc: &Cdp) -> Result<SessionInfo> {
    let caps = Value::object().with(
        "alwaysMatch",
        Value::object().with(
            "unhandledPromptBehavior",
            Value::object()
                .with("default", "dismiss and notify")
                .with("beforeUnload", "accept"),
        ),
    );
    let r = rpc.call(
        None,
        "session.new",
        Value::object().with("capabilities", caps),
    )?;
    let c = r.get("capabilities").cloned().unwrap_or(Value::Null);
    let s = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let mut name = s("browserName");
    if let Some(first) = name.get(..1) {
        name = first.to_ascii_uppercase() + &name[1..];
    }
    Ok(SessionInfo {
        product: format!("{name}/{}", s("browserVersion")),
        user_agent: s("userAgent"),
    })
}

/// Evaluates `expression` in the page of browsing context `context` and
/// returns its value: `activation` as if the user clicked (transient
/// activation, which capture needs), `wait` awaits a returned promise. A
/// thrown exception becomes an error.
pub fn evaluate(
    rpc: &Cdp,
    context: &str,
    expression: &str,
    activation: bool,
    wait: bool,
    timeout: Duration,
) -> Result<Value> {
    let r = rpc.call_timeout(
        None,
        "script.evaluate",
        Value::object()
            .with("expression", expression)
            .with("target", Value::object().with("context", context))
            .with("awaitPromise", wait)
            .with("userActivation", activation),
        timeout,
    )?;
    eval_result(&r)
}

/// The value of a `script.evaluate` result, or the exception it threw.
pub fn eval_result(r: &Value) -> Result<Value> {
    if r.get("type").and_then(Value::as_str) == Some("exception") {
        let d = r.get("exceptionDetails").cloned().unwrap_or(Value::Null);
        let text = d.get("text").and_then(Value::as_str).unwrap_or("exception");
        return Err(Error::protocol(format!(
            "page script: {}",
            text.lines().next().unwrap_or(text)
        )));
    }
    Ok(remote_value(r.get("result").unwrap_or(&Value::Null)))
}

/// A serialized remote value as JSON: primitives, arrays and plain
/// objects with string keys; anything else (nodes, functions, ...) is
/// null.
pub fn remote_value(v: &Value) -> Value {
    let val = v.get("value");
    match v.get("type").and_then(Value::as_str).unwrap_or("") {
        "string" | "boolean" => val.cloned().unwrap_or(Value::Null),
        // Non-finite numbers are strings ("NaN", "Infinity").
        "number" => match val {
            Some(n @ Value::Number(_)) => n.clone(),
            _ => Value::Null,
        },
        "array" => Value::Array(
            val.and_then(Value::as_array)
                .unwrap_or(&[])
                .iter()
                .map(remote_value)
                .collect(),
        ),
        "object" => {
            let mut o = Value::object();
            for pair in val.and_then(Value::as_array).unwrap_or(&[]) {
                if let (Some(k), Some(v)) = (pair.at(0).and_then(Value::as_str), pair.at(1)) {
                    o = o.with(k, remote_value(v));
                }
            }
            o
        }
        _ => Value::Null,
    }
}

/// Answers `network.responseStarted` events of intercepted responses:
/// lets them continue without their `Content-Security-Policy` headers
/// (capture.js loads blob: workers and a worklet and connects to
/// ws://127.0.0.1, which a page's policy may forbid). Every other header
/// is kept.
pub fn csp_hook() -> EventHook {
    Box::new(|ev: &Event| {
        if ev.method != "network.responseStarted" {
            return None;
        }
        let p = &ev.params;
        if p.get("isBlocked").and_then(Value::as_bool) != Some(true) {
            return None;
        }
        let request = p
            .get("request")
            .and_then(|r| r.get("request"))
            .and_then(Value::as_str)?;
        let headers = p
            .get("response")
            .and_then(|r| r.get("headers"))
            .and_then(Value::as_array)
            .unwrap_or(&[]);
        let (kept, dropped) = without_csp(headers);
        let mut params = Value::object().with("request", request);
        if dropped {
            params = params.with("headers", Value::Array(kept));
        }
        Some(("network.continueResponse".to_owned(), params))
    })
}

/// The headers without the CSP ones (`Content-Security-Policy`, also
/// `-Report-Only`), and whether there were any.
fn without_csp(headers: &[Value]) -> (Vec<Value>, bool) {
    let is_csp = |h: &Value| {
        h.get("name").and_then(Value::as_str).is_some_and(|n| {
            n.to_ascii_lowercase()
                .starts_with("content-security-policy")
        })
    };
    let kept: Vec<Value> = headers.iter().filter(|h| !is_csp(h)).cloned().collect();
    let dropped = kept.len() != headers.len();
    (kept, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_files() {
        assert_eq!(
            parse_server_file("{\n  \"ws_host\": \"127.0.0.1\",\n  \"ws_port\": 59764\n}"),
            Some(59764)
        );
        assert_eq!(
            parse_server_file(r#"{"ws_host":"localhost","ws_port":1}"#),
            Some(1)
        );
        for bad in [
            r#"{"ws_host":"10.0.0.1","ws_port":1}"#,
            r#"{"ws_host":"127.0.0.1","ws_port":0}"#,
            r#"{"ws_host":"127.0.0.1","ws_port":70000}"#,
            r#"{"ws_port":1}"#,
            "",
            "garbage",
        ] {
            assert_eq!(parse_server_file(bad), None, "{bad}");
        }
        assert_eq!(session_url(5), "ws://127.0.0.1:5/session");
    }

    #[test]
    fn results() {
        let ok = |t: &str| eval_result(&Value::parse(t).unwrap());
        assert_eq!(
            ok(r#"{"realm":"r","type":"success","result":{"type":"string","value":"sharing"}}"#)
                .unwrap(),
            Value::from("sharing")
        );
        assert_eq!(
            ok(r#"{"type":"success","result":{"type":"number","value":2}}"#).unwrap(),
            Value::from(2)
        );
        assert_eq!(
            ok(r#"{"type":"success","result":{"type":"number","value":"NaN"}}"#).unwrap(),
            Value::Null
        );
        assert_eq!(
            ok(r#"{"type":"success","result":{"type":"undefined"}}"#).unwrap(),
            Value::Null
        );
        assert_eq!(
            ok(r#"{"type":"success","result":{"type":"object","value":[["a",{"type":"boolean","value":true}],["b",{"type":"array","value":[{"type":"null"},{"type":"string","value":"x"}]}]]}}"#)
                .unwrap()
                .to_json(),
            r#"{"a":true,"b":[null,"x"]}"#
        );
        let e = ok(r#"{"type":"exception","exceptionDetails":{"text":"ReferenceError: x is not defined\nmore","lineNumber":0}}"#)
            .unwrap_err();
        assert!(
            e.to_string()
                .contains("page script: ReferenceError: x is not defined"),
            "{e}"
        );
        assert!(!e.to_string().contains("more"));
        // Garbage is null, not a panic.
        assert_eq!(ok("{}").unwrap(), Value::Null);
        assert_eq!(
            ok(r#"{"type":"success","result":{"type":"object","value":[[1,2]]}}"#).unwrap(),
            Value::object()
        );
    }

    #[test]
    fn marionette_packets() {
        assert_eq!(
            marionette_packet(1, "Marionette:Quit", Value::object()),
            b"26:[0,1,\"Marionette:Quit\",{}]".to_vec()
        );
        let read = |b: &[u8]| marionette_read(&mut &b[..]);
        assert_eq!(
            read(b"50:{\"applicationType\":\"gecko\",\"marionetteProtocol\":3}")
                .unwrap()
                .get("marionetteProtocol"),
            Some(&Value::from(3))
        );
        assert!(read(b"5:[1,1").is_err());
        assert!(read(b":[]").is_err());
        assert!(read(b"x:[]").is_err());
        assert!(read(b"999999999:[]").is_err());
        assert!(read(b"99999999:[]").is_err());
        assert!(read(b"").is_err());
    }

    /// A fake Marionette server: the hello, then the quit's answer.
    #[test]
    fn marionette_quit_asks_once() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            c.write_all(b"50:{\"applicationType\":\"gecko\",\"marionetteProtocol\":3}")
                .unwrap();
            let cmd = marionette_read(&mut c).unwrap();
            c.write_all(b"46:[1,1,null,{\"cause\":\"shutdown\",\"forced\":false}]")
                .unwrap();
            cmd
        });
        marionette_quit(port, Duration::from_secs(5)).unwrap();
        let cmd = t.join().unwrap();
        assert_eq!(cmd.at(2).and_then(Value::as_str), Some("Marionette:Quit"));
        assert_eq!(cmd.at(3).unwrap().to_json(), r#"{"flags":["eForceQuit"]}"#);
    }

    fn event(params: &str) -> Event {
        Event {
            method: "network.responseStarted".into(),
            params: Value::parse(params).unwrap(),
            session: None,
        }
    }

    #[test]
    fn csp_headers_are_dropped_from_intercepted_responses() {
        let hook = csp_hook();
        let (method, params) = hook(&event(
            r#"{"isBlocked":true,"request":{"request":"7","url":"https://x/"},"response":{"headers":[
                {"name":"Content-Type","value":{"type":"string","value":"text/html"}},
                {"name":"content-security-policy","value":{"type":"string","value":"default-src 'self'"}},
                {"name":"Content-Security-Policy-Report-Only","value":{"type":"string","value":"x"}}]}}"#,
        ))
        .unwrap();
        assert_eq!(method, "network.continueResponse");
        assert_eq!(
            params.to_json(),
            r#"{"request":"7","headers":[{"name":"Content-Type","value":{"type":"string","value":"text/html"}}]}"#
        );
        // Nothing to drop: continued as it is.
        let (_, params) = hook(&event(
            r#"{"isBlocked":true,"request":{"request":"8"},"response":{"headers":[]}}"#,
        ))
        .unwrap();
        assert_eq!(params.to_json(), r#"{"request":"8"}"#);
        // Not blocked, another event, garbage: nothing to answer.
        assert!(hook(&event(r#"{"isBlocked":false,"request":{"request":"9"}}"#)).is_none());
        assert!(hook(&event(r#"{"isBlocked":true}"#)).is_none());
        assert!(hook(&Event {
            method: "log.entryAdded".into(),
            params: Value::object(),
            session: None
        })
        .is_none());
    }
}
