//! Minimal HTTP/1.1 message framing, shared with RTSP (RFC 9112, RFC 2326).
//!
//! [`Conn`] wraps any `Read + Write` stream (a `TcpStream`, or the
//! encrypted AirPlay control channel) with a read buffer, and reads and
//! writes whole messages: start line, headers, and a body delimited by
//! `Content-Length`, chunked transfer coding (HTTP only), or connection
//! close (HTTP responses only). Every read is bounded by [`Limits`].
//!
//! Messages are written in a single `write_all`, which keeps the
//! encrypted channel's framing to one frame per small message.

use std::fmt;
use std::io::{Read, Write};

use crate::error::{Error, Result};

/// Size limits for incoming messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Longest start line or header line.
    pub max_line: usize,
    /// Total size of the start line and all header lines.
    pub max_head: usize,
    pub max_headers: usize,
    pub max_body: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_line: 8 * 1024,
            max_head: 64 * 1024,
            max_headers: 128,
            max_body: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    Http10,
    Http11,
    Rtsp10,
}

impl Version {
    pub fn as_str(self) -> &'static str {
        match self {
            Version::Http10 => "HTTP/1.0",
            Version::Http11 => "HTTP/1.1",
            Version::Rtsp10 => "RTSP/1.0",
        }
    }

    fn parse(s: &str) -> Result<Version> {
        match s {
            "HTTP/1.1" => Ok(Version::Http11),
            "HTTP/1.0" => Ok(Version::Http10),
            "RTSP/1.0" => Ok(Version::Rtsp10),
            _ => Err(Error::malformed(format!("http: unsupported version {s:?}"))),
        }
    }

    pub fn is_rtsp(self) -> bool {
        self == Version::Rtsp10
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Header fields in order. Names compare case-insensitively.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers {
    entries: Vec<(String, String)>,
}

impl Headers {
    pub fn new() -> Headers {
        Headers::default()
    }

    /// The first value of `name`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn get_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.entries
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Appends a field (keeps existing ones with the same name).
    pub fn add(&mut self, name: impl Into<String>, value: impl Into<String>) -> &mut Headers {
        self.entries.push((name.into(), value.into()));
        self
    }

    /// Replaces all fields named `name` with one, in the first one's place.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) -> &mut Headers {
        let name = name.into();
        let value = value.into();
        match self
            .entries
            .iter()
            .position(|(k, _)| k.eq_ignore_ascii_case(&name))
        {
            Some(i) => {
                self.entries[i] = (name, value);
                let mut j = i + 1;
                while j < self.entries.len() {
                    if self.entries[j].0.eq_ignore_ascii_case(&self.entries[i].0) {
                        self.entries.remove(j);
                    } else {
                        j += 1;
                    }
                }
            }
            None => self.entries.push((name, value)),
        }
        self
    }

    pub fn remove(&mut self, name: &str) {
        self.entries.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `Content-Length`, validated (RFC 9112 §6.3: repeated values must
    /// agree).
    pub fn content_length(&self) -> Result<Option<usize>> {
        let mut out = None;
        for v in self.get_all("content-length") {
            for part in v.split(',') {
                let part = part.trim();
                if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(Error::malformed(format!("http: bad Content-Length {v:?}")));
                }
                let n: usize = part
                    .parse()
                    .map_err(|_| Error::malformed(format!("http: Content-Length too big {v:?}")))?;
                if out.is_some_and(|o| o != n) {
                    return Err(Error::malformed("http: conflicting Content-Length values"));
                }
                out = Some(n);
            }
        }
        Ok(out)
    }

    /// Whether the last transfer coding is `chunked`, and whether any
    /// transfer coding is present.
    fn transfer_coding(&self) -> (bool, bool) {
        let mut any = false;
        let mut last_chunked = false;
        for v in self.get_all("transfer-encoding") {
            for coding in v.split(',') {
                let coding = coding.trim();
                if coding.is_empty() {
                    continue;
                }
                any = true;
                last_chunked = coding.eq_ignore_ascii_case("chunked");
            }
        }
        (any, last_chunked)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub version: Version,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl Request {
    pub fn new(method: &str, target: &str, version: Version) -> Request {
        Request {
            method: method.to_owned(),
            target: target.to_owned(),
            version,
            headers: Headers::new(),
            body: Vec::new(),
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Request {
        self.headers.set(name, value);
        self
    }

    /// Sets the body and its `Content-Type`.
    pub fn body(mut self, content_type: &str, body: Vec<u8>) -> Request {
        self.headers.set("Content-Type", content_type);
        self.body = body;
        self
    }

    /// The serialized message. `Content-Length` is added when there is a
    /// body and none was set.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        check_token(&self.method, "method")?;
        if self.target.is_empty() || self.target.bytes().any(|b| b <= b' ' || b == 0x7f) {
            return Err(Error::malformed(format!(
                "http: invalid request target {:?}",
                self.target
            )));
        }
        let mut out = format!("{} {} {}\r\n", self.method, self.target, self.version).into_bytes();
        write_head(&mut out, &self.headers, &self.body, !self.body.is_empty())?;
        out.extend_from_slice(&self.body);
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub version: Version,
    pub status: u16,
    pub reason: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(version: Version, status: u16, reason: &str) -> Response {
        Response {
            version,
            status,
            reason: reason.to_owned(),
            headers: Headers::new(),
            body: Vec::new(),
        }
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// `Ok(self)` for 2xx; otherwise an error naming the status
    /// (401/403/470 map to [`ErrorKind::Auth`](crate::error::ErrorKind)).
    pub fn error_for_status(self, what: &str) -> Result<Response> {
        if self.is_success() {
            return Ok(self);
        }
        let msg = format!("{what}: {} {} {}", self.version, self.status, self.reason);
        Err(match self.status {
            401 | 403 | 470 => Error::auth(msg),
            404 => Error::not_found(msg),
            _ => Error::protocol(msg),
        })
    }

    /// The serialized message; `Content-Length` is always present except
    /// for statuses that cannot have a body.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        if !(100..1000).contains(&self.status) {
            return Err(Error::malformed(format!(
                "http: invalid status {}",
                self.status
            )));
        }
        if self.reason.bytes().any(|b| b == b'\r' || b == b'\n') {
            return Err(Error::malformed("http: invalid reason phrase"));
        }
        let mut out = format!("{} {} {}\r\n", self.version, self.status, self.reason).into_bytes();
        let bodyless = self.status < 200 || self.status == 204 || self.status == 304;
        if bodyless && !self.body.is_empty() {
            return Err(Error::malformed(format!(
                "http: status {} cannot have a body",
                self.status
            )));
        }
        write_head(&mut out, &self.headers, &self.body, !bodyless)?;
        out.extend_from_slice(&self.body);
        Ok(out)
    }
}

fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

fn check_token(s: &str, what: &str) -> Result<()> {
    if s.is_empty() || !s.bytes().all(is_tchar) {
        return Err(Error::malformed(format!("http: invalid {what} {s:?}")));
    }
    Ok(())
}

fn write_head(out: &mut Vec<u8>, headers: &Headers, body: &[u8], add_length: bool) -> Result<()> {
    for (k, v) in headers.iter() {
        check_token(k, "header name")?;
        if v.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
            return Err(Error::malformed(format!(
                "http: invalid value for header {k}"
            )));
        }
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(v.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    if add_length && !headers.contains("content-length") && !headers.contains("transfer-encoding") {
        out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    Ok(())
}

/// A buffered connection that reads and writes whole messages.
pub struct Conn<S> {
    stream: S,
    buf: Vec<u8>,
    start: usize,
    end: usize,
    limits: Limits,
}

const READ_CHUNK: usize = 16 * 1024;

impl<S: Read + Write> Conn<S> {
    pub fn new(stream: S) -> Conn<S> {
        Conn::with_limits(stream, Limits::default())
    }

    pub fn with_limits(stream: S, limits: Limits) -> Conn<S> {
        Conn {
            stream,
            buf: Vec::new(),
            start: 0,
            end: 0,
            limits,
        }
    }

    pub fn limits_mut(&mut self) -> &mut Limits {
        &mut self.limits
    }

    pub fn get_ref(&self) -> &S {
        &self.stream
    }

    pub fn get_mut(&mut self) -> &mut S {
        &mut self.stream
    }

    /// Bytes read from the stream but not consumed yet.
    pub fn buffered(&self) -> &[u8] {
        &self.buf[self.start..self.end]
    }

    /// The stream and any buffered, unconsumed bytes (to switch to an
    /// encrypted wrapper after pair-verify: the leftover should be empty).
    pub fn into_parts(self) -> (S, Vec<u8>) {
        let rest = self.buf[self.start..self.end].to_vec();
        (self.stream, rest)
    }

    pub fn send_request(&mut self, req: &Request) -> Result<()> {
        let bytes = req.to_bytes()?;
        self.write_all(&bytes)
    }

    pub fn send_response(&mut self, resp: &Response) -> Result<()> {
        let bytes = resp.to_bytes()?;
        self.write_all(&bytes)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.stream
            .write_all(bytes)
            .and_then(|()| self.stream.flush())
            .map_err(|e| Error::io("http: write", e))
    }

    /// Sends `req` and reads its final response (skipping 1xx).
    pub fn request(&mut self, req: &Request) -> Result<Response> {
        self.send_request(req)?;
        let head = req.method.eq_ignore_ascii_case("HEAD");
        loop {
            let resp = self.read_response(head)?;
            if resp.status >= 200 {
                return Ok(resp);
            }
            trace!("skipping interim response {}", resp.status);
        }
    }

    /// Reads one response. `head_request` suppresses the body (RFC 9112
    /// §6.3 rule 1).
    pub fn read_response(&mut self, head_request: bool) -> Result<Response> {
        let (start, headers) = self.read_head()?;
        let mut parts = start.splitn(3, ' ');
        let version = Version::parse(parts.next().unwrap_or(""))?;
        let code = parts.next().unwrap_or("");
        if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::malformed(format!("http: bad status line {start:?}")));
        }
        let status: u16 = code
            .parse()
            .map_err(|_| Error::malformed(format!("http: bad status line {start:?}")))?;
        let reason = parts.next().unwrap_or("").to_owned();
        let bodyless = head_request || status < 200 || status == 204 || status == 304;
        let body = if bodyless {
            Vec::new()
        } else {
            self.read_body(&headers, version, true)?
        };
        Ok(Response {
            version,
            status,
            reason,
            headers,
            body,
        })
    }

    /// Reads one request (for the receiver-initiated channels).
    pub fn read_request(&mut self) -> Result<Request> {
        let (start, headers) = self.read_head()?;
        let parts: Vec<&str> = start.split(' ').collect();
        let [method, target, version] = parts[..] else {
            return Err(Error::malformed(format!(
                "http: bad request line {start:?}"
            )));
        };
        check_token(method, "method")?;
        if target.is_empty() {
            return Err(Error::malformed("http: empty request target"));
        }
        let version = Version::parse(version)?;
        let body = self.read_body(&headers, version, false)?;
        Ok(Request {
            method: method.to_owned(),
            target: target.to_owned(),
            version,
            headers,
            body,
        })
    }

    /// Reads the start line and header fields.
    fn read_head(&mut self) -> Result<(String, Headers)> {
        let mut total = 0;
        // RFC 9112 §2.2: ignore at least one empty line before a message.
        let start = loop {
            let line = self.read_line(&mut total, true)?;
            if !line.is_empty() {
                break line;
            }
        };
        let mut headers = Headers::new();
        loop {
            let line = self.read_line(&mut total, false)?;
            if line.is_empty() {
                break;
            }
            if line.starts_with([' ', '\t']) {
                // Obsolete line folding (RFC 9112 §5.2): join with a space.
                let Some(last) = headers.entries.last_mut() else {
                    return Err(Error::malformed(
                        "http: continuation line before any header",
                    ));
                };
                last.1.push(' ');
                last.1.push_str(line.trim());
                continue;
            }
            let Some((name, value)) = line.split_once(':') else {
                return Err(Error::malformed(format!("http: bad header line {line:?}")));
            };
            check_token(name, "header name")?;
            if headers.len() >= self.limits.max_headers {
                return Err(Error::malformed("http: too many header fields"));
            }
            headers.add(name, value.trim_matches([' ', '\t']));
        }
        Ok((start, headers))
    }

    fn read_body(
        &mut self,
        headers: &Headers,
        version: Version,
        response: bool,
    ) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        self.read_body_with(headers, version, response, &mut |d| {
            body.extend_from_slice(d);
            Ok(())
        })?;
        Ok(body)
    }

    /// Reads a response's status line and header fields, skipping 1xx
    /// responses; the body (empty here) is still to be read with
    /// [`Conn::read_body_into`]. For bodies too large to keep in memory.
    pub fn read_response_head(&mut self) -> Result<Response> {
        loop {
            let (start, headers) = self.read_head()?;
            let mut parts = start.splitn(3, ' ');
            let version = Version::parse(parts.next().unwrap_or(""))?;
            let code = parts.next().unwrap_or("");
            if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Error::malformed(format!("http: bad status line {start:?}")));
            }
            let status: u16 = code
                .parse()
                .map_err(|_| Error::malformed(format!("http: bad status line {start:?}")))?;
            if status < 200 {
                continue;
            }
            return Ok(Response {
                version,
                status,
                reason: parts.next().unwrap_or("").to_owned(),
                headers,
                body: Vec::new(),
            });
        }
    }

    /// Streams the body of `resp` (from [`Conn::read_response_head`]) to
    /// `sink` in pieces; `max_body` applies. `head_request`: there is none.
    pub fn read_body_into(
        &mut self,
        resp: &Response,
        head_request: bool,
        sink: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        if head_request || resp.status == 204 || resp.status == 304 {
            return Ok(());
        }
        self.read_body_with(&resp.headers, resp.version, true, sink)
    }

    fn read_body_with(
        &mut self,
        headers: &Headers,
        version: Version,
        response: bool,
        sink: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let (coded, chunked) = headers.transfer_coding();
        if coded && !version.is_rtsp() {
            if chunked {
                return self.read_chunked(sink);
            }
            if response {
                return self.read_to_close(sink);
            }
            return Err(Error::malformed(
                "http: request with unknown transfer coding",
            ));
        }
        match headers.content_length()? {
            Some(n) => {
                if n > self.limits.max_body {
                    return Err(Error::malformed(format!(
                        "http: body of {n} bytes exceeds the limit of {}",
                        self.limits.max_body
                    )));
                }
                self.read_exact_into(n, sink)
            }
            None if response && !version.is_rtsp() => self.read_to_close(sink),
            None => Ok(()),
        }
    }

    fn read_chunked(&mut self, sink: &mut dyn FnMut(&[u8]) -> Result<()>) -> Result<()> {
        let mut len = 0usize;
        let mut total = 0;
        loop {
            // Chunk-size lines do not count towards the header section's
            // size (a long body has many); each is bounded by `max_line`.
            let mut t = 0;
            let line = self.read_line(&mut t, false)?;
            let size = line.split(';').next().unwrap_or("").trim();
            if size.is_empty() || size.len() > 16 || !size.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Error::malformed(format!("http: bad chunk size {line:?}")));
            }
            let n = usize::from_str_radix(size, 16)
                .map_err(|_| Error::malformed("http: chunk size too big"))?;
            if n == 0 {
                break;
            }
            len = len.saturating_add(n);
            if len > self.limits.max_body {
                return Err(Error::malformed("http: chunked body exceeds the limit"));
            }
            self.read_exact_into(n, sink)?;
            // The chunk's CRLF is not part of the header section's size.
            let mut crlf = 0;
            if !self.read_line(&mut crlf, false)?.is_empty() {
                return Err(Error::malformed("http: missing CRLF after chunk"));
            }
        }
        // Trailer fields are read and dropped.
        while !self.read_line(&mut total, false)?.is_empty() {}
        Ok(())
    }

    fn read_to_close(&mut self, sink: &mut dyn FnMut(&[u8]) -> Result<()>) -> Result<()> {
        let mut len = self.end - self.start;
        if len > self.limits.max_body {
            return Err(Error::malformed("http: body exceeds the limit"));
        }
        sink(&self.buf[self.start..self.end])?;
        self.start = self.end;
        let mut chunk = [0u8; READ_CHUNK];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(()),
                Ok(n) => {
                    len += n;
                    if len > self.limits.max_body {
                        return Err(Error::malformed("http: body exceeds the limit"));
                    }
                    sink(&chunk[..n])?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(Error::io("http: read body", e)),
            }
        }
    }

    /// Reads one line without its terminator (CRLF, or bare LF). The
    /// line must be ASCII-compatible; invalid UTF-8 is replaced.
    fn read_line(&mut self, total: &mut usize, allow_eof: bool) -> Result<String> {
        let mut scanned = 0;
        loop {
            let avail = &self.buf[self.start..self.end];
            if let Some(i) = avail[scanned..].iter().position(|&b| b == b'\n') {
                let i = scanned + i;
                let mut line = &avail[..i];
                if line.last() == Some(&b'\r') {
                    line = &line[..line.len() - 1];
                }
                if line.len() > self.limits.max_line {
                    return Err(Error::malformed("http: line too long"));
                }
                *total += i + 1;
                if *total > self.limits.max_head {
                    return Err(Error::malformed("http: header section too large"));
                }
                let s = String::from_utf8_lossy(line).into_owned();
                self.start += i + 1;
                return Ok(s);
            }
            scanned = avail.len();
            if scanned > self.limits.max_line + 1 {
                return Err(Error::malformed("http: line too long"));
            }
            if self.fill()? == 0 {
                return Err(if allow_eof && self.start == self.end {
                    Error::closed("http: connection closed")
                } else {
                    Error::closed("http: connection closed mid-message")
                });
            }
        }
    }

    fn read_exact_into(
        &mut self,
        n: usize,
        sink: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let take = n.min(self.end - self.start);
        sink(&self.buf[self.start..self.start + take])?;
        self.start += take;
        let mut left = n - take;
        let mut chunk = [0u8; READ_CHUNK];
        while left > 0 {
            let want = left.min(chunk.len());
            match self.stream.read(&mut chunk[..want]) {
                Ok(0) => return Err(Error::closed("http: connection closed mid-body")),
                Ok(k) => {
                    sink(&chunk[..k])?;
                    left -= k;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(Error::io("http: read body", e)),
            }
        }
        Ok(())
    }

    /// Reads more bytes into the buffer; returns how many (0 at EOF).
    fn fill(&mut self) -> Result<usize> {
        if self.start > 0 {
            self.buf.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
        }
        if self.buf.len() < self.end + READ_CHUNK {
            self.buf.resize(self.end + READ_CHUNK, 0);
        }
        loop {
            match self.stream.read(&mut self.buf[self.end..]) {
                Ok(n) => {
                    self.end += n;
                    return Ok(n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(Error::io("http: read", e)),
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::util::vectors;
    use std::io::Cursor;

    /// An in-memory duplex stream: reads come from `input` (optionally
    /// in tiny pieces), writes go to `output`.
    pub(crate) struct Duplex {
        pub input: Cursor<Vec<u8>>,
        pub output: Vec<u8>,
        pub piece: usize,
    }

    impl Duplex {
        pub(crate) fn new(input: &[u8]) -> Duplex {
            Duplex {
                input: Cursor::new(input.to_vec()),
                output: Vec::new(),
                piece: usize::MAX,
            }
        }
    }

    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.piece);
            self.input.read(&mut buf[..n])
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn unescape(s: &str) -> Vec<u8> {
        s.replace("\\r", "\r").replace("\\n", "\n").into_bytes()
    }

    #[test]
    fn writes_requests() {
        let req = Request::new("POST", "/pair-setup", Version::Rtsp10)
            .header("CSeq", "1")
            .body("application/octet-stream", vec![1, 2, 3]);
        assert_eq!(
            req.to_bytes().unwrap(),
            b"POST /pair-setup RTSP/1.0\r\nCSeq: 1\r\nContent-Type: application/octet-stream\r\nContent-Length: 3\r\n\r\n\x01\x02\x03"
        );
        let get = Request::new("GET", "/info", Version::Http11).header("Host", "tv");
        assert_eq!(
            get.to_bytes().unwrap(),
            b"GET /info HTTP/1.1\r\nHost: tv\r\n\r\n"
        );
        for bad in [
            Request::new("GET", "/a b", Version::Http11),
            Request::new("G T", "/", Version::Http11),
            Request::new("GET", "/", Version::Http11).header("X", "a\r\nInjected: 1"),
            Request::new("GET", "/", Version::Http11).header("Bad Name", "1"),
        ] {
            assert!(bad.to_bytes().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn writes_responses() {
        let mut r = Response::new(Version::Rtsp10, 200, "OK");
        r.headers.add("CSeq", "3");
        assert_eq!(
            r.to_bytes().unwrap(),
            b"RTSP/1.0 200 OK\r\nCSeq: 3\r\nContent-Length: 0\r\n\r\n"
        );
        let mut nc = Response::new(Version::Http11, 204, "No Content");
        assert_eq!(nc.to_bytes().unwrap(), b"HTTP/1.1 204 No Content\r\n\r\n");
        nc.body = vec![1];
        assert!(nc.to_bytes().is_err());
    }

    #[test]
    fn headers_semantics() {
        let mut h = Headers::new();
        h.add("Content-Length", "5").add("X-A", "1").add("x-a", "2");
        assert_eq!(h.get("content-length"), Some("5"));
        assert_eq!(h.get_all("X-a").collect::<Vec<_>>(), ["1", "2"]);
        h.set("X-A", "3");
        assert_eq!(h.get_all("x-a").collect::<Vec<_>>(), ["3"]);
        assert_eq!(h.len(), 2);
        assert_eq!(h.content_length().unwrap(), Some(5));
        h.add("Content-Length", "5, 5");
        assert_eq!(h.content_length().unwrap(), Some(5));
        h.add("Content-Length", "6");
        assert!(h.content_length().is_err());
        h.set("Content-Length", "-1");
        assert!(h.content_length().is_err());
        h.set("Content-Length", "99999999999999999999999");
        assert!(h.content_length().is_err());
        h.remove("content-length");
        assert_eq!(h.content_length().unwrap(), None);
    }

    /// Response parsing known answers from `tests/vectors/http/responses.txt`.
    #[test]
    fn response_vectors() {
        for b in vectors::load("http/responses.txt") {
            let raw = unescape(b.str("raw"));
            for piece in [usize::MAX, 1, 7] {
                let mut d = Duplex::new(&raw);
                d.piece = piece;
                let mut conn = Conn::new(d);
                let head = b.get("head").is_some();
                let got = conn.read_response(head);
                match b.get("error") {
                    Some(kind) => {
                        let err = got.expect_err(&b.name());
                        assert_eq!(format!("{:?}", err.kind()), kind, "{}: {err}", b.name());
                    }
                    None => {
                        let resp = got.unwrap_or_else(|e| panic!("{}: {e}", b.name()));
                        assert_eq!(resp.status, b.parse::<u16>("status"), "{}", b.name());
                        assert_eq!(resp.version.as_str(), b.str("version"), "{}", b.name());
                        assert_eq!(resp.reason, b.get("reason").unwrap_or(""), "{}", b.name());
                        let body = unescape(b.get("body").unwrap_or(""));
                        assert_eq!(resp.body, body, "{}", b.name());
                        if let Some(h) = b.get("headers") {
                            let got: Vec<String> = resp
                                .headers
                                .iter()
                                .map(|(k, v)| format!("{k}={v}"))
                                .collect();
                            assert_eq!(got.join(" | "), h, "{}", b.name());
                        }
                        let rest = unescape(b.get("rest").unwrap_or(""));
                        let mut leftover = conn.buffered().to_vec();
                        conn.get_mut().input.read_to_end(&mut leftover).unwrap();
                        assert_eq!(leftover, rest, "{}", b.name());
                    }
                }
            }
        }
    }

    #[test]
    fn pipelined_responses_and_interim() {
        let raw = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhiRTSP/1.0 200 OK\r\nCSeq: 2\r\n\r\n";
        let mut conn = Conn::new(Duplex::new(raw));
        let resp = conn
            .request(&Request::new("POST", "/x", Version::Http11))
            .unwrap();
        assert_eq!((resp.status, resp.body.as_slice()), (200, &b"hi"[..]));
        assert_eq!(conn.get_ref().output, b"POST /x HTTP/1.1\r\n\r\n");
        let second = conn.read_response(false).unwrap();
        assert_eq!(second.headers.get("cseq"), Some("2"));
        let err = conn.read_response(false).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Closed);
    }

    #[test]
    fn reads_requests() {
        let raw = b"\r\nPOST /command RTSP/1.0\r\nCSeq: 7\r\nContent-Length: 4\r\n\r\nbodyGET /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n";
        let mut conn = Conn::new(Duplex::new(raw));
        let req = conn.read_request().unwrap();
        assert_eq!(
            (req.method.as_str(), req.target.as_str()),
            ("POST", "/command")
        );
        assert_eq!(req.version, Version::Rtsp10);
        assert_eq!(req.body, b"body");
        let req = conn.read_request().unwrap();
        assert_eq!(req.body, b"abc");
        for bad in [
            &b"GET /\r\n\r\n"[..],
            b"GET / HTTP/2\r\n\r\n",
            b"G@T / HTTP/1.1\r\n\r\n",
        ] {
            assert!(Conn::new(Duplex::new(bad)).read_request().is_err());
        }
        // Requests never read to close.
        let mut conn = Conn::new(Duplex::new(b"GET / HTTP/1.1\r\n\r\nrest"));
        assert!(conn.read_request().unwrap().body.is_empty());
        assert_eq!(conn.buffered(), b"rest");
    }

    #[test]
    fn limits_are_enforced() {
        let limits = Limits {
            max_line: 32,
            max_head: 64,
            max_headers: 2,
            max_body: 8,
        };
        let cases: [&[u8]; 6] = [
            b"HTTP/1.1 200 OK\r\nX-Long: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nA: 1\r\nB: 2\r\nC: 3\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nA: 1111111111111111111111\r\nB: 2222222222222222222222\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n123456789",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n12345\r\n5\r\n12345\r\n0\r\n\r\n",
            b"HTTP/1.1 200 OK\r\n\r\n123456789",
        ];
        for raw in cases {
            let mut conn = Conn::with_limits(Duplex::new(raw), limits);
            let err = conn.read_response(false).unwrap_err();
            assert_eq!(
                err.kind(),
                ErrorKind::Malformed,
                "{}",
                String::from_utf8_lossy(raw)
            );
        }
        // A line without a newline must not be buffered forever.
        let endless = vec![b'a'; 1000];
        let mut conn = Conn::with_limits(Duplex::new(&endless), limits);
        assert_eq!(
            conn.read_response(false).unwrap_err().kind(),
            ErrorKind::Malformed
        );
    }

    #[test]
    fn garbage_never_panics() {
        let base = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nX: y\r\n\r\n4;ext=1\r\nwxyz\r\n0\r\nT: 1\r\n\r\n";
        for i in 0..base.len() {
            for v in [0u8, b'\n', b' ', b':', b'0', 0xff] {
                let mut m = base.to_vec();
                m[i] = v;
                let _ = Conn::new(Duplex::new(&m)).read_response(false);
                let _ = Conn::new(Duplex::new(&m[..i])).read_response(false);
            }
        }
    }

    #[test]
    fn into_parts_returns_leftover() {
        let mut conn = Conn::new(Duplex::new(b"RTSP/1.0 200 OK\r\n\r\nEXTRA"));
        conn.read_response(false).unwrap();
        let (_, rest) = conn.into_parts();
        assert_eq!(rest, b"EXTRA");
    }
}
