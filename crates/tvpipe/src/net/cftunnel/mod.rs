//! A native Cloudflare Quick Tunnel client (`--tunnel`; docs/tunnels.md):
//! a public `https://RANDOM.trycloudflare.com` for a loopback port, with
//! no account and no cloudflared.
//!
//! 1. **Credentials**: `POST https://api.trycloudflare.com/tunnel` (no
//!    body) returns a tunnel's ID, hostname, account tag and secret.
//! 2. **Edge**: TLS to `region1.v2.argotunnel.com:7844` (`region2` on
//!    every other attempt; the names the SRV record
//!    `_v2-origintunneld._tcp.argotunnel.com` lists), SNI
//!    `h2.cftunnel.com`, verified against Cloudflare's origin CA (not a
//!    public root). It is HTTP/2 with the roles swapped: we dialed, but the
//!    edge is the client and opens streams ([`h2`]).
//! 3. **Registration**: the edge's first stream carries
//!    `Cf-Cloudflared-Proxy-Connection-Upgrade: control-stream`; on it we
//!    are a Cap'n Proto RPC client and call
//!    `registerConnection(auth, tunnelId, connIndex 0, options)`
//!    ([`capnp`]). The answer names the location (an airport code); from
//!    then on visitors' requests arrive.
//! 4. **Requests**: every other stream is a visitor's request (the same
//!    header with `websocket` marks a WebSocket), proxied to
//!    `127.0.0.1:PORT` as HTTP/1.1, so the page's own checks
//!    (`browser::access`) apply unchanged. The response goes back as
//!    `:status` (101 becomes 200), its headers serialized into
//!    `cf-cloudflared-response-headers` (`base64(name):base64(value);...`,
//!    unpadded), then the body; a WebSocket's frames then flow as DATA
//!    both ways.
//!
//! The hostname reaches Cloudflare's DNS 0.5-10 s after registration, and
//! a resolver asked before then caches the NXDOMAIN for minutes, so
//! `start` returns only once Cloudflare's name server answers for it. If
//! the connection drops we dial again with the same credentials, so the
//! hostname stays. `Drop` unregisters and closes.
//!
//! The protocol is cloudflared's HTTP/2 transport (its fallback from
//! QUIC) as of cloudflared 2026.11.1, undocumented beyond its source.
//!
//! Sources: cloudflared (Apache-2.0, github.com/cloudflare/cloudflared)
//! 2026.11.1: `cmd/cloudflared/tunnel/quick_tunnel.go`,
//! `connection/{http2,header,control,protocol}.go`,
//! `client/config.go`, `proxy/proxy.go`, `tunnelrpc/registration_client.go`,
//! `tunnelrpc/pogs/registration_server.go`,
//! `edgediscovery/allregions/discovery.go`, `tlsconfig/cloudflare_ca.go`.

pub mod capnp;
pub mod h2;
pub mod hpack;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConnection, RootCertStore, StreamOwned};

use self::capnp::Returned;
use self::h2::Stream;
use self::hpack::Header;
use crate::codec::json::Value;
use crate::error::{Error, Result};
use crate::net::base64;
use crate::net::http::{Conn, Request, Response, Version};
use crate::net::{dns, tunnel};

const API_HOST: &str = "api.trycloudflare.com";
const EDGE_HOSTS: [&str; 2] = ["region1.v2.argotunnel.com", "region2.v2.argotunnel.com"];
const EDGE_PORT: u16 = 7844;
const EDGE_SNI: &str = "h2.cftunnel.com";
const EDGE_CA: &str = include_str!("edge_ca.pem");
const UPGRADE: &str = "cf-cloudflared-proxy-connection-upgrade";
/// One of trycloudflare.com's name servers.
const NAME_SERVER: &str = "kevin.ns.cloudflare.com";
const TIMEOUT: Duration = Duration::from_secs(15);
/// How long `start` waits for the first registration.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// Largest request body we pass on (the pages take none this large).
const MAX_BODY: usize = 1 << 20;

/// One instantiation of `spawn` for all of the tunnel's threads.
fn spawn(name: &str, f: Box<dyn FnOnce() + Send>) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .map_err(|e| Error::io("tunnel: spawn", e))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// TLS

