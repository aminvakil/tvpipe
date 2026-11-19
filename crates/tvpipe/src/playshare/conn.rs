//! One WebSocket on two threads: the reader (the HTTP server's connection
//! thread after the upgrade) hands messages to a callback; the writer
//! drains an outbox, so nobody blocks on a slow peer. The outbox counts
//! its bytes (the peer's backlog) and reports crossing a mark.

use std::collections::VecDeque;
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::net::websocket::{self, Message, Role};

/// The reader's timeout: a period without a byte from the peer.
pub(crate) const PING_EVERY: Duration = Duration::from_secs(15);
/// The writer pings this often (whatever else it sends), so a live peer's
/// pong always comes within the reader's timeout.
const PING_INTERVAL: Duration = Duration::from_millis(7500);
/// Silent periods (not even a pong) after which the connection is dead.
const DEAD_AFTER: u32 = 3;
/// After our close frame, how long the peer has to answer.
const CLOSE_LINGER: Duration = Duration::from_secs(2);

enum Item {
    Text(String),
    Binary(Vec<u8>),
    Close(u16, String),
}

impl Item {
    fn len(&self) -> usize {
        match self {
            Item::Text(t) => t.len(),
            Item::Binary(b) => b.len(),
            Item::Close(..) => 0,
        }
    }
}

/// Called with `true` when the backlog goes above the mark, `false` when
/// it is back under it.
pub(crate) type MarkFn = Box<dyn Fn(bool) + Send + Sync>;

struct State {
    queue: VecDeque<Item>,
    bytes: usize,
    /// The connection ended: nothing more is sent.
    done: bool,
    /// A close was queued: nothing more is queued.
    closing: bool,
    above: bool,
}

struct Outbox {
    state: Mutex<State>,
    cv: Condvar,
    mark: Option<(usize, MarkFn)>,
    /// What the mark callback was told last; reports are made under this
    /// lock from the current state, so they alternate and end right.
    reported: Mutex<bool>,
    /// The backlog past which the peer is closed (`limit_code`).
    limit: Option<(usize, u16, &'static str)>,
}

impl Outbox {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Tells the mark callback the current side of the mark, if it moved.
    fn report(&self) {
        let Some((_, f)) = &self.mark else { return };
        let mut told = self.reported.lock().unwrap_or_else(|e| e.into_inner());
        let above = self.lock().above;
        if above != *told {
            *told = above;
            f(above);
        }
    }
}

/// The sending side of a connection.
#[derive(Clone)]
pub(crate) struct Out(Arc<Outbox>);

impl Out {
    fn new(mark: Option<(usize, MarkFn)>, limit: Option<(usize, u16, &'static str)>) -> Out {
        Out(Arc::new(Outbox {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                bytes: 0,
                done: false,
                closing: false,
                above: false,
            }),
            cv: Condvar::new(),
            mark,
            reported: Mutex::new(false),
            limit,
        }))
    }

    fn push(&self, item: Item) -> bool {
        let mut crossed = false;
        {
            let mut st = self.0.lock();
            if st.done || st.closing {
                return false;
            }
            st.bytes += item.len();
            if let Some((limit, code, reason)) = self.0.limit {
                if st.bytes > limit {
                    st.queue.clear();
                    st.bytes = 0;
                    st.closing = true;
                    st.queue.push_back(Item::Close(code, reason.to_owned()));
                    self.0.cv.notify_all();
                    return false;
                }
            }
            st.queue.push_back(item);
            if let Some((mark, _)) = &self.0.mark {
                if !st.above && st.bytes > *mark {
                    st.above = true;
                    crossed = true;
                }
            }
            self.0.cv.notify_all();
        }
        if crossed {
            self.0.report();
        }
        true
    }

    pub fn text(&self, text: String) -> bool {
        self.push(Item::Text(text))
    }

    pub fn binary(&self, data: Vec<u8>) -> bool {
        self.push(Item::Binary(data))
    }

