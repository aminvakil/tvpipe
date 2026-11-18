//! A minimal Chrome DevTools Protocol client (`tvpipe share URL`).
//!
//! JSON-RPC over our WebSocket client (`net::websocket::connect`):
//! a command is `{id, method, params, sessionId?}`, the browser answers
//! with the same `id` and a `result` or an `error`; a message without an
//! `id` is an event. One connection to the browser's endpoint
//! (`/devtools/browser/<id>`, from `DevToolsActivePort`) drives every
//! target through flattened sessions (`Target.attachToTarget` with
//! `flatten: true`: the target's messages carry its `sessionId`).
//!
//! A reader thread owns the socket's receiving half: answers wake the
//! caller waiting for that id, events go to a bounded queue (the oldest
//! are dropped when nobody reads them). Nothing the browser sends can
//! panic the client; a malformed message is logged and skipped.
//!
//! Sources: Chrome DevTools Protocol docs
//! (chromedevtools.github.io/devtools-protocol: Target, Page, Runtime,
//! Browser), RFC 6455.

use std::collections::{HashMap, VecDeque};
use std::net::TcpStream;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::codec::json::Value;
use crate::error::{Error, Result};
use crate::net::websocket::{self, Message};

/// How long a command may take by default.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// Events kept for a slow reader before the oldest are dropped.
const EVENT_QUEUE: usize = 4096;

/// An event from the browser.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub method: String,
    pub params: Value,
    /// The flattened session it belongs to (`None`: the browser's).
    pub session: Option<String>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    /// Commands in flight: `None` until answered.
    waiting: HashMap<u64, Option<Result<Value>>>,
    events: VecDeque<Event>,
    dropped_events: u64,
    /// Why the connection is gone.
    closed: Option<String>,
}

struct Inner {
    tx: websocket::Sender<TcpStream>,
    state: Mutex<State>,
    cv: Condvar,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A DevTools connection.
pub struct Cdp {
    inner: Arc<Inner>,
    reader: Option<JoinHandle<()>>,
}

impl Cdp {
    /// Connects to a DevTools WebSocket URL (`ws://127.0.0.1:PORT/devtools/...`).
    pub fn connect(url: &str, timeout: Duration) -> Result<Cdp> {
        let (mut rx, tx) = websocket::connect(url, timeout, websocket::Limits::default())?;
        let inner = Arc::new(Inner {
            tx,
            state: Mutex::new(State::default()),
            cv: Condvar::new(),
        });
        let me = inner.clone();
        let reader = std::thread::Builder::new()
            .name("cdp".into())
            .spawn(move || {
                let why = loop {
                    match rx.read_message() {
                        Ok(Message::Text(t)) => me.dispatch(&t),
                        Ok(Message::Binary(_)) => trace!("cdp: binary message ignored"),
                        Ok(Message::Close(_)) => break "the browser closed the connection".into(),
                        Err(e) => break format!("the connection to the browser failed: {e}"),
                    }
                };
                let mut st = me.lock();
                st.closed = Some(why);
                me.cv.notify_all();
            })
            .map_err(|e| Error::io("spawn cdp reader", e))?;
        Ok(Cdp {
            inner,
            reader: Some(reader),
        })
    }

    /// Runs `method` (on `session`, or the browser) and returns its
    /// `result`; a protocol `error` becomes an error.
    pub fn call(&self, session: Option<&str>, method: &str, params: Value) -> Result<Value> {
        self.call_timeout(session, method, params, CALL_TIMEOUT)
    }

