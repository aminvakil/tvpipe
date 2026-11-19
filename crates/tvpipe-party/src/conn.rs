//! One WebSocket per phone or screen, on two threads: the reader (the
//! HTTP server's connection thread after the upgrade) passes messages to
//! the party with their arrival time; the writer drains an outbox, so the
//! game's thread never blocks on a slow phone. The outbox's size is the
//! party's `bufferedAmount` (backpressure, docs/party-design.md §5).

use std::collections::VecDeque;
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tvpipe::__web::websocket::{self, Message, Role};

pub(crate) type ConnId = u64;

/// The first message must arrive within this.
pub(crate) const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// The server pings this often when nothing else is sent.
const PING_EVERY: Duration = Duration::from_secs(15);
/// Silence (not even a pong) for this long ends the connection.
const DEAD_AFTER: u32 = 3; // × PING_EVERY = 45 s
/// After our close frame, how long the peer has to answer.
const CLOSE_LINGER: Duration = Duration::from_secs(2);
/// Bytes queued past which `Drained` is reported once they go below it.
pub(crate) const STATE_HIGH_WATER: usize = 64 * 1024;

/// What connection threads tell the party.
pub(crate) enum Inbound {
    /// A WebSocket is up (before its `hello`).
    Opened { conn: Conn },
    /// A message, stamped when it arrived.
    Message {
        conn: ConnId,
        at: Instant,
        msg: Incoming,
    },
    /// No message within [`HELLO_TIMEOUT`].
    HelloTimeout { conn: ConnId },
    /// The outbox went below [`STATE_HIGH_WATER`] again.
    Drained { conn: ConnId },
    /// The socket ended (either side, an error, silence).
    Closed { conn: ConnId },
    /// [`Waker::wake`](crate::Waker::wake): only ends a wait.
    Wake,
    /// From the fast path's driver.
    #[cfg(feature = "webrtc")]
    Fast(crate::fast::driver::Up),
}

pub(crate) enum Incoming {
    Text(String),
    Binary,
}

enum Out {
    Text(String),
    Close(u16, String),
}

struct OutState {
    queue: VecDeque<Out>,
    bytes: usize,
    /// The reader ended: nothing more is sent.
    done: bool,
    /// A close was queued: nothing more is queued.
    closing: bool,
    /// Above the high-water mark since the last `Drained`.
    high: bool,
}

struct Outbox {
    state: Mutex<OutState>,
    cv: Condvar,
}

impl Outbox {
    fn lock(&self) -> MutexGuard<'_, OutState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The party's handle of a connection.
#[derive(Clone)]
pub(crate) struct Conn {
    pub id: ConnId,
    /// The visitor's address, for limits and logs.
    pub address: String,
    out: Arc<Outbox>,
}

impl Conn {
    /// Queues a text message.
    pub fn send(&self, text: String) {
        let mut st = self.out.lock();
        if st.done || st.closing {
            return;
        }
        st.bytes += text.len();
        if st.bytes > STATE_HIGH_WATER {
            st.high = true;
        }
        st.queue.push_back(Out::Text(text));
        self.out.cv.notify_all();
    }

    /// Queues a close frame after what is queued.
    pub fn close(&self, code: u16, reason: &str) {
        let mut st = self.out.lock();
        if st.done || st.closing {
            return;
        }
        st.closing = true;
        st.queue.push_back(Out::Close(code, reason.to_owned()));
        self.out.cv.notify_all();
    }

    /// Bytes queued and not written yet.
    pub fn buffered(&self) -> usize {
        self.out.lock().bytes
    }