/// Cloudflare's origin CA, which signs the edge's certificate.
fn edge_roots() -> Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for block in EDGE_CA.split("-----BEGIN CERTIFICATE-----").skip(1) {
        let b64: String = block
            .split("-----END")
            .next()
            .unwrap_or("")
            .split_whitespace()
            .collect();
        roots
            .add(CertificateDer::from(base64::decode(&b64)?))
            .map_err(|e| Error::crypto(format!("tunnel: root: {e}")))?;
    }
    Ok(roots)
}

fn tls_config(roots: RootCertStore, http: bool) -> Result<Arc<rustls::ClientConfig>> {
    let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| Error::crypto(format!("tunnel: TLS: {e}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    if http {
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    }
    Ok(Arc::new(cfg))
}

/// TCP to `host:port`, then a TLS handshake for `sni`.
fn dial(
    host: &str,
    port: u16,
    sni: &str,
    roots: RootCertStore,
) -> Result<(ClientConnection, TcpStream)> {
    let mut last = None;
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| Error::io(format!("tunnel: resolve {host}"), e))?;
    let mut tcp = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, TIMEOUT) {
            Ok(t) => {
                tcp = Some(t);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let mut tcp = tcp.ok_or_else(|| match last {
        Some(e) => Error::io(format!("tunnel: connect {host}"), e),
        None => Error::not_found(format!("tunnel: no address for {host}")),
    })?;
    let _ = tcp.set_nodelay(true);
    tcp.set_read_timeout(Some(TIMEOUT))
        .and_then(|()| tcp.set_write_timeout(Some(TIMEOUT)))
        .map_err(|e| Error::io("tunnel: socket", e))?;
    let name = ServerName::try_from(sni.to_owned())
        .map_err(|_| Error::usage(format!("tunnel: bad host {sni}")))?;
    let mut conn = ClientConnection::new(tls_config(roots, port == 443)?, name)
        .map_err(|e| Error::crypto(format!("tunnel: TLS: {e}")))?;
    while conn.is_handshaking() {
        conn.complete_io(&mut tcp)
            .map_err(|e| Error::io(format!("tunnel: TLS handshake with {host}"), e))?;
    }
    Ok((conn, tcp))
}

/// The TLS connection to the edge, split: one thread reads (blocking on
/// the socket without the lock), any thread writes.
struct Tls {
    conn: ClientConnection,
    sock: TcpStream,
}

struct TlsReader {
    tls: Arc<Mutex<Tls>>,
    sock: TcpStream,
    /// Records read that rustls did not take yet.
    pending: Vec<u8>,
}

impl Read for TlsReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        loop {
            {
                let mut tls = lock(&self.tls);
                match tls.conn.reader().read(out) {
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    r => return r,
                }
                if !self.pending.is_empty() {
                    let n = tls.conn.read_tls(&mut &self.pending[..])?;
                    self.pending.drain(..n);
                    let state = tls.conn.process_new_packets();
                    let Tls { conn, sock } = &mut *tls;
                    while conn.wants_write() {
                        conn.write_tls(sock)?;
                    }
                    state.map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                    continue;
                }
            }
            let mut buf = [0u8; 16 * 1024];
            let n = self.sock.read(&mut buf)?;
            if n == 0 {
                return Ok(0);
            }
            self.pending.extend_from_slice(&buf[..n]);
        }
    }
}

struct TlsWriter(Arc<Mutex<Tls>>);

