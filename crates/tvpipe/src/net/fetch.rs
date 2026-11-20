//! A blocking HTTP/1.1 client for downloads (docs/playshare-design.md
//! §18): games, the games list and GitHub's API.
//!
//! One GET per connection (`Connection: close`) over TLS ([`super::tls`]:
//! Mozilla's roots plus `$SSL_CERT_FILE`) or, for loopback hosts and
//! callers that pin the body's hash, plain TCP. Redirects (301, 302, 303,
//! 307, 308) are followed, at most [`MAX_REDIRECTS`], never from https to
//! http; an `Authorization` value goes only to the host it is meant for.
//! The body streams to a sink with a size cap; connecting has
//! [`CONNECT_TIMEOUT`], every read [`READ_TIMEOUT`], and a stop request
//! ends the transfer between reads. No proxies, no compression (the
//! request asks for none), no keep-alive.

use std::fmt;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use super::http::{Conn, Headers, Limits, Request, Version};
use super::tls::{self, TlsStream};
use crate::error::{Error, Result};
use crate::util::stop::Stop;

/// Redirects followed at most.
pub const MAX_REDIRECTS: usize = 5;
/// Connecting (TCP) at most.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Silence on a connection at most.
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// The body of a non-2xx response is kept up to this (for the message).
const ERROR_BODY: usize = 64 * 1024;

/// An `http(s)://` URL: no user info; the fragment is dropped (callers
/// read it before).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub https: bool,
    /// Lowercase; IPv6 addresses without brackets.
    pub host: String,
    pub port: u16,
    /// The path and query (`/` at least).
    pub target: String,
}

fn bad_url(s: &str) -> Error {
    Error::usage(format!("not an http(s) URL: {s:?}"))
}

impl Url {
    /// Parses an absolute `http://` or `https://` URL.
    pub fn parse(s: &str) -> Result<Url> {
        let (https, rest) = if let Some(r) = strip_prefix_ci(s, "https://") {
            (true, r)
        } else if let Some(r) = strip_prefix_ci(s, "http://") {
            (false, r)
        } else {
            return Err(bad_url(s));
        };
        let rest = rest.split('#').next().unwrap_or("");
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, target) = rest.split_at(end);
        if authority.is_empty() || authority.contains('@') {
            return Err(bad_url(s));
        }
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (h, after) = v6.split_once(']').ok_or_else(|| bad_url(s))?;
            (h, after.strip_prefix(':'))
        } else {
            match authority.split_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (authority, None),
            }
        };
        let port = match port {
            None | Some("") => {
                if https {
                    443
                } else {
                    80
                }
            }
            Some(p) => p.parse().map_err(|_| bad_url(s))?,
        };
        let ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_');
        if host.is_empty() || !host.chars().all(ok) || port == 0 {
            return Err(bad_url(s));
        }
        if target.bytes().any(|b| b <= b' ' || b == 0x7f) {
            return Err(bad_url(s));
        }
        let target = if target.starts_with('/') {
            target.to_owned()
        } else {
            format!("/{target}")
        };
        Ok(Url {
            https,
            host: host.to_ascii_lowercase(),
            port,
            target,
        })
    }

    /// `host[:port]` as the `Host` header has it.
    pub fn authority(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == if self.https { 443 } else { 80 } {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }

    /// The path without the query.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("/")
    }

    /// A loopback host (`localhost`, `127.0.0.0/8`, `::1`).
    pub fn is_loopback(&self) -> bool {
        self.host == "localhost"
            || self
                .host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|a| a.is_loopback())
    }

    /// `location` (a redirect's target) relative to this URL.
    pub fn join(&self, location: &str) -> Result<Url> {
        let location = location.trim();
        if strip_prefix_ci(location, "https://").is_some()
            || strip_prefix_ci(location, "http://").is_some()
        {
            return Url::parse(location);
        }
        if let Some(rest) = location.strip_prefix("//") {
            let scheme = if self.https { "https" } else { "http" };
            return Url::parse(&format!("{scheme}://{rest}"));
        }
        if location.contains("://") {
            return Err(bad_url(location));
        }
        let location = location.split('#').next().unwrap_or("");
        let target = if location.starts_with('/') {
            location.to_owned()
        } else if let Some(q) = location.strip_prefix('?') {
            format!("{}?{q}", self.path())
        } else {
            let base = self.path();
            let dir = &base[..base.rfind('/').map_or(0, |i| i + 1)];
            format!("{dir}{location}")
        };
        let mut u = self.clone();
        u.target = if target.is_empty() {
            "/".into()
        } else {
            target
        };
        if u.target.bytes().any(|b| b <= b' ' || b == 0x7f) {
            return Err(bad_url(location));
        }
        Ok(u)
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scheme = if self.https { "https" } else { "http" };
        write!(f, "{scheme}://{}{}", self.authority(), self.target)
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    (s.len() >= prefix.len() && s.is_char_boundary(prefix.len()))
        .then(|| s.split_at(prefix.len()))
        .filter(|(p, _)| p.eq_ignore_ascii_case(prefix))
        .map(|(_, rest)| rest)
}

