//! WebSocket (RFC 6455), server and client, over any blocking stream.
//!
//! - Handshake: [`server_response`] validates an upgrade request read with
//!   `net::http` and builds the `101` response; [`connect`] /
//!   [`client_handshake`] open a client connection (`ws://` only).
//! - Framing: a [`Receiver`] reads whole messages (reassembling fragments,
//!   unmasking, answering pings and echoing a close), a [`Sender`] writes
//!   them (masking client frames with a fresh random key each). The sender
//!   is a cloneable handle around a mutex, so one thread can block in
//!   `read_message` while others send; the receiver answers pings through
//!   it.
//! - Limits: a message (all its fragments) larger than
//!   [`Limits::max_message`] is refused with close code 1009; control
//!   frames are limited to 125 bytes and must not be fragmented; reserved
//!   bits and opcodes, a wrongly (un)masked frame, a continuation without
//!   a start, invalid UTF-8 in text (1007) and invalid close codes are
//!   protocol errors (1002). Every violation closes the connection with
//!   the right code and returns an error; nothing panics on peer data.
//! - Timeouts: a read timeout on the stream surfaces as an error of kind
//!   [`ErrorKind::Timeout`]; the receiver keeps partial frames buffered,
//!   so calling `read_message` again continues where it stopped.
//!
//! No extensions (`permessage-deflate`) and no subprotocols are
//! negotiated.
//!
//! Sources: RFC 6455.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::crypto::ring as crypto;
use crate::error::{Error, ErrorKind, Result};
use crate::net::base64;
use crate::net::http::{self, Conn, Request, Response, Version};

/// RFC 6455 §1.3.
pub const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Close codes (RFC 6455 §7.4.1).
pub mod close_code {
    pub const NORMAL: u16 = 1000;
    pub const GOING_AWAY: u16 = 1001;
    pub const PROTOCOL: u16 = 1002;
    pub const UNSUPPORTED: u16 = 1003;
    pub const INVALID_DATA: u16 = 1007;
    pub const POLICY: u16 = 1008;
    pub const TOO_BIG: u16 = 1009;
    pub const INTERNAL: u16 = 1011;
}

const OP_CONT: u8 = 0x0;
const OP_TEXT: u8 = 0x1;
const OP_BINARY: u8 = 0x2;
const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xA;

const MAX_CONTROL: usize = 125;
const READ_CHUNK: usize = 64 * 1024;

/// Which end of the connection we are: clients mask what they send and
/// expect unmasked frames, servers the opposite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Server,
    Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Largest message (sum of its fragments).
    pub max_message: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_message: 16 << 20,
        }
    }
}

/// A received message. Pings and pongs are handled inside the receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    /// The peer closed (code and reason, if it sent them). The close was
    /// echoed; the connection is done.
    Close(Option<(u16, String)>),
}

/// `Sec-WebSocket-Accept` for a `Sec-WebSocket-Key`.
pub fn accept_key(key: &str) -> String {
    let mut h = crypto::Sha1::new();
    h.update(key.as_bytes()).update(GUID.as_bytes());
    base64::encode(&h.finish())
}