impl Write for TlsWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let mut tls = lock(&self.0);
        tls.conn.writer().write_all(data)?;
        let Tls { conn, sock } = &mut *tls;
        while conn.wants_write() {
            conn.write_tls(sock)?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Credentials

struct Credentials {
    hostname: String,
    tunnel_id: [u8; 16],
    account_tag: String,
    secret: Vec<u8>,
}

/// Whether `host` is `LABEL.trycloudflare.com` (it becomes an allowed
/// `Host`, so it is checked).
fn quick_host(host: &str) -> bool {
    host.strip_suffix(".trycloudflare.com").is_some_and(|l| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && l != "api"
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

fn parse_credentials(body: &str) -> Result<Credentials> {
    let v = Value::parse(body)?;
    let r = v
        .get("result")
        .filter(|_| v.get("success").and_then(Value::as_bool) == Some(true));
    let s = |k: &str| r.and_then(|r| r.get(k)).and_then(Value::as_str);
    let id = crate::util::hex::decode(&s("id").unwrap_or("").replace('-', ""))?;
    let (Ok(tunnel_id), Some(host), Some(tag), Some(secret)) = (
        <[u8; 16]>::try_from(id),
        s("hostname"),
        s("account_tag"),
        s("secret"),
    ) else {
        let why = v
            .get("errors")
            .and_then(|e| e.at(0))
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("unexpected answer");
        return Err(Error::protocol(format!("tunnel: trycloudflare.com: {why}")));
    };
    if !quick_host(host) {
        return Err(Error::protocol(format!("tunnel: odd hostname {host:?}")));
    }
    Ok(Credentials {
        hostname: host.to_owned(),
        tunnel_id,
        account_tag: tag.to_owned(),
        secret: base64::decode(secret)?,
    })
}

/// Asks trycloudflare.com for a new quick tunnel (HTTP/1.0: the answer
/// is not chunked and ends with the connection).
fn provision() -> Result<Credentials> {
    let (conn, tcp) = dial(API_HOST, 443, API_HOST, super::roots::store())?;
    let mut tls = StreamOwned::new(conn, tcp);
    let req = format!(
        "POST /tunnel HTTP/1.0\r\nHost: {API_HOST}\r\nUser-Agent: tvpipe/{}\r\n\
         Content-Type: application/json\r\nContent-Length: 0\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    );
    let io = |e| Error::io("tunnel: trycloudflare.com", e);
    tls.write_all(req.as_bytes()).map_err(io)?;
    let mut resp = Vec::new();
    match (&mut tls).take(1 << 16).read_to_end(&mut resp) {
        // Some servers close without close_notify.
        Err(e) if !(e.kind() == std::io::ErrorKind::UnexpectedEof && !resp.is_empty()) => {
            return Err(io(e))
        }
        _ => {}
    }
    let text = String::from_utf8_lossy(&resp);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.lines().next().unwrap_or("");
    let ok = status.split(' ').nth(1).is_some_and(|c| c.starts_with('2'));
    if !ok && !body.starts_with('{') {
        return Err(Error::protocol(format!(
            "tunnel: trycloudflare.com answered {status:?}"
        )));
    }
    parse_credentials(body)
}

// ---------------------------------------------------------------------------
// Proxying a visitor's request

const HOP_BY_HOP: [&str; 7] = [
    "connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
    "te",
    "host",
    "proxy-connection",
];

/// `name:value;...` with each part base64 (unpadded): how the edge wants
/// an origin's response headers (`SerializeHeaders`).
fn serialize_headers<'a>(h: impl Iterator<Item = (&'a str, &'a str)>) -> String {
    let b64 = |s: &str| {
        base64::encode(s.as_bytes())
            .trim_end_matches('=')
            .to_owned()
    };
    h.map(|(n, v)| format!("{}:{}", b64(n), b64(v)))
        .collect::<Vec<_>>()
        .join(";")
}

/// Answers with a bare status (`src: cloudflared`: our own error).
fn fail(s: &Stream, status: &str) {
    let _ = s.send_headers(
        &[
            (":status", status),
            ("cf-cloudflared-response-meta", r#"{"src":"cloudflared"}"#),
        ],
        true,
    );
}

/// Sends the request to the page's server and returns its response and
/// the connection (a WebSocket continues on it).
fn forward<'a>(
    inner: &'a Inner,
    s: &Stream,
    headers: &[Header],
    ws: bool,
) -> Result<(Response, Conn<TcpStream>, Visitor<'a>)> {
    let get = |n: &str| {
        headers
            .iter()
            .find(|h| h.0 == n)
            .map_or("", |h| h.1.as_str())
    };
    let mut req = Request::new(get(":method"), get(":path"), Version::Http11);
    req.headers.add("Host", get(":authority"));
    for (n, v) in headers {
        if !n.starts_with(':')
            && !n.starts_with("cf-cloudflared-")
            && !HOP_BY_HOP.contains(&n.as_str())
        {
            req.headers.add(n.as_str(), v.as_str());
        }
    }
    if ws {
        req.headers
            .set("Connection", "Upgrade")
            .set("Upgrade", "websocket")
            .set("Sec-WebSocket-Version", "13");
    } else {
        req.headers.set("Connection", "close");
        while let Some(d) = s.recv() {
            req.body.extend(d);
            if req.body.len() > MAX_BODY {
                return Err(Error::unsupported("tunnel: request body too large"));
            }
        }
        if !req.body.is_empty() || ["POST", "PUT"].contains(&req.method.as_str()) {
            req.headers
                .set("Content-Length", req.body.len().to_string());
        }
    }
    let tcp = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, inner.port))
        .map_err(|e| Error::io("tunnel: connect to the page's server", e))?;
    let _ = tcp.set_nodelay(true);
    let visitor = Visitor(inner, tcp.local_addr().ok());
    if let Some(local) = visitor.1 {
        lock(&inner.peers).insert(local, get("cf-connecting-ip").to_owned());
    }
    let _ = tcp.set_read_timeout(Some(TIMEOUT));
    let mut c = Conn::new(tcp);
    c.send_request(&req)?;
    loop {
        let resp = c.read_response(req.method == "HEAD")?;
        if resp.status >= 200 || resp.status == 101 {
            return Ok((resp, c, visitor));
        }
    }
}

/// One visitor's request, start to end.
fn proxy(inner: &Inner, s: Arc<Stream>, headers: Vec<Header>) {
    let ws = headers
        .iter()
        .any(|(n, v)| n == UPGRADE && v == "websocket");
    let (resp, c, _visitor) = match forward(inner, &s, &headers, ws) {
        Ok(r) => r,
        Err(e) => {
            debug!("tunnel: stream {}: {e}", s.id);
            return fail(&s, "502");
        }
    };
    let upgraded = ws && resp.status == 101;
    let user = serialize_headers(
        resp.headers
            .iter()
            .filter(|(n, _)| upgraded || !HOP_BY_HOP.contains(&n.to_ascii_lowercase().as_str())),
    );
    let status = if upgraded { 200 } else { resp.status }.to_string();
    let mut h = vec![
        (":status", status.as_str()),
        ("cf-cloudflared-response-headers", user.as_str()),
        ("cf-cloudflared-response-meta", r#"{"src":"origin"}"#),
    ];
    let length = resp.body.len().to_string();
    if !upgraded {
        h.push((
            "content-length",
            resp.headers.get("content-length").unwrap_or(&length),
        ));
    }
    let end = !upgraded && resp.body.is_empty();
    if s.send_headers(&h, end).is_err() || end {
        return;
    }
    if !upgraded {
        let _ = s.send_data(&resp.body, true);
        return;
    }
    // A WebSocket: bytes both ways until either side ends.
    let (tcp, rest) = c.into_parts();
    let _ = tcp.set_read_timeout(None);
    let Ok(mut up) = tcp.try_clone() else {
        return s.reset();
    };
    let s2 = s.clone();
    let pump = spawn(
        "tunnel-ws",
        Box::new(move || {
            let mut buf = vec![0u8; 16 * 1024];
            let mut data = rest;
            loop {
                if !data.is_empty() && s2.send_data(&data, false).is_err() {
                    break;
                }
                match up.read(&mut buf) {
                    Ok(0) => {
                        let _ = s2.send_data(&[], true);
                        break;
                    }
                    Ok(n) => data = buf[..n].to_vec(),
                    Err(_) => break,
                }
            }
            let _ = up.shutdown(Shutdown::Both);
        }),
    );
    if pump.is_err() {
        return s.reset();
    }
    let mut down = &tcp;
    while let Some(d) = s.recv() {
        if down.write_all(&d).is_err() {
            break;
        }
    }
    let _ = tcp.shutdown(Shutdown::Write);
}

/// Who is behind a connection to the page's server, while it lasts.
struct Visitor<'a>(&'a Inner, Option<SocketAddr>);

impl Drop for Visitor<'_> {
    fn drop(&mut self) {
        if let Some(a) = self.1 {
            lock(&self.0.peers).remove(&a);
        }
    }
}

