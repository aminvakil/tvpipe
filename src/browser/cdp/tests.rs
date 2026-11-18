//! The client against a fake DevTools endpoint over a real loopback
//! WebSocket: ids, results, errors, sessions, events, timeouts, garbage
//! and the browser going away.

use std::sync::mpsc;

use super::*;
use crate::net::http::Request;
use crate::net::httpd::{self, Reply};
use crate::net::websocket::{close_code, Role};

/// What the fake browser does with each command it gets.
type Script = Arc<dyn Fn(&Value, &websocket::Sender<TcpStream>) + Send + Sync>;

/// A fake browser endpoint at `ws://127.0.0.1:PORT/devtools/browser/x`;
/// every command it receives is also sent to the returned channel.
fn fake(script: Script) -> (httpd::Server, String, mpsc::Receiver<Value>) {
    let (seen_tx, seen) = mpsc::channel();
    let seen_tx = Arc::new(Mutex::new(seen_tx));
    let handler: httpd::Handler = Arc::new(move |req: &Request| {
        if req.target != "/devtools/browser/x" || !websocket::is_upgrade(req) {
            return Reply::Response(httpd::error_response(404, "Not Found"));
        }
        let resp = websocket::server_response(req).unwrap();
        let script = script.clone();
        let seen_tx = seen_tx.clone();
        Reply::Upgrade(
            resp,
            Box::new(move |tcp, rest| {
                let (mut rx, tx) =
                    websocket::pair(tcp, rest, Role::Server, websocket::Limits::default()).unwrap();
                while let Ok(Message::Text(t)) = rx.read_message() {
                    let v = Value::parse(&t).unwrap();
                    let _ = seen_tx.lock().unwrap().send(v.clone());
                    script(&v, &tx);
                }
            }),
        )
    });
    let server = httpd::Server::bind(([127, 0, 0, 1], 0).into(), handler).unwrap();
    let url = format!(
        "ws://127.0.0.1:{}/devtools/browser/x",
        server.local_addr().port()
    );
    (server, url, seen)
}

fn reply(tx: &websocket::Sender<TcpStream>, v: Value) {
    tx.send_text(&v.to_json()).unwrap();
}

fn id(cmd: &Value) -> u64 {
    cmd.get("id").unwrap().as_u64().unwrap()
}

#[test]
fn commands_get_their_answers() {
    let (_server, url, seen) = fake(Arc::new(|cmd, tx| {
        let method = cmd.get("method").unwrap().as_str().unwrap();
        match method {
            "Browser.getVersion" => {
                // Garbage and an answer to nobody first: both ignored.
                tx.send_text("{not json").unwrap();
                reply(tx, Value::object().with("id", 999).with("result", 1));
                reply(
                    tx,
                    Value::object().with("id", id(cmd)).with(
                        "result",
                        Value::object().with("product", "HeadlessChrome/157"),
                    ),
                );
            }
            "Target.attachToTarget" => reply(
                tx,
                Value::object()
                    .with("id", id(cmd))
                    .with("result", Value::object().with("sessionId", "S1")),
            ),
            "Nope.nope" => reply(
                tx,
                Value::object().with("id", id(cmd)).with(
                    "error",
                    Value::object()
                        .with("code", -32601)
                        .with("message", "'Nope.nope' wasn't found"),
                ),
            ),
            "Runtime.evaluate" => {
                let expr = cmd
                    .get("params")
                    .unwrap()
                    .get("expression")
                    .unwrap()
                    .as_str()
                    .unwrap();
                let result = if expr == "throw" {
                    Value::object()
                        .with("result", Value::object().with("type", "object"))
                        .with(
                            "exceptionDetails",
                            Value::object().with("text", "Uncaught").with(
                                "exception",
                                Value::object()
                                    .with("description", "Error: boom\n    at <anonymous>:1:7"),
                            ),
                        )
                } else {
                    Value::object().with(
                        "result",
                        Value::object().with("type", "number").with("value", 42),
                    )
                };
                reply(
                    tx,
                    Value::object().with("id", id(cmd)).with("result", result),
                );
            }
            // "Slow.call": never answered.
            _ => {}
        }
    }));
    let cdp = Cdp::connect(&url, Duration::from_secs(5)).unwrap();
    let v = cdp
        .call(None, "Browser.getVersion", Value::object())
        .unwrap();
    assert_eq!(
        v.get("product").unwrap().as_str(),
        Some("HeadlessChrome/157")
    );
    let s = cdp
        .call(
            None,
            "Target.attachToTarget",
            Value::object().with("targetId", "T").with("flatten", true),
        )
        .unwrap();
    assert_eq!(s.get("sessionId").unwrap().as_str(), Some("S1"));
    let e = cdp.call(None, "Nope.nope", Value::object()).unwrap_err();
    assert!(e.to_string().contains("wasn't found"), "{e}");
    assert_eq!(
        cdp.evaluate("S1", "6 * 7", true, true, Duration::from_secs(5))
            .unwrap()
            .as_i64(),
        Some(42)
    );
    let e = cdp
        .evaluate("S1", "throw", false, false, Duration::from_secs(5))
        .unwrap_err();
    assert!(e.to_string().contains("Error: boom") && !e.to_string().contains("anonymous"));
    let t0 = Instant::now();
    let e = cdp
        .call_timeout(
            None,
            "Slow.call",
            Value::object(),
            Duration::from_millis(200),
        )
        .unwrap_err();
    assert_eq!(e.kind(), crate::error::ErrorKind::Timeout);
    assert!(t0.elapsed() < Duration::from_secs(3));
    // What went out: ids increase, sessions only where given, params kept.
    let cmds: Vec<Value> = seen.try_iter().collect();
    let ids: Vec<u64> = cmds.iter().map(id).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");
    assert!(cmds[0].get("sessionId").is_none());
    let eval = cmds
        .iter()
        .find(|c| c.get("method").unwrap().as_str() == Some("Runtime.evaluate"))
        .unwrap();
    assert_eq!(eval.get("sessionId").unwrap().as_str(), Some("S1"));
    let p = eval.get("params").unwrap();
    assert_eq!(p.get("userGesture").unwrap().as_bool(), Some(true));
    assert_eq!(p.get("awaitPromise").unwrap().as_bool(), Some(true));
    assert_eq!(p.get("returnByValue").unwrap().as_bool(), Some(true));
}

