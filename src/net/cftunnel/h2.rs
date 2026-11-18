//! The server side of HTTP/2 (RFC 9113), as much as the tunnel needs: the
//! edge dials nothing, we dial it and then serve its streams.
//!
//! One thread reads frames ([`serve`]) and hands each new stream to a
//! handler, which runs it on a thread of its own with a [`Stream`]: its
//! request body arrives through a channel, its response goes out through
//! the shared writer. Flow control both ways: we advertise large windows
//! and return credit as the handler consumes data; sending waits for the
//! edge's credit. No server push, priorities are ignored, HPACK encoding
//! is literal ([`hpack`](super::hpack)).
//!
//! Sources: RFC 9113 (frames §4-6, flow control §5.2, the preface §3.4).

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::hpack::{self, Decoder, Header};
use crate::error::{Error, ErrorKind, Result};

const DATA: u8 = 0;
const HEADERS: u8 = 1;
const RST_STREAM: u8 = 3;
const SETTINGS: u8 = 4;
const PING: u8 = 6;
const GOAWAY: u8 = 7;
const WINDOW_UPDATE: u8 = 8;
const CONTINUATION: u8 = 9;

const END_STREAM: u8 = 1;
const ACK: u8 = 1;
const END_HEADERS: u8 = 4;
const PADDED: u8 = 8;
const PRIORITY: u8 = 0x20;

const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
/// SETTINGS_MAX_FRAME_SIZE: the default, which we also keep to when
/// sending (any peer must accept it).
const MAX_FRAME: usize = 16_384;
/// Our windows: per stream (SETTINGS_INITIAL_WINDOW_SIZE) and for the
/// connection.
const STREAM_WINDOW: u32 = 1 << 20;
const CONN_WINDOW: u32 = 16 << 20;
/// Streams at once (SETTINGS_MAX_CONCURRENT_STREAMS).
const MAX_STREAMS: u32 = 256;
/// Without a frame for this long we ping; for twice as long, give up.
const IDLE: Duration = Duration::from_secs(15);
/// A header block (with CONTINUATION frames) larger than this is refused.
const MAX_HEADER_BLOCK: usize = 64 * 1024;
/// How long a sender may wait for flow-control credit.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// A stream's request body as it arrives, and whether it ended.
#[derive(Default)]
struct Inbox {
    q: Mutex<(VecDeque<Vec<u8>>, bool)>,
    cv: Condvar,
}

impl Inbox {
    /// Adds data, or with `None` ends the body.
    fn push(&self, d: Option<Vec<u8>>) {
        let mut q = lock(&self.q);
        match d {
            Some(d) => q.0.push_back(d),
            None => q.1 = true,
        }
        self.cv.notify_all();
    }
}

struct Out {
    w: Box<dyn Write + Send>,
    window: i64,
    /// The open streams: our send window, the request body.
    streams: HashMap<u32, (i64, Arc<Inbox>)>,
    initial: i64,
    closed: bool,
}

/// One connection, shared by the reader and the streams.
pub struct Conn {
    out: Mutex<Out>,
    credit: Condvar,
}