// ---------------------------------------------------------------------------
// The connection to the edge

#[derive(Default)]
struct State {
    /// The connection now up: its HTTP/2 side, socket and control stream.
    live: Option<(Arc<h2::Conn>, TcpStream)>,
    control: Option<Arc<Stream>>,
    unregistered: bool,
    /// What became of the latest registration (or connection attempt).
    registered: Option<Result<String>>,
}

struct Inner {
    port: u16,
    creds: Credentials,
    client_id: [u8; 16],
    stop: AtomicBool,
    state: Mutex<State>,
    /// Visitors (`cf-connecting-ip`) by our local address towards the
    /// page's server, while their requests last.
    peers: Mutex<HashMap<SocketAddr, String>>,
    changed: Condvar,
}

fn report(inner: &Inner, r: Result<String>) {
    lock(&inner.state).registered = Some(r);
    inner.changed.notify_all();
}

/// The control stream: register, then wait (for the answer to
/// `unregisterConnection` at the end). Reports the registration.
fn control(inner: &Inner, s: &Arc<Stream>, attempt: u8) -> Result<()> {
    lock(&inner.state).control = Some(s.clone());
    s.send_headers(&[(":status", "200")], false)?;
    let version = concat!("tvpipe/", env!("CARGO_PKG_VERSION"));
    let arch = format!("{}_{}", std::env::consts::OS, std::env::consts::ARCH);
    let reg = capnp::Registration {
        account_tag: &inner.creds.account_tag,
        secret: &inner.creds.secret,
        tunnel_id: &inner.creds.tunnel_id,
        client_id: &inner.client_id,
        conn_index: 0,
        previous_attempts: attempt,
        version,
        arch: &arch,
    };
    let call = capnp::call(
        1,
        0,
        capnp::REGISTRATION_SERVER,
        capnp::REGISTER_CONNECTION,
        |b, at| capnp::register_params(&reg, b, at),
    );
    s.send_data(&[capnp::bootstrap(0), call].concat(), false)?;
    let mut buf = Vec::new();
    while let Some(d) = s.recv() {
        buf.extend(d);
        while let Some((m, n)) = capnp::Message::read(&buf)? {
            buf.drain(..n);
            match capnp::parse(&m)? {
                (_, Some((1, ret))) => {
                    let r = match ret {
                        Returned::Results(c) => capnp::registered(c)?,
                        Returned::Exception(e) => Err((e, true)),
                        Returned::Other(w) => Err((format!("return {w}"), true)),
                    };
                    let _ = s.send_data(&capnp::finish(1), false);
                    let r = r.map_err(|(why, retry)| {
                        Error::protocol(format!(
                            "tunnel: the edge refused the connection: {why}{}",
                            if retry { "" } else { " (permanent)" }
                        ))
                    });
                    let failed = r.is_err();
                    report(inner, r);
                    if failed {
                        return Ok(());
                    }
                }
                (_, Some((2, _))) => {
                    lock(&inner.state).unregistered = true;
                    inner.changed.notify_all();
                }
                (1, None) => return Err(Error::closed("tunnel: the edge aborted the RPC session")),
                (which, _) => trace!("tunnel: ignoring RPC message {which}"),
            }
        }
    }
    Ok(())
}