#[test]
fn events_arrive_in_order_and_closing_ends_everything() {
    let (_server, url, _seen) = fake(Arc::new(|cmd, tx| {
        if cmd.get("method").unwrap().as_str() == Some("Page.enable") {
            for n in 0..3 {
                reply(
                    tx,
                    Value::object()
                        .with("method", "Page.frameNavigated")
                        .with("params", Value::object().with("n", n))
                        .with("sessionId", "S1"),
                );
            }
            reply(
                tx,
                Value::object()
                    .with("id", id(cmd))
                    .with("result", Value::object()),
            );
            reply(
                tx,
                Value::object()
                    .with("method", "Target.targetCrashed")
                    .with("params", Value::object().with("targetId", "T")),
            );
            // Then the browser goes away.
            let _ = tx.close(close_code::GOING_AWAY, "bye");
        }
    }));
    let cdp = Cdp::connect(&url, Duration::from_secs(5)).unwrap();
    assert_eq!(cdp.next_event(Duration::from_millis(50)).unwrap(), None);
    cdp.call(Some("S1"), "Page.enable", Value::object())
        .unwrap();
    let mut got = Vec::new();
    loop {
        match cdp.next_event(Duration::from_secs(5)) {
            Ok(Some(ev)) => got.push(ev),
            Ok(None) => panic!("no close"),
            Err(e) => {
                assert_eq!(e.kind(), crate::error::ErrorKind::Closed);
                break;
            }
        }
    }
    assert_eq!(got.len(), 4);
    for (n, ev) in got.iter().take(3).enumerate() {
        assert_eq!(ev.method, "Page.frameNavigated");
        assert_eq!(ev.session.as_deref(), Some("S1"));
        assert_eq!(ev.params.get("n").unwrap().as_u64(), Some(n as u64));
    }
    assert_eq!(got[3].method, "Target.targetCrashed");
    assert_eq!(got[3].session, None);
    assert!(cdp.closed().is_some());
    let e = cdp
        .call(None, "Browser.close", Value::object())
        .unwrap_err();
    assert_eq!(e.kind(), crate::error::ErrorKind::Closed);
}

#[test]
fn a_call_in_flight_fails_when_the_browser_dies() {
    let (_server, url, _seen) = fake(Arc::new(|cmd, tx| {
        if cmd.get("method").unwrap().as_str() == Some("Browser.close") {
            // The browser exits without answering.
            tx.with_stream(|s| {
                let _ = s.shutdown(std::net::Shutdown::Both);
            });
        }
    }));
    let cdp = Cdp::connect(&url, Duration::from_secs(5)).unwrap();
    let t0 = Instant::now();
    let e = cdp
        .call(None, "Browser.close", Value::object())
        .unwrap_err();
    assert_eq!(e.kind(), crate::error::ErrorKind::Closed, "{e}");
    assert!(t0.elapsed() < Duration::from_secs(5));
}

#[test]
fn connecting_to_nothing_fails() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    assert!(Cdp::connect(
        &format!("ws://127.0.0.1:{port}/devtools/browser/x"),
        Duration::from_secs(2)
    )
    .is_err());
}

#[test]
fn eval_results() {
    let ok = Value::parse(r#"{"result":{"type":"string","value":"sharing"}}"#).unwrap();
    assert_eq!(eval_result(&ok).unwrap().as_str(), Some("sharing"));
    let undefined = Value::parse(r#"{"result":{"type":"undefined"}}"#).unwrap();
    assert_eq!(eval_result(&undefined).unwrap(), Value::Null);
    let thrown = Value::parse(
        r#"{"result":{},"exceptionDetails":{"text":"Uncaught ReferenceError: x is not defined"}}"#,
    )
    .unwrap();
    assert!(eval_result(&thrown)
        .unwrap_err()
        .to_string()
        .contains("x is not defined"));
}