/// One GET.
#[derive(Debug, Clone)]
pub struct Get {
    pub url: Url,
    /// Extra request headers (`Accept`, `If-None-Match`).
    pub headers: Vec<(String, String)>,
    /// `Authorization` for this host only (`api.github.com`): dropped when
    /// a redirect leaves it.
    pub auth: Option<(String, String)>,
    /// The largest 2xx body.
    pub max_body: usize,
    /// Plain http to a host that is not loopback (the caller checks the
    /// body's hash).
    pub allow_http: bool,
}

impl Get {
    pub fn new(url: Url, max_body: usize) -> Get {
        Get {
            url,
            headers: Vec::new(),
            auth: None,
            max_body,
            allow_http: false,
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Get {
        self.headers.push((name.to_owned(), value.into()));
        self
    }
}

/// The final response of a GET (after redirects).
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Headers,
    /// Where it came from in the end.
    pub url: Url,
    /// For a status other than 2xx: the start of its body (an API's
    /// message); for 2xx the body went to the sink.
    pub error_body: Vec<u8>,
}

impl Reply {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The connection to one server.
enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

/// The client: trust anchors, timeouts, the stop request.
#[derive(Debug, Clone)]
pub struct Client {
    /// Extra trust anchors (DER) beside Mozilla's roots.
    pub extra_roots: Vec<Vec<u8>>,
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    /// Ends a transfer between reads.
    pub stop: Option<Stop>,
    /// Tests: these hosts connect to these addresses (as curl's
    /// `--resolve`), the name still checked by TLS.
    pub resolve: Vec<(String, SocketAddr)>,
    pub user_agent: String,
}

impl Default for Client {
    fn default() -> Client {
        Client {
            extra_roots: Vec::new(),
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: READ_TIMEOUT,
            stop: None,
            resolve: Vec::new(),
            user_agent: concat!("tvpipe/", env!("CARGO_PKG_VERSION")).to_owned(),
        }
    }
}

impl Client {
    /// Mozilla's roots and `$SSL_CERT_FILE`.
    pub fn from_env(stop: Option<Stop>) -> Client {
        Client {
            extra_roots: tls::env_roots("fetch"),
            stop,
            ..Client::default()
        }
    }

    fn stopped(&self) -> bool {
        self.stop.as_ref().is_some_and(Stop::requested)
    }

    fn connect(&self, url: &Url) -> Result<Stream> {
        let what = "fetch";
        let tcp = match self.resolve.iter().find(|(h, _)| *h == url.host) {
            Some((_, addr)) => {
                let tcp = TcpStream::connect_timeout(addr, self.connect_timeout)
                    .map_err(|e| Error::io(format!("{what}: connect {}", url.host), e))?;
                let _ = tcp.set_nodelay(true);
                tcp
            }
            None => tls::tcp(&url.host, url.port, self.connect_timeout, what)?,
        };
        tcp.set_read_timeout(Some(self.read_timeout))
            .and_then(|()| tcp.set_write_timeout(Some(self.read_timeout)))
            .map_err(|e| Error::io(format!("{what}: socket"), e))?;
        if !url.https {
            return Ok(Stream::Plain(tcp));
        }
        let config = tls::client_config(&self.extra_roots, what)?;
        Ok(Stream::Tls(Box::new(tls::handshake(
            tcp, config, &url.host, what,
        )?)))
    }