    /// Sends what is queued, then a close frame.
    pub fn close(&self, code: u16, reason: &str) {
        let mut st = self.0.lock();
        if st.done || st.closing {
            return;
        }
        st.closing = true;
        st.queue.push_back(Item::Close(code, reason.to_owned()));
        self.0.cv.notify_all();
    }

    /// Bytes queued and not written yet.
    pub fn queued(&self) -> usize {
        self.0.lock().bytes
    }

    pub fn closed(&self) -> bool {
        let st = self.0.lock();
        st.done || st.closing
    }
}

/// How [`run`] treats a connection.
pub(crate) struct Options {
    pub name: &'static str,
    pub max_message: usize,
    /// The first message must come within this.
    pub first_message: Option<Duration>,
    /// `busy`/`drain` reports.
    pub mark: Option<(usize, MarkFn)>,
    /// Close the peer (code, reason) when this much waits for it.
    pub limit: Option<(usize, u16, &'static str)>,
}

/// Why the reader stopped, for the close report.
#[derive(Debug, Clone)]
pub(crate) struct Ended {
    pub code: u16,
    pub reason: String,
}

/// What a message callback wants next.
pub(crate) enum Next {
    Continue,
    /// Close with this code and reason, then stop reading.
    Close(u16, String),
}

/// Runs an upgraded connection: `start` gets the outbox before anything is
/// read (to register it), `on_message` gets each message; returns when
/// the socket ended, with the peer's close code (1006 when it vanished).
pub(crate) fn run(
    tcp: TcpStream,
    leftover: Vec<u8>,
    opts: Options,
    start: impl FnOnce(&Out),
    mut on_message: impl FnMut(Message) -> Next,
) -> Ended {
    let _ = tcp.set_nodelay(true);
    let first = opts.first_message.unwrap_or(PING_EVERY);
    let out = Out::new(opts.mark, opts.limit);
    let (Ok(shutdown), Ok(())) = (tcp.try_clone(), tcp.set_read_timeout(Some(first))) else {
        return Ended {
            code: 1006,
            reason: String::new(),
        };
    };
    let limits = websocket::Limits {
        max_message: opts.max_message,
    };
    let Ok((mut rx, tx)) = websocket::pair(tcp, leftover, Role::Server, limits) else {
        return Ended {
            code: 1006,
            reason: String::new(),
        };
    };
    let writer = {
        let (out, shutdown) = (out.clone(), shutdown.try_clone().ok());
        std::thread::Builder::new()
            .name(format!("{}-write", opts.name))
            .spawn(move || write_loop(&out, &tx, shutdown))
    };
    let Ok(writer) = writer else {
        return Ended {
            code: 1006,
            reason: String::new(),
        };
    };
    start(&out);
    let mut got_first = opts.first_message.is_none();
    let mut silent = 0;
    let mut ended = Ended {
        code: 1006,
        reason: String::new(),
    };
    loop {
        match rx.read_message() {
            Ok(Message::Close(c)) => {
                if let Some((code, reason)) = c {
                    ended = Ended { code, reason };
                } else {
                    ended.code = 1005;
                }
                break;
            }
            Ok(m) => {
                silent = 0;
                if !got_first {
                    got_first = true;
                    let _ = rx.with_stream(|s| s.set_read_timeout(Some(PING_EVERY)));
                }
                if let Next::Close(code, reason) = on_message(m) {
                    out.close(code, &reason);
                    ended = Ended { code, reason };
                    // Wait for the writer to send the close (and the
                    // peer's answer, briefly).
                    let deadline = Instant::now() + CLOSE_LINGER;
                    let _ = rx.with_stream(|s| s.set_read_timeout(Some(CLOSE_LINGER)));
                    while Instant::now() < deadline {
                        match rx.read_message() {
                            Ok(Message::Close(_)) | Err(_) => break,
                            Ok(_) => {}
                        }
                    }
                    break;
                }
            }
            Err(e) if e.kind() == crate::error::ErrorKind::Timeout => {
                if !got_first {
                    out.close(4002, "no message");
                    ended = Ended {
                        code: 4002,
                        reason: "no message".into(),
                    };
                    std::thread::sleep(Duration::from_millis(50));
                    break;
                }
                silent += 1;
                if silent >= DEAD_AFTER {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    {
        let mut st = out.0.lock();
        // Let a queued close go out before the writer stops.
        if !st.closing {
            st.queue.clear();
            st.bytes = 0;
        }
        st.done = true;
        out.0.cv.notify_all();
    }
    let _ = writer.join();
    let _ = shutdown.shutdown(Shutdown::Both);
    ended
}

fn write_loop(out: &Out, tx: &websocket::Sender<TcpStream>, shutdown: Option<TcpStream>) {
    // A ping every PING_EVERY even while data flows: the reader counts
    // silence from the peer, and a peer that only receives answers pings.
    let mut last_ping = Instant::now();
    loop {
        if last_ping.elapsed() >= PING_INTERVAL {
            last_ping = Instant::now();
            if tx.ping(b"").is_err() {
                return;
            }
        }
        let (item, drained) = {
            let mut st = out.0.lock();
            let deadline = last_ping + PING_INTERVAL;
            while st.queue.is_empty() && !st.done {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                st = out
                    .0
                    .cv
                    .wait_timeout(st, left)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            if st.done && !matches!(st.queue.front(), Some(Item::Close(..))) {
                return;
            }
            let item = st.queue.pop_front();
            if let Some(i) = &item {
                st.bytes -= i.len();
            }
            let mut drained = false;
            if let Some((mark, _)) = &out.0.mark {
                if st.above && st.bytes <= *mark {
                    st.above = false;
                    drained = true;
                }
            }
            (item, drained)
        };
        if drained {
            out.0.report();
        }
        let ok = match item {
            None => continue,
            Some(Item::Text(t)) => tx.send_text(&t).is_ok(),
            Some(Item::Binary(b)) => tx.send_binary(&b).is_ok(),
            Some(Item::Close(code, reason)) => {
                let _ = tx.close(code, &reason);
                let st = out.0.lock();
                let (st, _) = out
                    .0
                    .cv
                    .wait_timeout_while(st, CLOSE_LINGER, |st| !st.done)
                    .unwrap_or_else(|e| e.into_inner());
                if !st.done {
                    if let Some(s) = &shutdown {
                        let _ = s.shutdown(Shutdown::Both);
                    }
                }
                return;
            }
        };
        if !ok {
            if let Some(s) = &shutdown {
                let _ = s.shutdown(Shutdown::Both);
            }
            return;
        }
    }
}

/// Messages per second with a burst (a token bucket).
pub(crate) struct Bucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl Bucket {
    pub fn new(rate: u32, burst: u32) -> Bucket {
        Bucket {
            rate: f64::from(rate),
            burst: f64::from(burst),
            tokens: f64::from(burst),
            last: Instant::now(),
        }
    }

    pub fn take(&mut self) -> bool {
        self.take_at(Instant::now())
    }

    pub fn take_at(&mut self, now: Instant) -> bool {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * self.rate).min(self.burst);
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

/// At most `limit` events per key within a window (upgrades per address).
pub(crate) struct WindowCounter {
    limit: usize,
    window: Duration,
    hits: std::collections::HashMap<String, VecDeque<Instant>>,
}

impl WindowCounter {
    pub fn new(limit: usize, window: Duration) -> WindowCounter {
        WindowCounter {
            limit,
            window,
            hits: Default::default(),
        }
    }

    /// Counts one; `false` (not counted) when over the limit.
    pub fn hit(&mut self, key: &str, now: Instant) -> bool {
        if self.hits.len() > 10_000 {
            let window = self.window;
            self.hits
                .retain(|_, v| v.back().is_some_and(|t| now.duration_since(*t) < window));
        }
        let q = self.hits.entry(key.to_owned()).or_default();
        while q
            .front()
            .is_some_and(|t| now.duration_since(*t) >= self.window)
        {
            q.pop_front();
        }
        if q.len() >= self.limit {
            return false;
        }
        q.push_back(now);
        true
    }
}