    pub fn call_timeout(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let id = {
            let mut st = self.inner.lock();
            if let Some(why) = &st.closed {
                return Err(Error::closed(format!("cdp: {method}: {why}")));
            }
            st.next_id += 1;
            let id = st.next_id;
            st.waiting.insert(id, None);
            id
        };
        let msg = Value::object()
            .with("id", id)
            .with("method", method)
            .with("params", params)
            .with_opt("sessionId", session);
        trace!("cdp> {}", short(&msg.to_json()));
        if let Err(e) = self.inner.tx.send_text(&msg.to_json()) {
            self.inner.lock().waiting.remove(&id);
            return Err(e);
        }
        let deadline = Instant::now() + timeout;
        let mut st = self.inner.lock();
        loop {
            if let Some(Some(_)) = st.waiting.get(&id) {
                if let Some(Some(r)) = st.waiting.remove(&id) {
                    return r
                        .map_err(|e| Error::protocol(format!("cdp: {method}: {}", e.message())));
                }
            }
            if let Some(why) = st.closed.clone() {
                st.waiting.remove(&id);
                return Err(Error::closed(format!("cdp: {method}: {why}")));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                st.waiting.remove(&id);
                return Err(Error::timeout(format!(
                    "cdp: {method}: no answer within {} ms",
                    timeout.as_millis()
                )));
            }
            st = self
                .inner
                .cv
                .wait_timeout(st, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// The next event, waiting up to `timeout`: `Ok(None)` when none came,
    /// an error once the connection is gone and every event was read.
    pub fn next_event(&self, timeout: Duration) -> Result<Option<Event>> {
        let deadline = Instant::now() + timeout;
        let mut st = self.inner.lock();
        loop {
            if st.dropped_events > 0 {
                debug!(
                    "cdp: {} events dropped (nobody read them)",
                    st.dropped_events
                );
                st.dropped_events = 0;
            }
            if let Some(ev) = st.events.pop_front() {
                return Ok(Some(ev));
            }
            if let Some(why) = &st.closed {
                return Err(Error::closed(why.clone()));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            st = self
                .inner
                .cv
                .wait_timeout(st, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Why the connection is gone, if it is.
    pub fn closed(&self) -> Option<String> {
        self.inner.lock().closed.clone()
    }

    /// Evaluates `expression` in the session's page and returns its value
    /// (`returnByValue`). `gesture`: as if the user clicked (transient
    /// activation, which `getDisplayMedia` needs); `wait`: await a
    /// returned promise. A thrown exception becomes an error.
    pub fn evaluate(
        &self,
        session: &str,
        expression: &str,
        gesture: bool,
        wait: bool,
        timeout: Duration,
    ) -> Result<Value> {
        let r = self.call_timeout(
            Some(session),
            "Runtime.evaluate",
            Value::object()
                .with("expression", expression)
                .with("returnByValue", true)
                .with("userGesture", gesture)
                .with("awaitPromise", wait),
            timeout,
        )?;
        eval_result(&r)
    }
}

/// The value of a `Runtime.evaluate` result, or the exception it threw.
pub fn eval_result(r: &Value) -> Result<Value> {
    if let Some(ex) = r.get("exceptionDetails") {
        let text = ex
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(Value::as_str)
            .or_else(|| ex.get("text").and_then(Value::as_str))
            .unwrap_or("exception");
        return Err(Error::protocol(format!(
            "page script: {}",
            text.lines().next().unwrap_or(text)
        )));
    }
    Ok(r.get("result")
        .and_then(|v| v.get("value"))
        .cloned()
        .unwrap_or(Value::Null))
}

impl Inner {
    /// One message from the browser: an answer or an event.
    fn dispatch(&self, text: &str) {
        trace!("cdp< {}", short(text));
        let v = match Value::parse(text) {
            Ok(v) => v,
            Err(e) => {
                debug!("cdp: unparsable message: {e}");
                return;
            }
        };
        let mut st = self.lock();
        if let Some(id) = v.get("id").and_then(Value::as_u64) {
            if let Some(slot) = st.waiting.get_mut(&id) {
                *slot = Some(match v.get("error") {
                    Some(err) => Err(Error::protocol(
                        err.get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("error")
                            .to_owned(),
                    )),
                    None => Ok(v.get("result").cloned().unwrap_or_else(Value::object)),
                });
                self.cv.notify_all();
            }
            return;
        }
        let Some(method) = v.get("method").and_then(Value::as_str) else {
            return;
        };
        if st.events.len() >= EVENT_QUEUE {
            st.events.pop_front();
            st.dropped_events += 1;
        }
        st.events.push_back(Event {
            method: method.to_owned(),
            params: v.get("params").cloned().unwrap_or_else(Value::object),
            session: v
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
        self.cv.notify_all();
    }
}

/// A message shortened for traces.
fn short(s: &str) -> &str {
    match s.char_indices().nth(300) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

impl Drop for Cdp {
    fn drop(&mut self) {
        let _ = self.inner.tx.close(websocket::close_code::NORMAL, "");
        self.inner.tx.with_stream(|s| {
            let _ = s.shutdown(std::net::Shutdown::Both);
        });
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
    }
}

#[cfg(test)]
mod tests;