    /// Whether the socket ended or a close is on its way.
    pub fn closed(&self) -> bool {
        let st = self.out.lock();
        st.done || st.closing
    }
}

/// One of the server's WebSocket places: taken before the upgrade,
/// given back when dropped (also when the upgrade never happens).
pub(crate) struct Slot(Arc<AtomicUsize>);

impl Slot {
    pub fn take(count: &Arc<AtomicUsize>) -> Slot {
        count.fetch_add(1, Ordering::SeqCst);
        Slot(count.clone())
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Runs an upgraded connection: spawns the writer, reports `Opened`, and
/// reads on the calling thread until the socket ends.
pub(crate) fn run(
    tcp: TcpStream,
    leftover: Vec<u8>,
    id: ConnId,
    address: String,
    max_message: usize,
    inbox: mpsc::Sender<Inbound>,
    _slot: Slot,
) {
    let _ = tcp.set_nodelay(true);
    if tcp.set_read_timeout(Some(HELLO_TIMEOUT)).is_err() {
        return;
    }
    let Ok(shutdown) = tcp.try_clone() else {
        return;
    };
    let limits = websocket::Limits { max_message };
    let Ok((mut rx, tx)) = websocket::pair(tcp, leftover, Role::Server, limits) else {
        return;
    };
    let out = Arc::new(Outbox {
        state: Mutex::new(OutState {
            queue: VecDeque::new(),
            bytes: 0,
            done: false,
            closing: false,
            high: false,
        }),
        cv: Condvar::new(),
    });
    let conn = Conn {
        id,
        address,
        out: out.clone(),
    };
    let writer = {
        let (out, inbox, shutdown) = (out.clone(), inbox.clone(), shutdown.try_clone());
        std::thread::Builder::new()
            .name("party-ws-write".into())
            .spawn(move || write_loop(id, &out, &tx, &inbox, shutdown.ok()))
    };
    let Ok(writer) = writer else {
        return;
    };
    if inbox.send(Inbound::Opened { conn }).is_err() {
        let _ = shutdown.shutdown(Shutdown::Both);
    }
    let mut hello = false;
    let mut silent = 0;
    loop {
        match rx.read_message() {
            Ok(Message::Text(text)) => {
                silent = 0;
                if !hello {
                    hello = true;
                    let _ = rx.with_stream(|s| s.set_read_timeout(Some(PING_EVERY)));
                }
                let msg = Incoming::Text(text);
                if inbox
                    .send(Inbound::Message {
                        conn: id,
                        at: Instant::now(),
                        msg,
                    })
                    .is_err()
                {
                    break;
                }
            }
            Ok(Message::Binary(_)) => {
                silent = 0;
                if !hello {
                    hello = true;
                    let _ = rx.with_stream(|s| s.set_read_timeout(Some(PING_EVERY)));
                }
                let msg = Incoming::Binary;
                if inbox
                    .send(Inbound::Message {
                        conn: id,
                        at: Instant::now(),
                        msg,
                    })
                    .is_err()
                {
                    break;
                }
            }
            Ok(Message::Close(_)) => break,
            Err(e) if e.kind() == tvpipe::ErrorKind::Timeout => {
                if !hello {
                    // Once: the party sends `bye` and closes.
                    hello = true;
                    let _ = rx.with_stream(|s| s.set_read_timeout(Some(PING_EVERY)));
                    let _ = inbox.send(Inbound::HelloTimeout { conn: id });
                    continue;
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
        let mut st = out.lock();
        st.done = true;
        st.queue.clear();
        st.bytes = 0;
        out.cv.notify_all();
    }
    let _ = shutdown.shutdown(Shutdown::Both);
    let _ = writer.join();
    let _ = inbox.send(Inbound::Closed { conn: id });
}

fn write_loop(
    id: ConnId,
    out: &Outbox,
    tx: &websocket::Sender<TcpStream>,
    inbox: &mpsc::Sender<Inbound>,
    shutdown: Option<TcpStream>,
) {
    loop {
        let item = {
            let mut st = out.lock();
            let deadline = Instant::now() + PING_EVERY;
            while st.queue.is_empty() && !st.done {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                st = out
                    .cv
                    .wait_timeout(st, left)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            if st.done {
                return;
            }
            let item = st.queue.pop_front();
            if let Some(Out::Text(t)) = &item {
                st.bytes -= t.len();
            }
            if st.high && st.bytes <= STATE_HIGH_WATER {
                st.high = false;
                let _ = inbox.send(Inbound::Drained { conn: id });
            }
            item
        };
        match item {
            None => {
                if tx.ping(b"").is_err() {
                    return;
                }
            }
            Some(Out::Text(t)) => {
                if tx.send_text(&t).is_err() {
                    return;
                }
            }
            Some(Out::Close(code, reason)) => {
                let _ = tx.close(code, &reason);
                // The reader ends when the peer answers; do not wait forever.
                let st = out.lock();
                let (st, _) = out
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
        }
    }
}