    /// GETs `req.url`, following redirects. A 2xx body goes to `sink`
    /// (`progress` hears bytes so far and the length when known); other
    /// statuses come back with the start of their body.
    pub fn get(
        &self,
        req: &Get,
        progress: &mut dyn FnMut(u64, Option<u64>),
        sink: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<Reply> {
        let mut url = req.url.clone();
        for hop in 0..=MAX_REDIRECTS {
            if !url.https && !url.is_loopback() && !req.allow_http {
                return Err(Error::usage(format!(
                    "{url}: plain http is only for this computer (or a download pinned with #sha256=)"
                )));
            }
            if self.stopped() {
                return Err(Error::closed("fetch: stopped"));
            }
            let mut r = Request::new("GET", &url.target, Version::Http11)
                .header("Host", url.authority())
                .header("User-Agent", self.user_agent.clone())
                .header("Accept-Encoding", "identity")
                .header("Connection", "close");
            for (k, v) in &req.headers {
                r = r.header(k, v.clone());
            }
            if let Some((host, value)) = &req.auth {
                if *host == url.host && (url.https || url.is_loopback()) {
                    r = r.header("Authorization", value.clone());
                }
            }
            let limits = Limits {
                max_body: req.max_body,
                ..Limits::default()
            };
            let mut conn = Conn::with_limits(self.connect(&url)?, limits);
            conn.send_request(&r)?;
            let head = conn.read_response_head()?;
            if matches!(head.status, 301 | 302 | 303 | 307 | 308) {
                let Some(loc) = head.headers.get("location") else {
                    return Err(Error::protocol(format!(
                        "{url}: redirect {} without a Location",
                        head.status
                    )));
                };
                let next = url.join(loc)?;
                if url.https && !next.https {
                    return Err(Error::protocol(format!(
                        "{url}: refusing a redirect from https to {next}"
                    )));
                }
                if hop == MAX_REDIRECTS {
                    return Err(Error::protocol(format!(
                        "{}: more than {MAX_REDIRECTS} redirects",
                        req.url
                    )));
                }
                trace!("fetch: {url} → {next} ({})", head.status);
                url = next;
                continue;
            }
            if !(200..300).contains(&head.status) {
                let mut body = Vec::new();
                conn.limits_mut().max_body = usize::MAX;
                let _ = conn.read_body_into(&head, false, &mut |d| {
                    let room = ERROR_BODY.saturating_sub(body.len());
                    body.extend_from_slice(&d[..d.len().min(room)]);
                    if body.len() >= ERROR_BODY {
                        return Err(Error::closed("enough"));
                    }
                    Ok(())
                });
                return Ok(Reply {
                    status: head.status,
                    headers: head.headers,
                    url,
                    error_body: body,
                });
            }
            let total = head.headers.content_length()?.map(|n| n as u64);
            if total.is_some_and(|n| n > req.max_body as u64) {
                return Err(Error::usage(format!(
                    "{url}: {} bytes, more than the {} allowed",
                    total.unwrap_or(0),
                    req.max_body
                )));
            }
            let mut done = 0u64;
            progress(0, total);
            conn.read_body_into(&head, false, &mut |d| {
                if self.stopped() {
                    return Err(Error::closed("fetch: stopped"));
                }
                done += d.len() as u64;
                sink(d)?;
                progress(done, total);
                Ok(())
            })
            .map_err(|e| {
                if e.message().contains("exceeds the limit") {
                    Error::usage(format!(
                        "{url}: more than the {} bytes allowed",
                        req.max_body
                    ))
                } else {
                    e
                }
            })?;
            if total.is_some_and(|n| n != done) {
                return Err(Error::closed(format!("{url}: the download ended early")));
            }
            return Ok(Reply {
                status: head.status,
                headers: head.headers,
                url,
                error_body: Vec::new(),
            });
        }
        Err(Error::protocol(format!("{}: too many redirects", req.url)))
    }

    /// One request with any method (an API call): no redirects (a 3xx
    /// is just its status), `body` as `(content type, bytes)` with a
    /// `Content-Length`. Plain http only to loopback hosts. A 2xx body
    /// over `max_body` is an error; other statuses keep its start.
    pub fn send(
        &self,
        method: &str,
        url: &Url,
        headers: &[(&str, &str)],
        body: Option<(&str, Vec<u8>)>,
        max_body: usize,
    ) -> Result<(u16, Vec<u8>)> {
        if !url.https && !url.is_loopback() {
            return Err(Error::usage(format!(
                "{url}: plain http is only for this computer"
            )));
        }
        if self.stopped() {
            return Err(Error::closed("fetch: stopped"));
        }
        let mut r = Request::new(method, &url.target, Version::Http11)
            .header("Host", url.authority())
            .header("User-Agent", self.user_agent.clone())
            .header("Accept-Encoding", "identity")
            .header("Connection", "close");
        for (k, v) in headers {
            r = r.header(k, *v);
        }
        match body {
            Some((content_type, bytes)) => r = r.body(content_type, bytes),
            None if matches!(method, "POST" | "PUT" | "PATCH") => {
                r = r.header("Content-Length", "0");
            }
            None => {}
        }
        let limits = Limits {
            max_body: usize::MAX,
            ..Limits::default()
        };
        let mut conn = Conn::with_limits(self.connect(url)?, limits);
        conn.send_request(&r)?;
        let head = conn.read_response_head()?;
        let ok = (200..300).contains(&head.status);
        if ok && head.headers.content_length()?.is_some_and(|n| n > max_body) {
            return Err(Error::protocol(format!(
                "{method} {url}: an answer of more than {max_body} bytes"
            )));
        }
        let mut out = Vec::new();
        let read = conn.read_body_into(&head, method == "HEAD", &mut |d| {
            if self.stopped() {
                return Err(Error::closed("fetch: stopped"));
            }
            let room = max_body.saturating_sub(out.len());
            out.extend_from_slice(&d[..d.len().min(room)]);
            if d.len() > room {
                return Err(Error::closed("enough"));
            }
            Ok(())
        });
        match read {
            Ok(()) => {}
            Err(_) if !ok && out.len() >= max_body => {}
            Err(e) if e.message() == "enough" => {
                return Err(Error::protocol(format!(
                    "{method} {url}: an answer of more than {max_body} bytes"
                )));
            }
            Err(e) => return Err(e),
        }
        Ok((head.status, out))
    }

    /// [`Client::get`] into memory: the reply and, for 2xx, the body.
    pub fn get_bytes(&self, req: &Get) -> Result<(Reply, Vec<u8>)> {
        let mut body = Vec::new();
        let reply = self.get(req, &mut |_, _| {}, &mut |d| {
            body.extend_from_slice(d);
            Ok(())
        })?;
        Ok((reply, body))
    }
}

#[cfg(test)]
pub(crate) mod tests;