/// Runs each stream the edge opens on a thread of its own.
fn dispatch(inner: &Arc<Inner>, attempt: u8) -> impl FnMut(Arc<Stream>, Vec<Header>) {
    let inner = inner.clone();
    move |s, headers| {
        let upgrade = headers.iter().find(|h| h.0 == UPGRADE).map(|h| h.1.clone());
        let tcp_proxy = headers.iter().any(|h| h.0 == "cf-cloudflared-proxy-src");
        let inner = inner.clone();
        let spawned = spawn(
            "tunnel-stream",
            Box::new(move || match upgrade.as_deref() {
                Some("control-stream") => {
                    if let Err(e) = control(&inner, &s, attempt) {
                        debug!("tunnel: control stream: {e}");
                    }
                    // Without it the connection is useless.
                    if let Some((_, sock)) = &lock(&inner.state).live {
                        let _ = sock.shutdown(Shutdown::Both);
                    }
                }
                None | Some("websocket") if !tcp_proxy => proxy(&inner, s, headers),
                // Configuration pushes (remotely managed tunnels) and TCP
                // streams (WARP routing) are not for us.
                _ => fail(&s, "502"),
            }),
        );
        if let Err(e) = spawned {
            debug!("tunnel: spawn: {e}");
        }
    }
}