/// Whether a comma-separated header (every occurrence) lists `token`.
fn has_token<'a>(mut values: impl Iterator<Item = &'a str>, token: &str) -> bool {
    values.any(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
}

/// Whether `req` asks for a WebSocket upgrade (it may still be invalid).
pub fn is_upgrade(req: &Request) -> bool {
    has_token(req.headers.get_all("upgrade"), "websocket")
}

/// Validates a client's opening handshake (§4.2.1) and returns the `101
/// Switching Protocols` response to send. An `Err` says why; the caller
/// answers `400` (or `426` when [`ErrorKind::Unsupported`]: the version).
pub fn server_response(req: &Request) -> Result<Response> {
    if req.method != "GET" || req.version != Version::Http11 {
        return Err(Error::protocol(
            "websocket: upgrade needs GET over HTTP/1.1",
        ));
    }
    if !req.headers.contains("host") {
        return Err(Error::protocol("websocket: no Host header"));
    }
    if !is_upgrade(req) || !has_token(req.headers.get_all("connection"), "upgrade") {
        return Err(Error::protocol("websocket: not an upgrade request"));
    }
    if req.headers.get("sec-websocket-version").map(str::trim) != Some("13") {
        return Err(Error::unsupported("websocket: version 13 required"));
    }
    let key = req
        .headers
        .get("sec-websocket-key")
        .map(str::trim)
        .ok_or_else(|| Error::protocol("websocket: no Sec-WebSocket-Key"))?;
    if base64::decode(key).map(|k| k.len()).ok() != Some(16) {
        return Err(Error::protocol(
            "websocket: Sec-WebSocket-Key is not 16 bytes",
        ));
    }
    let mut resp = Response::new(Version::Http11, 101, "Switching Protocols");
    resp.headers
        .add("Upgrade", "websocket")
        .add("Connection", "Upgrade")
        .add("Sec-WebSocket-Accept", accept_key(key));
    Ok(resp)
}

/// A `ws://` URL: host (as written, for the `Host` header), socket
/// address string and request target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsUrl {
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl WsUrl {
    pub fn parse(url: &str) -> Result<WsUrl> {
        let rest = url
            .strip_prefix("ws://")
            .ok_or_else(|| Error::unsupported(format!("websocket: only ws:// URLs: {url:?}")))?;
        let (authority, path) = match rest.find(['/', '?']) {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let path = if path.starts_with('?') {
            format!("/{path}")
        } else {
            path.to_owned()
        };
        let path = path.split('#').next().unwrap_or("/").to_owned();
        let bad = || Error::malformed(format!("websocket: bad URL {url:?}"));
        if authority.contains('@') || authority.is_empty() {
            return Err(bad());
        }
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (h, after) = v6.split_once(']').ok_or_else(bad)?;
            let port = match after.strip_prefix(':') {
                Some(p) => p.parse().map_err(|_| bad())?,
                None if after.is_empty() => 80,
                None => return Err(bad()),
            };
            (format!("[{h}]"), port)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h.to_owned(), p.parse().map_err(|_| bad())?),
                None => (authority.to_owned(), 80),
            }
        };
        if host.is_empty() || path.bytes().any(|b| b <= b' ' || b == 0x7f) {
            return Err(bad());
        }
        Ok(WsUrl { host, port, path })
    }

    /// The `Host` header value.
    pub fn authority(&self) -> String {
        if self.port == 80 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// Runs the client's opening handshake on `stream` (§4.1). Returns the
/// stream and any bytes the server sent after its response (the first
/// frames), which belong to the [`Receiver`].
pub fn client_handshake<S: Read + Write>(
    stream: S,
    url: &WsUrl,
    extra_headers: &[(&str, &str)],
) -> Result<(S, Vec<u8>)> {
    let nonce: [u8; 16] = crypto::random_array()?;
    let key = base64::encode(&nonce);
    let mut req = Request::new("GET", &url.path, Version::Http11)
        .header("Host", url.authority())
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Key", key.clone())
        .header("Sec-WebSocket-Version", "13");
    for (k, v) in extra_headers {
        req.headers.add(*k, *v);
    }
    let mut conn = Conn::with_limits(
        stream,
        http::Limits {
            max_body: 64 * 1024,
            ..http::Limits::default()
        },
    );
    conn.send_request(&req)?;
    let resp = conn.read_response(false)?;
    if resp.status != 101 {
        return Err(Error::protocol(format!(
            "websocket: handshake refused: {} {}",
            resp.status, resp.reason
        )));
    }
    let h = &resp.headers;
    if !has_token(h.get_all("upgrade"), "websocket")
        || !has_token(h.get_all("connection"), "upgrade")
    {
        return Err(Error::protocol("websocket: response is not an upgrade"));
    }
    if h.get("sec-websocket-accept").map(str::trim) != Some(accept_key(&key).as_str()) {
        return Err(Error::protocol("websocket: wrong Sec-WebSocket-Accept"));
    }
    if h.contains("sec-websocket-extensions") || h.contains("sec-websocket-protocol") {
        return Err(Error::protocol(
            "websocket: server selected an extension or subprotocol we did not offer",
        ));
    }
    Ok(conn.into_parts())
}

/// Connects to a `ws://` URL with a connect timeout; the returned pair
/// shares one TCP connection.
pub fn connect(
    url: &str,
    timeout: Duration,
    limits: Limits,
) -> Result<(Receiver<TcpStream, TcpStream>, Sender<TcpStream>)> {
    let u = WsUrl::parse(url)?;
    let host = u.host.trim_start_matches('[').trim_end_matches(']');
    let addrs = (host, u.port)
        .to_socket_addrs()
        .map_err(|e| Error::io(format!("websocket: resolve {}", u.host), e))?;
    let mut last = None;
    let mut tcp = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, timeout) {
            Ok(s) => {
                tcp = Some(s);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let tcp = match (tcp, last) {
        (Some(t), _) => t,
        (None, Some(e)) => return Err(Error::io(format!("websocket: connect to {url}"), e)),
        (None, None) => return Err(Error::not_found(format!("websocket: no address for {url}"))),
    };
    let _ = tcp.set_nodelay(true);
    tcp.set_read_timeout(Some(timeout))
        .and_then(|()| tcp.set_write_timeout(Some(timeout)))
        .map_err(|e| Error::io("websocket: set timeout", e))?;
    let (tcp, rest) = client_handshake(tcp, &u, &[])?;
    let _ = tcp.set_read_timeout(None);
    pair(tcp, rest, Role::Client, limits)
}

/// A receiver and sender over one TCP stream (after the handshake;
/// `leftover` are bytes already read past it).
pub fn pair(
    tcp: TcpStream,
    leftover: Vec<u8>,
    role: Role,
    limits: Limits,
) -> Result<(Receiver<TcpStream, TcpStream>, Sender<TcpStream>)> {
    let w = tcp
        .try_clone()
        .map_err(|e| Error::io("websocket: clone stream", e))?;
    let sender = Sender::new(w, role);
    Ok((
        Receiver::new(tcp, leftover, role, limits, sender.clone()),
        sender,
    ))
}

/// One serialized frame (§5.2). `mask` is required for client frames.
pub fn encode_frame(fin: bool, opcode: u8, payload: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(if fin { 0x80 } else { 0 } | (opcode & 0x0f));
    let m = if mask.is_some() { 0x80 } else { 0 };
    let n = payload.len();
    if n < 126 {
        out.push(m | n as u8);
    } else if n <= 0xffff {
        out.push(m | 126);
        out.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        out.push(m | 127);
        out.extend_from_slice(&(n as u64).to_be_bytes());
    }
    match mask {
        Some(k) => {
            out.extend_from_slice(&k);
            out.extend(payload.iter().enumerate().map(|(i, b)| b ^ k[i & 3]));
        }
        None => out.extend_from_slice(payload),
    }
    out
}

struct SendState<W> {
    w: W,
    role: Role,
    /// A close frame went out; nothing may follow it.
    closed: bool,
    failed: bool,
}

/// The sending half: a cheap, cloneable handle.
pub struct Sender<W> {
    inner: Arc<Mutex<SendState<W>>>,
}

impl<W> Clone for Sender<W> {
    fn clone(&self) -> Sender<W> {
        Sender {
            inner: self.inner.clone(),
        }
    }
}

impl<W: Write> Sender<W> {
    pub fn new(w: W, role: Role) -> Sender<W> {
        Sender {
            inner: Arc::new(Mutex::new(SendState {
                w,
                role,
                closed: false,
                failed: false,
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SendState<W>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, opcode: u8, payload: &[u8]) -> Result<()> {
        let mut st = self.lock();
        if st.closed || st.failed {
            return Err(Error::closed("websocket: connection closed"));
        }
        let mask = match st.role {
            Role::Client => Some(crypto::random_array::<4>()?),
            Role::Server => None,
        };
        let frame = encode_frame(true, opcode, payload, mask);
        if opcode == OP_CLOSE {
            st.closed = true;
        }
        let r = st.w.write_all(&frame).and_then(|()| st.w.flush());
        if let Err(e) = r {
            st.failed = true;
            return Err(Error::io("websocket: write", e));
        }
        Ok(())
    }

    pub fn send_text(&self, text: &str) -> Result<()> {
        self.send(OP_TEXT, text.as_bytes())
    }

    pub fn send_binary(&self, data: &[u8]) -> Result<()> {
        self.send(OP_BINARY, data)
    }

    pub fn ping(&self, data: &[u8]) -> Result<()> {
        if data.len() > MAX_CONTROL {
            return Err(Error::usage("websocket: ping payload over 125 bytes"));
        }
        self.send(OP_PING, data)
    }

    /// Sends a close frame (once; later calls do nothing). The reason is
    /// cut to fit a control frame.
    pub fn close(&self, code: u16, reason: &str) -> Result<()> {
        if self.lock().closed {
            return Ok(());
        }
        let mut payload = code.to_be_bytes().to_vec();
        let mut cut = reason.len().min(MAX_CONTROL - 2);
        while !reason.is_char_boundary(cut) {
            cut -= 1;
        }
        payload.extend_from_slice(&reason.as_bytes()[..cut]);
        self.send(OP_CLOSE, &payload)
    }

    /// Whether we sent a close or a write failed.
    pub fn is_closed(&self) -> bool {
        let st = self.lock();
        st.closed || st.failed
    }

    /// Runs `f` on the underlying stream (e.g. `TcpStream::shutdown`).
    pub fn with_stream<T>(&self, f: impl FnOnce(&mut W) -> T) -> T {
        f(&mut self.lock().w)
    }
}

struct Frame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

/// The receiving half.
pub struct Receiver<R, W> {
    r: R,
    buf: Vec<u8>,
    pos: usize,
    role: Role,
    limits: Limits,
    sender: Sender<W>,
    /// A fragmented message in progress: its opcode and data so far.
    partial: Option<(u8, Vec<u8>)>,
    done: bool,
}

/// An error that closes the connection with `code`.
struct Violation {
    code: u16,
    err: Error,
}

fn violation(code: u16, msg: impl Into<String>) -> Violation {
    let msg = msg.into();
    Violation {
        code,
        err: if code == close_code::TOO_BIG || code == close_code::INVALID_DATA {
            Error::malformed(format!("websocket: {msg}"))
        } else {
            Error::protocol(format!("websocket: {msg}"))
        },
    }
}

fn valid_close_code(c: u16) -> bool {
    matches!(c, 1000..=1003 | 1007..=1014 | 3000..=4999)
}

impl<R: Read, W: Write> Receiver<R, W> {
    pub fn new(
        r: R,
        leftover: Vec<u8>,
        role: Role,
        limits: Limits,
        sender: Sender<W>,
    ) -> Receiver<R, W> {
        Receiver {
            r,
            buf: leftover,
            pos: 0,
            role,
            limits,
            sender,
            partial: None,
            done: false,
        }
    }

    pub fn sender(&self) -> &Sender<W> {
        &self.sender
    }

    /// Reads the next message. Errors of kind [`ErrorKind::Timeout`] (a
    /// read timeout) leave the state intact; any other error, and a
    /// returned `Close`, end the connection.
    pub fn read_message(&mut self) -> Result<Message> {
        if self.done {
            return Err(Error::closed("websocket: connection closed"));
        }
        match self.next_message() {
            Ok(m) => {
                if matches!(m, Message::Close(_)) {
                    self.done = true;
                }
                Ok(m)
            }
            Err(v) => {
                if v.err.kind() != ErrorKind::Timeout {
                    self.done = true;
                    if v.code != 0 {
                        let _ = self.sender.close(v.code, v.err.message());
                    }
                }
                Err(v.err)
            }
        }
    }

    fn next_message(&mut self) -> std::result::Result<Message, Violation> {
        loop {
            let f = self.next_frame()?;
            match f.opcode {
                OP_PING => {
                    // A failed pong surfaces on the next send.
                    if !self.sender.is_closed() {
                        let _ = self.sender.send(OP_PONG, &f.payload);
                    }
                }
                OP_PONG => {}
                OP_CLOSE => return self.closing(&f.payload),
                OP_CONT => {
                    let Some((op, mut data)) = self.partial.take() else {
                        return Err(violation(
                            close_code::PROTOCOL,
                            "continuation frame without a message",
                        ));
                    };
                    if data.len() + f.payload.len() > self.limits.max_message {
                        return Err(violation(close_code::TOO_BIG, "message too big"));
                    }
                    data.extend_from_slice(&f.payload);
                    if f.fin {
                        return finish(op, data);
                    }
                    self.partial = Some((op, data));
                }
                op => {
                    if self.partial.is_some() {
                        return Err(violation(
                            close_code::PROTOCOL,
                            "new message inside a fragmented one",
                        ));
                    }
                    if f.fin {
                        return finish(op, f.payload);
                    }
                    self.partial = Some((op, f.payload));
                }
            }
        }
    }

    fn closing(&mut self, payload: &[u8]) -> std::result::Result<Message, Violation> {
        let close = match payload.len() {
            0 => None,
            1 => return Err(violation(close_code::PROTOCOL, "close frame of 1 byte")),
            _ => {
                let code = u16::from_be_bytes([payload[0], payload[1]]);
                if !valid_close_code(code) {
                    return Err(violation(
                        close_code::PROTOCOL,
                        format!("invalid close code {code}"),
                    ));
                }
                let reason = std::str::from_utf8(&payload[2..]).map_err(|_| {
                    violation(close_code::INVALID_DATA, "close reason is not UTF-8")
                })?;
                Some((code, reason.to_owned()))
            }
        };
        // Echo (§5.5.1); after our own close this does nothing.
        let code = close.as_ref().map_or(close_code::NORMAL, |c| c.0);
        let _ = self.sender.close(code, "");
        Ok(Message::Close(close))
    }

    /// Bytes buffered and not consumed.
    fn avail(&self) -> &[u8] {
        &self.buf[self.pos..]
    }

    /// Makes at least `n` bytes available.
    fn fill(&mut self, n: usize) -> std::result::Result<(), Violation> {
        while self.avail().len() < n {
            if self.pos > 0 && (self.pos >= self.buf.len() / 2 || self.buf.len() >= READ_CHUNK) {
                self.buf.drain(..self.pos);
                self.pos = 0;
            }
            let have = self.buf.len();
            let want = (n - self.avail().len()).max(READ_CHUNK);
            self.buf.resize(have + want, 0);
            let r = self.r.read(&mut self.buf[have..]);
            match r {
                Ok(0) => {
                    self.buf.truncate(have);
                    return Err(Violation {
                        code: 0,
                        err: Error::closed("websocket: connection closed without a close frame"),
                    });
                }
                Ok(k) => self.buf.truncate(have + k),
                Err(e) => {
                    self.buf.truncate(have);
                    if e.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(Violation {
                        code: 0,
                        err: Error::io("websocket: read", e),
                    });
                }
            }
        }
        Ok(())
    }

    /// Reads one frame, unmasked, without consuming anything until the
    /// whole frame is buffered (so a timeout loses nothing).
    fn next_frame(&mut self) -> std::result::Result<Frame, Violation> {
        self.fill(2)?;
        let (b0, b1) = (self.avail()[0], self.avail()[1]);
        let fin = b0 & 0x80 != 0;
        let opcode = b0 & 0x0f;
        if b0 & 0x70 != 0 {
            return Err(violation(close_code::PROTOCOL, "reserved bits set"));
        }
        if !matches!(
            opcode,
            OP_CONT | OP_TEXT | OP_BINARY | OP_CLOSE | OP_PING | OP_PONG
        ) {
            return Err(violation(
                close_code::PROTOCOL,
                format!("reserved opcode {opcode:#x}"),
            ));
        }
        let masked = b1 & 0x80 != 0;
        if masked != (self.role == Role::Server) {
            return Err(violation(
                close_code::PROTOCOL,
                if masked {
                    "masked frame from the server"
                } else {
                    "unmasked frame from the client"
                },
            ));
        }
        let control = opcode & 0x08 != 0;
        let (len, head) = match b1 & 0x7f {
            126 => {
                self.fill(4)?;
                let a = self.avail();
                (u16::from_be_bytes([a[2], a[3]]) as u64, 4)
            }
            127 => {
                self.fill(10)?;
                let mut l = [0u8; 8];
                l.copy_from_slice(&self.avail()[2..10]);
                let l = u64::from_be_bytes(l);
                if l >> 63 != 0 {
                    return Err(violation(close_code::PROTOCOL, "frame length overflow"));
                }
                (l, 10)
            }
            n => (n as u64, 2),
        };
        if control && (!fin || len > MAX_CONTROL as u64) {
            return Err(violation(
                close_code::PROTOCOL,
                "fragmented or oversized control frame",
            ));
        }
        if len > self.limits.max_message as u64 {
            return Err(violation(close_code::TOO_BIG, "message too big"));
        }
        let len = len as usize;
        let head = head + if masked { 4 } else { 0 };
        self.fill(head + len)?;
        let a = self.avail();
        let mut payload = a[head..head + len].to_vec();
        if masked {
            let k = [a[head - 4], a[head - 3], a[head - 2], a[head - 1]];
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= k[i & 3];
            }
        }
        self.pos += head + len;
        if self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        }
        Ok(Frame {
            fin,
            opcode,
            payload,
        })
    }
}

fn finish(op: u8, data: Vec<u8>) -> std::result::Result<Message, Violation> {
    if op == OP_TEXT {
        String::from_utf8(data)
            .map(Message::Text)
            .map_err(|_| violation(close_code::INVALID_DATA, "text message is not UTF-8"))
    } else {
        Ok(Message::Binary(data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    type Out = Arc<Mutex<Vec<u8>>>;

    #[derive(Clone, Default)]
    struct Shared(Out);

    impl Write for Shared {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn receiver(role: Role, input: Vec<u8>) -> (Receiver<Cursor<Vec<u8>>, Shared>, Out) {
        let out = Shared::default();
        let o = out.0.clone();
        let sender = Sender::new(out, role);
        (
            Receiver::new(
                Cursor::new(input),
                Vec::new(),
                role,
                Limits::default(),
                sender,
            ),
            o,
        )
    }

    const MASKED_HELLO: [u8; 11] = [
        0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
    ];

    #[test]
    fn rfc6455_accept_key() {
        // §1.3.
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn rfc6455_section_5_7_examples() {
        // Unmasked "Hello" (server to client).
        let hello = [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
        assert_eq!(encode_frame(true, OP_TEXT, b"Hello", None), hello);
        let (mut r, _) = receiver(Role::Client, hello.to_vec());
        assert_eq!(r.read_message().unwrap(), Message::Text("Hello".into()));
        // Masked "Hello" (client to server).
        assert_eq!(
            encode_frame(true, OP_TEXT, b"Hello", Some([0x37, 0xfa, 0x21, 0x3d])),
            MASKED_HELLO
        );
        let (mut r, _) = receiver(Role::Server, MASKED_HELLO.to_vec());
        assert_eq!(r.read_message().unwrap(), Message::Text("Hello".into()));
        // Fragmented unmasked text, with an unmasked ping in between: the
        // pong goes out, the message comes back whole.
        let mut frag = vec![0x01, 0x03, 0x48, 0x65, 0x6c];
        frag.extend_from_slice(&[0x89, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f]);
        frag.extend_from_slice(&[0x80, 0x02, 0x6c, 0x6f]);
        let (mut r, out) = receiver(Role::Client, frag);
        assert_eq!(r.read_message().unwrap(), Message::Text("Hello".into()));
        // Our pong is masked (client): opcode 0xA, mask bit, 5 bytes.
        let sent = out.lock().unwrap().clone();
        assert_eq!(&sent[..2], &[0x8a, 0x85]);
        let k = &sent[2..6];
        let pong: Vec<u8> = sent[6..]
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ k[i & 3])
            .collect();
        assert_eq!(pong, b"Hello");
        // A masked pong to a server is accepted and ignored.
        let mut pong_in = vec![
            0x8a, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
        ];
        pong_in.extend_from_slice(&MASKED_HELLO);
        let (mut r, _) = receiver(Role::Server, pong_in);
        assert_eq!(r.read_message().unwrap(), Message::Text("Hello".into()));
        // 256 bytes and 64 KiB binary headers.
        let f = encode_frame(true, OP_BINARY, &[0u8; 256], None);
        assert_eq!(&f[..4], &[0x82, 0x7e, 0x01, 0x00]);
        let f = encode_frame(true, OP_BINARY, &vec![0u8; 65536], None);
        assert_eq!(&f[..10], &[0x82, 0x7f, 0, 0, 0, 0, 0, 1, 0, 0]);
        let (mut r, _) = receiver(Role::Client, f);
        assert_eq!(r.read_message().unwrap(), Message::Binary(vec![0; 65536]));
    }

    #[test]
    fn close_is_echoed() {
        let mut input = encode_frame(true, OP_CLOSE, &[0x03, 0xe8, b'b', b'y', b'e'], None);
        input.extend_from_slice(&encode_frame(true, OP_TEXT, b"late", None));
        let (mut r, out) = receiver(Role::Client, input);
        assert_eq!(
            r.read_message().unwrap(),
            Message::Close(Some((1000, "bye".into())))
        );
        let sent = out.lock().unwrap().clone();
        assert_eq!(&sent[..2], &[0x88, 0x82]);
        assert!(r.read_message().is_err());
        assert!(r.sender().send_text("x").is_err());
    }

    fn rejects(role: Role, input: Vec<u8>, code: u16) {
        let (mut r, out) = receiver(role, input.clone());
        assert!(r.read_message().is_err(), "{input:02x?}");
        let sent = out.lock().unwrap().clone();
        assert!(sent.len() >= 4, "no close for {input:02x?}");
        let off = if role == Role::Client { 6 } else { 2 };
        let mut c = [sent[off], sent[off + 1]];
        if role == Role::Client {
            c[0] ^= sent[2];
            c[1] ^= sent[3];
        }
        assert_eq!(u16::from_be_bytes(c), code, "{input:02x?}");
        assert!(r.read_message().is_err());
    }

    #[test]
    fn violations_close_with_codes() {
        use close_code::*;
        let m = Some([1, 2, 3, 4]);
        // Unmasked frame to a server, masked frame to a client.
        rejects(
            Role::Server,
            encode_frame(true, OP_TEXT, b"hi", None),
            PROTOCOL,
        );
        rejects(
            Role::Client,
            encode_frame(true, OP_TEXT, b"hi", m),
            PROTOCOL,
        );
        // Reserved bits and opcodes.
        let mut f = encode_frame(true, OP_TEXT, b"hi", m);
        f[0] |= 0x40;
        rejects(Role::Server, f, PROTOCOL);
        rejects(Role::Server, encode_frame(true, 0x3, b"", m), PROTOCOL);
        rejects(Role::Server, encode_frame(true, 0xb, b"", m), PROTOCOL);
        // Fragmented and oversized control frames.
        rejects(Role::Server, encode_frame(false, OP_PING, b"", m), PROTOCOL);
        rejects(
            Role::Server,
            encode_frame(true, OP_PING, &[0; 126], m),
            PROTOCOL,
        );
        // Continuation without a start; a new message inside a fragment.
        rejects(Role::Server, encode_frame(true, OP_CONT, b"x", m), PROTOCOL);
        let mut f = encode_frame(false, OP_TEXT, b"a", m);
        f.extend(encode_frame(true, OP_TEXT, b"b", m));
        rejects(Role::Server, f, PROTOCOL);
        // Invalid UTF-8 (also split across fragments).
        rejects(
            Role::Server,
            encode_frame(true, OP_TEXT, &[0xff], m),
            INVALID_DATA,
        );
        let mut f = encode_frame(false, OP_TEXT, &[0xe2, 0x82], m);
        f.extend(encode_frame(true, OP_CONT, &[0x28], m));
        rejects(Role::Server, f, INVALID_DATA);
        // Close codes and payloads.
        rejects(
            Role::Server,
            encode_frame(true, OP_CLOSE, &[3], m),
            PROTOCOL,
        );
        rejects(
            Role::Server,
            encode_frame(true, OP_CLOSE, &[0x03, 0xed], m),
            PROTOCOL,
        );
        rejects(
            Role::Server,
            encode_frame(true, OP_CLOSE, &[0x03, 0xe8, 0xff], m),
            INVALID_DATA,
        );
        // Too big: one frame, and fragments adding up.
        let mut f = vec![0x82, 0xff];
        f.extend_from_slice(&(1u64 << 40).to_be_bytes());
        rejects(Role::Server, f, TOO_BIG);
        let mut f = vec![0x82, 0xff];
        f.extend_from_slice(&(1u64 << 63).to_be_bytes());
        rejects(Role::Server, f, PROTOCOL);
        let big = vec![0u8; 9 << 20];
        let mut f = encode_frame(false, OP_BINARY, &big, m);
        f.extend(encode_frame(true, OP_CONT, &big, m));
        rejects(Role::Server, f, TOO_BIG);
    }

    #[test]
    fn fragments_and_valid_utf8_split() {
        let m = Some([9, 8, 7, 6]);
        let mut f = encode_frame(false, OP_TEXT, &[0xe2, 0x82], m);
        f.extend(encode_frame(false, OP_CONT, &[], m));
        f.extend(encode_frame(true, OP_CONT, &[0xac], m));
        f.extend(encode_frame(true, OP_CLOSE, &[], m));
        let (mut r, out) = receiver(Role::Server, f);
        assert_eq!(r.read_message().unwrap(), Message::Text("€".into()));
        assert_eq!(r.read_message().unwrap(), Message::Close(None));
        // The echo of an empty close carries 1000.
        assert_eq!(*out.lock().unwrap(), [0x88, 0x02, 0x03, 0xe8]);
    }

    /// Delivers its bytes one at a time, with a timeout error in between.
    struct Trickle(Vec<u8>, usize, bool);

    impl Read for Trickle {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.2 = !self.2;
            if self.2 {
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "tick"));
            }
            if self.1 == self.0.len() || out.is_empty() {
                return Ok(0);
            }
            out[0] = self.0[self.1];
            self.1 += 1;
            Ok(1)
        }
    }

    #[test]
    fn timeouts_keep_partial_frames() {
        let m = Some([1, 1, 1, 1]);
        let mut data = encode_frame(true, OP_BINARY, &[7u8; 300], m);
        data.extend(encode_frame(true, OP_TEXT, b"ok", m));
        let sender = Sender::new(Shared::default(), Role::Server);
        let mut r = Receiver::new(
            Trickle(data, 0, false),
            Vec::new(),
            Role::Server,
            Limits::default(),
            sender,
        );
        let mut got = Vec::new();
        let mut timeouts = 0;
        while got.len() < 2 {
            match r.read_message() {
                Ok(m) => got.push(m),
                Err(e) if e.kind() == ErrorKind::Timeout => timeouts += 1,
                Err(e) => panic!("{e}"),
            }
        }
        assert!(timeouts > 300);
        assert_eq!(got[0], Message::Binary(vec![7; 300]));
        assert_eq!(got[1], Message::Text("ok".into()));
        // EOF without a close frame.
        assert_eq!(
            loop {
                match r.read_message() {
                    Err(e) if e.kind() == ErrorKind::Timeout => continue,
                    other => break other.unwrap_err().kind(),
                }
            },
            ErrorKind::Closed
        );
    }

    #[test]
    fn urls() {
        let u = WsUrl::parse("ws://127.0.0.1:9222/devtools/browser/abc").unwrap();
        assert_eq!(
            (u.host.as_str(), u.port, u.path.as_str()),
            ("127.0.0.1", 9222, "/devtools/browser/abc")
        );
        assert_eq!(u.authority(), "127.0.0.1:9222");
        let u = WsUrl::parse("ws://localhost?x=1#frag").unwrap();
        assert_eq!((u.port, u.path.as_str()), (80, "/?x=1"));
        assert_eq!(u.authority(), "localhost");
        let u = WsUrl::parse("ws://[::1]:8080").unwrap();
        assert_eq!(
            (u.host.as_str(), u.port, u.path.as_str()),
            ("[::1]", 8080, "/")
        );
        for bad in [
            "wss://x/",
            "http://x/",
            "ws://",
            "ws://u@h/",
            "ws://h:x/",
            "ws://[::1/",
        ] {
            assert!(WsUrl::parse(bad).is_err(), "{bad}");
        }
    }

    fn upgrade_request() -> Request {
        Request::new("GET", "/chat", Version::Http11)
            .header("Host", "server.example.com")
            .header("Upgrade", "websocket")
            .header("Connection", "keep-alive, Upgrade")
            .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("Sec-WebSocket-Version", "13")
    }

    #[test]
    fn server_handshake() {
        let req = upgrade_request();
        assert!(is_upgrade(&req));
        let resp = server_response(&req).unwrap();
        let text = String::from_utf8(resp.to_bytes().unwrap()).unwrap();
        assert_eq!(
            text,
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n"
        );
        let mut r = upgrade_request();
        r.headers.set("Sec-WebSocket-Version", "8");
        assert_eq!(
            server_response(&r).unwrap_err().kind(),
            ErrorKind::Unsupported
        );
        let mut r = upgrade_request();
        r.headers.set("Sec-WebSocket-Key", "c2hvcnQ=");
        assert!(server_response(&r).is_err());
        let mut r = upgrade_request();
        r.headers.set("Connection", "keep-alive");
        assert!(server_response(&r).is_err());
        let mut r = upgrade_request();
        r.method = "POST".into();
        assert!(server_response(&r).is_err());
    }

    /// Client and server over a real loopback socket: handshake, both
    /// directions, a big fragmented-free message, ping, close.
    #[test]
    fn loopback() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let mut conn = Conn::new(tcp);
            let req = conn.read_request().unwrap();
            assert_eq!(req.target, "/ws?k=1");
            conn.send_response(&server_response(&req).unwrap()).unwrap();
            let (tcp, rest) = conn.into_parts();
            let (mut rx, tx) = pair(tcp, rest, Role::Server, Limits::default()).unwrap();
            tx.send_text("welcome").unwrap();
            loop {
                match rx.read_message().unwrap() {
                    Message::Binary(b) => tx.send_binary(&b).unwrap(),
                    Message::Text(t) => tx.send_text(&t.to_uppercase()).unwrap(),
                    Message::Close(c) => return c,
                }
            }
        });
        let url = format!("ws://{addr}/ws?k=1");
        let (mut rx, tx) = connect(&url, Duration::from_secs(10), Limits::default()).unwrap();
        assert_eq!(rx.read_message().unwrap(), Message::Text("welcome".into()));
        tx.ping(b"p").unwrap();
        tx.send_text("hello").unwrap();
        assert_eq!(rx.read_message().unwrap(), Message::Text("HELLO".into()));
        let big: Vec<u8> = (0..200_000u32).map(|i| (i * 7) as u8).collect();
        tx.send_binary(&big).unwrap();
        assert_eq!(rx.read_message().unwrap(), Message::Binary(big));
        tx.close(close_code::GOING_AWAY, "done").unwrap();
        assert_eq!(
            rx.read_message().unwrap(),
            Message::Close(Some((close_code::GOING_AWAY, String::new())))
        );
        assert_eq!(
            server.join().unwrap(),
            Some((close_code::GOING_AWAY, "done".into()))
        );
    }

    #[test]
    fn fuzz_smoke() {
        // Random bytes never panic, in either role.
        let mut seed = 0x1234_5678_u32;
        for _ in 0..2000 {
            let n = (seed % 64) as usize;
            let data: Vec<u8> = (0..n)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    seed as u8
                })
                .collect();
            for role in [Role::Server, Role::Client] {
                let (mut r, _) = receiver(role, data.clone());
                for _ in 0..4 {
                    if r.read_message().is_err() {
                        break;
                    }
                }
            }
        }
    }
}