fn frame(kind: u8, flags: u8, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(9 + payload.len());
    f.extend_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
    f.extend_from_slice(&[kind, flags]);
    f.extend_from_slice(&id.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Conn {
    /// A connection writing to `w`; sends our SETTINGS (the server's
    /// preface) and opens the connection window.
    pub fn new(w: Box<dyn Write + Send>) -> Result<Arc<Conn>> {
        let c = Arc::new(Conn {
            out: Mutex::new(Out {
                w,
                window: 65_535,
                streams: HashMap::new(),
                initial: 65_535,
                closed: false,
            }),
            credit: Condvar::new(),
        });
        let mut settings = Vec::new();
        for (id, v) in [(3u16, MAX_STREAMS), (4, STREAM_WINDOW)] {
            settings.extend_from_slice(&id.to_be_bytes());
            settings.extend_from_slice(&v.to_be_bytes());
        }
        let mut first = frame(SETTINGS, 0, 0, &settings);
        first.extend(frame(
            WINDOW_UPDATE,
            0,
            0,
            &(CONN_WINDOW - 65_535).to_be_bytes(),
        ));
        c.write(&first)?;
        Ok(c)
    }

    fn write_locked(out: &mut Out, bytes: &[u8]) -> Result<()> {
        if out.closed {
            return Err(Error::closed("tunnel: connection closed"));
        }
        out.w.write_all(bytes).map_err(|e| {
            out.closed = true;
            Error::io("tunnel: write", e)
        })
    }

    fn write(&self, bytes: &[u8]) -> Result<()> {
        Conn::write_locked(&mut lock(&self.out), bytes)
    }

    /// Marks the connection dead: senders fail, readers see the end.
    pub fn close(&self) {
        let mut out = lock(&self.out);
        out.closed = true;
        for (_, (_, i)) in out.streams.drain() {
            i.push(None);
        }
        self.credit.notify_all();
    }

    fn window_update(&self, id: u32, n: usize) -> Result<()> {
        let inc = (n as u32).to_be_bytes();
        let mut f = frame(WINDOW_UPDATE, 0, 0, &inc);
        if id != 0 {
            f.extend(frame(WINDOW_UPDATE, 0, id, &inc));
        }
        self.write(&f)
    }

    fn forget(&self, id: u32) {
        lock(&self.out).streams.remove(&id);
    }

    fn add_credit(&self, id: u32, n: i64) -> Result<()> {
        let mut out = lock(&self.out);
        let w = if id == 0 {
            &mut out.window
        } else {
            match out.streams.get_mut(&id) {
                Some(s) => &mut s.0,
                None => return Ok(()),
            }
        };
        *w += n;
        if *w > i64::from(i32::MAX) {
            return Err(Error::protocol("tunnel: flow-control window overflow"));
        }
        self.credit.notify_all();
        Ok(())
    }
}

/// One stream from the edge: the request's headers were decoded already.
pub struct Stream {
    pub id: u32,
    conn: Arc<Conn>,
    inbox: Arc<Inbox>,
    /// We ended our side (END_STREAM or RST_STREAM).
    done: AtomicBool,
}

impl Stream {
    /// Sends the response head (`:status` first).
    pub fn send_headers(&self, headers: &[(&str, &str)], end: bool) -> Result<()> {
        let mut block = Vec::new();
        for (n, v) in headers {
            hpack::encode(&mut block, n, v);
        }
        let mut out = lock(&self.conn.out);
        if block.len() > MAX_FRAME {
            return Err(Error::unsupported("tunnel: response headers too large"));
        }
        let flags = END_HEADERS | if end { END_STREAM } else { 0 };
        if end {
            self.done.store(true, Ordering::SeqCst);
        }
        Conn::write_locked(&mut out, &frame(HEADERS, flags, self.id, &block))
    }

    /// Sends body bytes as the edge's flow control allows.
    pub fn send_data(&self, mut data: &[u8], end: bool) -> Result<()> {
        let deadline = Instant::now() + SEND_TIMEOUT;
        let mut out = lock(&self.conn.out);
        loop {
            if out.closed {
                return Err(Error::closed("tunnel: connection closed"));
            }
            let Some(sw) = out.streams.get(&self.id).map(|s| s.0) else {
                return Err(Error::closed("tunnel: stream reset"));
            };
            let n = data.len().min(MAX_FRAME) as i64;
            let n = n.min(sw).min(out.window).max(0) as usize;
            if n == 0 && !data.is_empty() {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(Error::timeout("tunnel: no flow-control credit"));
                }
                out = self
                    .conn
                    .credit
                    .wait_timeout(out, left)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
                continue;
            }
            let last = n == data.len();
            let flags = if last && end { END_STREAM } else { 0 };
            if flags != 0 {
                self.done.store(true, Ordering::SeqCst);
            }
            Conn::write_locked(&mut out, &frame(DATA, flags, self.id, &data[..n]))?;
            out.window -= n as i64;
            if let Some(s) = out.streams.get_mut(&self.id) {
                s.0 -= n as i64;
            }
            data = &data[n..];
            if last {
                return Ok(());
            }
        }
    }

    /// The next piece of the request body; `None` at its end (or when the
    /// edge reset the stream or the connection ended).
    pub fn recv(&self) -> Option<Vec<u8>> {
        let mut q = lock(&self.inbox.q);
        loop {
            if let Some(d) = q.0.pop_front() {
                drop(q);
                let _ = self.conn.window_update(self.id, d.len());
                return Some(d);
            }
            if q.1 {
                return None;
            }
            q = self.inbox.cv.wait(q).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Ends our side abnormally (RST_STREAM CANCEL).
    pub fn reset(&self) {
        if !self.done.swap(true, Ordering::SeqCst) {
            let _ = self
                .conn
                .write(&frame(RST_STREAM, 0, self.id, &8u32.to_be_bytes()));
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.reset();
        self.conn.forget(self.id);
        // Body nobody read: its credit goes back to the connection.
        let unread: usize = lock(&self.inbox.q).0.iter().map(Vec::len).sum();
        if unread > 0 {
            let _ = self.conn.window_update(0, unread);
        }
    }
}

/// Reads whole frames, waking up to keep the connection alive.
struct Frames<R> {
    r: R,
    buf: Vec<u8>,
    last: Instant,
    pinged: bool,
}

impl<R: Read> Frames<R> {
    /// The next frame: type, flags, stream, payload.
    fn next(&mut self, conn: &Conn, stop: &AtomicBool) -> Result<(u8, u8, u32, Vec<u8>)> {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            if self.buf.len() >= 9 {
                let len = u32::from_be_bytes([0, self.buf[0], self.buf[1], self.buf[2]]) as usize;
                if len > MAX_FRAME {
                    return Err(Error::protocol("tunnel: frame too large"));
                }
                if self.buf.len() >= 9 + len {
                    let h: Vec<u8> = self.buf.drain(..9 + len).collect();
                    let id = u32::from_be_bytes([h[5], h[6], h[7], h[8]]) & 0x7fff_ffff;
                    return Ok((h[3], h[4], id, h[9..].to_vec()));
                }
            }
            if stop.load(Ordering::SeqCst) {
                return Err(Error::closed("tunnel: stopped"));
            }
            match self.r.read(&mut chunk) {
                Ok(0) => return Err(Error::closed("tunnel: the edge closed the connection")),
                Ok(n) => {
                    self.buf.extend_from_slice(&chunk[..n]);
                    self.last = Instant::now();
                    self.pinged = false;
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    let idle = self.last.elapsed();
                    if idle >= IDLE * 2 {
                        return Err(Error::timeout("tunnel: the edge went silent"));
                    }
                    if idle >= IDLE && !self.pinged {
                        self.pinged = true;
                        conn.write(&frame(PING, 0, 0, &[0; 8]))?;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(Error::io("tunnel: read", e)),
            }
        }
    }
}

/// Strips padding (and the priority fields of HEADERS).
fn unpad(flags: u8, kind: u8, mut p: Vec<u8>) -> Result<Vec<u8>> {
    let bad = || Error::protocol("tunnel: bad padding");
    if flags & PADDED != 0 {
        let pad = usize::from(*p.first().ok_or_else(bad)?);
        if pad + 1 > p.len() {
            return Err(bad());
        }
        p.truncate(p.len() - pad);
        p.remove(0);
    }
    if kind == HEADERS && flags & PRIORITY != 0 {
        if p.len() < 5 {
            return Err(bad());
        }
        p.drain(..5);
    }
    Ok(p)
}

/// Serves the connection until it ends: reads the client preface, then
/// frames; `handler` gets each new request stream and its headers (it
/// must not block: it runs on the reader).
pub fn serve(
    conn: &Arc<Conn>,
    r: impl Read,
    stop: &AtomicBool,
    mut handler: impl FnMut(Arc<Stream>, Vec<Header>),
) -> Result<()> {
    let mut frames = Frames {
        r,
        buf: Vec::new(),
        last: Instant::now(),
        pinged: false,
    };
    let result = serve_frames(conn, &mut frames, stop, &mut handler);
    if let Err(e) = &result {
        if e.kind() == ErrorKind::Protocol {
            let mut goaway = vec![0; 4];
            goaway.extend_from_slice(&1u32.to_be_bytes()); // PROTOCOL_ERROR
            let _ = conn.write(&frame(GOAWAY, 0, 0, &goaway));
        }
    }
    conn.close();
    result
}

fn serve_frames<R: Read>(
    conn: &Arc<Conn>,
    frames: &mut Frames<R>,
    stop: &AtomicBool,
    handler: &mut impl FnMut(Arc<Stream>, Vec<Header>),
) -> Result<()> {
    // The client preface.
    while frames.buf.len() < PREFACE.len() {
        let mut b = [0u8; 24];
        let n = frames
            .r
            .read(&mut b)
            .map_err(|e| Error::io("tunnel: read", e))?;
        if n == 0 {
            return Err(Error::closed("tunnel: closed before the preface"));
        }
        frames.buf.extend_from_slice(&b[..n]);
    }
    if !frames.buf.starts_with(PREFACE) {
        return Err(Error::protocol("tunnel: no HTTP/2 preface"));
    }
    frames.buf.drain(..PREFACE.len());
    let mut hpack = Decoder::new();
    let mut last_id = 0;
    loop {
        let (kind, flags, id, payload) = frames.next(conn, stop)?;
        match kind {
            DATA => {
                let n = payload.len();
                let data = unpad(flags, kind, payload)?;
                let inbox = lock(&conn.out).streams.get(&id).map(|s| s.1.clone());
                // Padding, and data for streams that are gone, are
                // credited right away; the rest when the stream reads it.
                let early = n - if inbox.is_some() { data.len() } else { 0 };
                if early > 0 {
                    conn.window_update(0, early)?;
                }
                if let Some(inbox) = inbox {
                    if !data.is_empty() {
                        inbox.push(Some(data));
                    }
                    if flags & END_STREAM != 0 {
                        inbox.push(None);
                    }
                }
            }
            HEADERS => {
                let mut block = unpad(flags, kind, payload)?;
                let mut end_headers = flags & END_HEADERS != 0;
                while !end_headers {
                    let (k, f, i, p) = frames.next(conn, stop)?;
                    if k != CONTINUATION || i != id {
                        return Err(Error::protocol("tunnel: expected CONTINUATION"));
                    }
                    block.extend_from_slice(&p);
                    if block.len() > MAX_HEADER_BLOCK {
                        return Err(Error::protocol("tunnel: header block too large"));
                    }
                    end_headers = f & END_HEADERS != 0;
                }
                let headers = hpack.decode(&block)?;
                if id <= last_id {
                    // Trailers (ignored), but they may end the stream.
                    if flags & END_STREAM != 0 {
                        if let Some(s) = lock(&conn.out).streams.get(&id) {
                            s.1.push(None);
                        }
                    }
                    continue;
                }
                if id % 2 == 0 {
                    return Err(Error::protocol("tunnel: even stream from the edge"));
                }
                last_id = id;
                let inbox = Arc::new(Inbox::default());
                if flags & END_STREAM != 0 {
                    inbox.push(None);
                }
                {
                    let mut out = lock(&conn.out);
                    let initial = out.initial;
                    out.streams.insert(id, (initial, inbox.clone()));
                }
                let stream = Arc::new(Stream {
                    id,
                    conn: conn.clone(),
                    inbox,
                    done: AtomicBool::new(false),
                });
                handler(stream, headers);
            }
            RST_STREAM => {
                if let Some((_, inbox)) = lock(&conn.out).streams.remove(&id) {
                    inbox.push(None);
                }
                conn.credit.notify_all();
            }
            SETTINGS if flags & ACK == 0 => {
                if payload.len() % 6 != 0 {
                    return Err(Error::protocol("tunnel: bad SETTINGS"));
                }
                let mut out = lock(&conn.out);
                for s in payload.chunks_exact(6) {
                    let v = u32::from_be_bytes([s[2], s[3], s[4], s[5]]);
                    match u16::from_be_bytes([s[0], s[1]]) {
                        4 if v <= i32::MAX as u32 => {
                            let delta = i64::from(v) - out.initial;
                            out.initial = i64::from(v);
                            out.streams.values_mut().for_each(|s| s.0 += delta);
                        }
                        4 => return Err(Error::protocol("tunnel: bad SETTINGS value")),
                        _ => {}
                    }
                }
                Conn::write_locked(&mut out, &frame(SETTINGS, ACK, 0, &[]))?;
                drop(out);
                conn.credit.notify_all();
            }
            PING if flags & ACK == 0 => {
                conn.write(&frame(PING, ACK, 0, &payload))?;
            }
            GOAWAY => {
                return Err(Error::closed("tunnel: the edge is going away (GOAWAY)"));
            }
            WINDOW_UPDATE => {
                let b: [u8; 4] = payload
                    .get(..4)
                    .and_then(|b| b.try_into().ok())
                    .ok_or_else(|| Error::protocol("tunnel: bad WINDOW_UPDATE"))?;
                let n = u32::from_be_bytes(b) & 0x7fff_ffff;
                conn.add_credit(id, i64::from(n))?;
            }
            _ => {} // PRIORITY, acks, unknown frame types
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The edge's side of a connection for tests: writes frames, reads
    /// and decodes ours.
    pub struct Edge<S> {
        pub s: S,
        buf: Vec<u8>,
        pub hpack: Decoder,
    }

    impl<S: Read + Write> Edge<S> {
        pub fn new(mut s: S) -> Edge<S> {
            s.write_all(PREFACE).unwrap();
            s.write_all(&frame(SETTINGS, 0, 0, &[])).unwrap();
            Edge {
                s,
                buf: Vec::new(),
                hpack: Decoder::new(),
            }
        }

        pub fn send(&mut self, kind: u8, flags: u8, id: u32, payload: &[u8]) {
            self.s.write_all(&frame(kind, flags, id, payload)).unwrap();
        }

        pub fn request(&mut self, id: u32, headers: &[(&str, &str)], end: bool) {
            let mut block = Vec::new();
            for (n, v) in headers {
                hpack::encode(&mut block, n, v);
            }
            let flags = END_HEADERS | if end { END_STREAM } else { 0 };
            self.send(HEADERS, flags, id, &block);
        }

        pub fn data(&mut self, id: u32, data: &[u8], end: bool) {
            self.send(DATA, if end { END_STREAM } else { 0 }, id, data);
        }

        /// The next frame that is not SETTINGS/WINDOW_UPDATE/PING.
        pub fn frame(&mut self) -> (u8, u8, u32, Vec<u8>) {
            loop {
                while self.buf.len() < 9
                    || self.buf.len()
                        < 9 + u32::from_be_bytes([0, self.buf[0], self.buf[1], self.buf[2]])
                            as usize
                {
                    let mut b = [0u8; 4096];
                    let n = self.s.read(&mut b).unwrap();
                    assert!(n > 0, "connection closed");
                    self.buf.extend_from_slice(&b[..n]);
                }
                let len = u32::from_be_bytes([0, self.buf[0], self.buf[1], self.buf[2]]) as usize;
                let h: Vec<u8> = self.buf.drain(..9 + len).collect();
                let id = u32::from_be_bytes([h[5], h[6], h[7], h[8]]);
                if ![SETTINGS, WINDOW_UPDATE, PING].contains(&h[3]) {
                    return (h[3], h[4], id, h[9..].to_vec());
                }
            }
        }

        /// The response on a stream: headers, the body and whether it ended.
        pub fn response(&mut self, id: u32) -> (Vec<Header>, Vec<u8>) {
            let (k, f, i, p) = self.frame();
            assert_eq!((k, i), (HEADERS, id));
            let headers = self.hpack.decode(&p).unwrap();
            let mut body = Vec::new();
            let mut end = f & END_STREAM != 0;
            while !end {
                let (k, f, i, p) = self.frame();
                assert_eq!((k, i), (DATA, id));
                body.extend_from_slice(&p);
                end = f & END_STREAM != 0;
            }
            (headers, body)
        }

        /// Body bytes on a stream until `n` arrived.
        pub fn read_data(&mut self, id: u32, n: usize) -> Vec<u8> {
            let mut body = Vec::new();
            while body.len() < n {
                let (k, _, i, p) = self.frame();
                assert_eq!((k, i), (DATA, id));
                body.extend_from_slice(&p);
            }
            body
        }
    }

    #[test]
    fn frames_and_flow_control() {
        let (a, b) = crate::net::cftunnel::tests::socket_pair();
        let conn = Conn::new(Box::new(a.try_clone().unwrap())).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let c = conn.clone();
        let st = stop.clone();
        let reader = std::thread::spawn(move || {
            serve(&c, a, &st, |s, headers| {
                std::thread::spawn(move || {
                    let path = headers.iter().find(|h| h.0 == ":path").unwrap().1.clone();
                    let mut body = Vec::new();
                    while let Some(d) = s.recv() {
                        body.extend(d);
                    }
                    s.send_headers(&[(":status", "200"), ("x-path", &path)], false)
                        .unwrap();
                    // 100 KB: more than the default 64 KiB window.
                    let big = vec![7u8; 100_000];
                    s.send_data(&body, false).unwrap();
                    s.send_data(&big, true).unwrap();
                });
            })
        });
        let mut edge = Edge::new(b);
        // A padded DATA frame, a request in two header frames.
        let mut block = Vec::new();
        hpack::encode(&mut block, ":method", "POST");
        hpack::encode(&mut block, ":path", "/x");
        edge.send(HEADERS, 0, 1, &block[..5]);
        edge.send(CONTINUATION, END_HEADERS, 1, &block[5..]);
        edge.send(DATA, PADDED, 1, &[2, b'h', b'i', 0, 0]);
        edge.data(1, b"!", true);
        let (k, _, _, p) = edge.frame();
        assert_eq!(k, HEADERS);
        let h = edge.hpack.decode(&p).unwrap();
        assert_eq!(h[1], ("x-path".into(), "/x".into()));
        assert_eq!(edge.read_data(1, 3), b"hi!");
        // 65535 - 3 bytes fit the window; the rest waits for credit.
        let got = edge.read_data(1, 65_532);
        assert_eq!(got.len(), 65_532);
        edge.send(WINDOW_UPDATE, 0, 0, &100_000u32.to_be_bytes());
        edge.send(WINDOW_UPDATE, 0, 1, &100_000u32.to_be_bytes());
        let rest = edge.read_data(1, 100_000 - 65_532);
        assert_eq!(rest.len(), 100_000 - 65_532);
        // PING is answered; GOAWAY ends the connection.
        edge.send(PING, 0, 0, &[1; 8]);
        edge.send(GOAWAY, 0, 0, &[0; 8]);
        let r = reader.join().unwrap();
        assert_eq!(r.unwrap_err().kind(), ErrorKind::Closed);
    }

    #[test]
    fn bad_input_is_an_error() {
        for bytes in [
            b"GET / HTTP/1.1\r\n\r\n\r\n\r\n\r\n\r\n".to_vec(),
            [PREFACE, &frame(SETTINGS, 0, 0, &[1, 2, 3])].concat(),
            [PREFACE, &frame(HEADERS, END_HEADERS, 2, &[0x80])].concat(),
            [PREFACE, &frame(DATA, PADDED, 1, &[9])].concat(),
            [PREFACE, &[0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0][..]].concat(),
        ] {
            let conn = Conn::new(Box::new(std::io::sink())).unwrap();
            let r = serve(&conn, &bytes[..], &AtomicBool::new(false), |_, _| {});
            assert!(r.is_err());
        }
    }
}