/// One connection to the edge, until it ends.
fn connect(inner: &Arc<Inner>, attempt: u8) -> Result<()> {
    let host = EDGE_HOSTS[usize::from(attempt) % 2];
    let (conn, sock) = dial(host, EDGE_PORT, EDGE_SNI, edge_roots()?)?;
    debug!("tunnel: connected to {host}");
    // Reads wake up every second (keepalive, stop); writes may block a while.
    sock.set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|e| Error::io("tunnel: socket", e))?;
    let clone = || sock.try_clone().map_err(|e| Error::io("tunnel: socket", e));
    let (read_sock, kill) = (clone()?, clone()?);
    let tls = Arc::new(Mutex::new(Tls { conn, sock }));
    let h2 = h2::Conn::new(Box::new(TlsWriter(tls.clone())))?;
    {
        let mut st = lock(&inner.state);
        st.live = Some((h2.clone(), kill));
        (st.control, st.unregistered) = (None, false);
    }
    let reader = TlsReader {
        tls,
        sock: read_sock,
        pending: Vec::new(),
    };
    let r = h2::serve(&h2, reader, &inner.stop, dispatch(inner, attempt));
    if let Some((_, sock)) = lock(&inner.state).live.take() {
        let _ = sock.shutdown(Shutdown::Both);
    }
    r
}

/// Keeps a connection up until stopped, dialing again when it drops.
fn supervise(inner: Arc<Inner>) {
    let mut attempt: u8 = 0;
    while !inner.stop.load(Ordering::SeqCst) {
        let since = Instant::now();
        let r = connect(&inner, attempt);
        if inner.stop.load(Ordering::SeqCst) {
            break;
        }
        let e = r
            .err()
            .unwrap_or_else(|| Error::closed("tunnel: connection ended"));
        warn!("{e}; connecting again");
        report(&inner, Err(e));
        attempt = if since.elapsed() > Duration::from_secs(60) {
            0
        } else {
            attempt.saturating_add(1)
        };
        // 0.2 s, 0.4 s, ... up to 25.6 s.
        let wait = Duration::from_millis(100u64 << attempt.min(8));
        let until = Instant::now() + wait;
        while Instant::now() < until && !inner.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Waits until trycloudflare.com's name server has an address for
/// `host` (plain DNS over UDP, no recursion: nothing is cached anywhere).
fn wait_for_dns(host: &str, deadline: Instant, stop: &dyn Fn() -> bool) -> Result<()> {
    let ns = (NAME_SERVER, 53)
        .to_socket_addrs()
        .map_err(|e| Error::io("tunnel: resolve the name server", e))?
        .find(SocketAddr::is_ipv4)
        .ok_or_else(|| Error::not_found("tunnel: no name server address"))?;
    let name = dns::Name::parse(host)?;
    let mut buf = [0u8; 1500];
    while !stop() && Instant::now() < deadline {
        // A new socket (source port) each time: Cloudflare's servers each
        // remember a negative answer for about 5 s, and the port seems to
        // pick the server.
        let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| Error::io("tunnel: udp", e))?;
        sock.set_read_timeout(Some(Duration::from_millis(300)))
            .map_err(|e| Error::io("tunnel: udp", e))?;
        let id = u16::from_le_bytes(crate::crypto::ring::random_array()?);
        let q = dns::Message::query(id, vec![dns::Question::new(name.clone(), dns::TYPE_A)]);
        sock.send_to(&q.to_bytes(false)?, ns)
            .map_err(|e| Error::io("tunnel: dns query", e))?;
        if let Ok((n, from)) = sock.recv_from(&mut buf) {
            let found = dns::Message::parse(&buf[..n])
                .is_ok_and(|m| m.id == id && m.answers.iter().any(|r| r.rtype == dns::TYPE_A));
            if found && from == ns {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(Error::timeout(format!("tunnel: {host} is not in DNS yet")))
}

/// A running quick tunnel; dropping it closes it.
pub struct Tunnel {
    inner: Arc<Inner>,
    thread: Option<JoinHandle<()>>,
}

impl Tunnel {
    /// Gets a quick tunnel and serves `127.0.0.1:port` on it; returns once
    /// the edge has registered the connection and the name is in DNS.
    /// `stop` aborts the wait.
    pub fn start(port: u16, stop: &dyn Fn() -> bool) -> Result<Tunnel> {
        let creds = provision()?;
        debug!("tunnel: got {}", creds.hostname);
        let inner = Arc::new(Inner {
            port,
            creds,
            client_id: crate::crypto::ring::random_array()?,
            stop: AtomicBool::new(false),
            state: Mutex::new(State::default()),
            peers: Mutex::new(HashMap::new()),
            changed: Condvar::new(),
        });
        let i = inner.clone();
        let thread = spawn("tunnel", Box::new(move || supervise(i)))?;
        let tunnel = Tunnel {
            inner,
            thread: Some(thread),
        };
        let deadline = Instant::now() + START_TIMEOUT;
        let location = {
            let mut st = lock(&tunnel.inner.state);
            loop {
                if stop() {
                    return Err(Error::closed("tunnel: stopped"));
                }
                if Instant::now() > deadline {
                    return Err(Error::timeout("tunnel: the edge did not register us"));
                }
                match st.registered.take() {
                    Some(r) => break r?,
                    None => {
                        let wait = Duration::from_millis(100);
                        st = match tunnel.inner.changed.wait_timeout(st, wait) {
                            Ok(r) => r.0,
                            Err(e) => e.into_inner().0,
                        };
                    }
                }
            }
        };
        info!("tunnel: connected to Cloudflare ({location})");
        if let Err(e) = wait_for_dns(tunnel.host(), deadline, stop) {
            debug!("tunnel: {e}");
        }
        if stop() {
            return Err(Error::closed("tunnel: stopped"));
        }
        Ok(tunnel)
    }

    /// The public hostname (`RANDOM.trycloudflare.com`).
    pub fn host(&self) -> &str {
        &self.inner.creds.hostname
    }

    /// Drops the edge connection (it reconnects), for tests.
    #[cfg(test)]
    fn drop_connection(&self) {
        if let Some((_, sock)) = &lock(&self.inner.state).live {
            let _ = sock.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        let mut state = lock(&self.inner.state);
        if let Some(c) = state.control.clone() {
            // unregisterConnection, waiting a moment for its answer.
            let call = capnp::call(
                2,
                0,
                capnp::REGISTRATION_SERVER,
                capnp::UNREGISTER_CONNECTION,
                |_, _| {},
            );
            if c.send_data(&call, false).is_ok() {
                let deadline = Instant::now() + Duration::from_secs(1);
                while !state.unregistered && Instant::now() < deadline {
                    let left = deadline.saturating_duration_since(Instant::now());
                    state = self
                        .inner
                        .changed
                        .wait_timeout(state, left)
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                }
            }
        }
        if let Some((conn, sock)) = &state.live {
            conn.close();
            let _ = sock.shutdown(Shutdown::Both);
        }
        state.control = None;
        drop(state);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl tunnel::Tunnel for Tunnel {
    fn host(&self) -> &str {
        Tunnel::host(self)
    }

    fn name(&self) -> &'static str {
        "Cloudflare quick tunnel"
    }

    fn peer_lookup(&self) -> Option<tunnel::PeerLookup> {
        let inner = self.inner.clone();
        Some(Arc::new(move |local| {
            lock(&inner.peers).get(&local).cloned()
        }))
    }
}

#[cfg(test)]
pub(crate) mod tests;
