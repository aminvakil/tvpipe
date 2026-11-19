//! The OpenTunnel client behind `--tunnel` (docs/tunnels.md): a public
//! `https://ROUTE.ID.opentunnel.xyz` for a loopback port, with TLS
//! terminated here, so the relay never sees the page, its token or the
//! stream.
//!
//! - **Identity** (once per machine): `POST /api/tunnel` gives a tunnel
//!   (`ID.opentunnel.xyz`) and a bearer token; we make a P-256 key and a
//!   PKCS#10 request for `ID.opentunnel.xyz` and `*.ID.opentunnel.xyz`,
//!   bind it, and poll until the certificate is issued (ACME at the
//!   service, 10-30 s). Kept in the config directory (`opentunnel/`, the
//!   OpenTunnel CLI's layout, files 0600): `tunnel.json`, `token`,
//!   `private-key.pem`, `certificate.pem`, `chain.pem`. The service renews
//!   the certificate for the same key; we fetch it on every (re)connect at
//!   most every 12 hours, and new visitors get the new one.
//! - **Bridge**: one WebSocket (`opentunnel` subprotocol) to
//!   `wss://opentunnel.xyz/api/tunnel/ID/connect`; `attach` claims a route,
//!   a random 16-hex label per run (unguessable: the wildcard certificate
//!   does not list it). Each visitor's TCP connection arrives as `open`
//!   plus binary frames (`u32be conn | payload`, at most 32 KiB) starting
//!   with its TLS ClientHello; a rustls server with our certificate
//!   decrypts it and the plaintext goes to and from `127.0.0.1:PORT`.
//! - **Reconnect**: when the bridge drops, goes silent or the service
//!   asks (`drain`, on deploys), the client attaches again with the same
//!   route, so the URL stays the same; backoff from 250 ms to 30 s with
//!   jitter, reset by a successful attach (protocol.md, Reconnect).
//! - **Back-pressure**: the reader never waits on the bridge. One writer
//!   thread sends control messages first, then data round-robin over the
//!   connections, each with its own queue ([`CONN_QUEUE`]); a full queue
//!   stops reading that connection's local socket only. The wire has no
//!   per-connection flow control (the relay stops reading the whole bridge
//!   when one visitor's 512 KiB fill up), so the player also limits each
//!   viewer's unacknowledged bytes (`browser::player`).
//! - **Trust**: `opentunnel.xyz` (behind Cloudflare, whose certificates
//!   come from several public CAs) is checked against Mozilla's root
//!   list (`roots/mozilla.pem`, `scripts/update-roots.sh`), plus the
//!   certificates in `$SSL_CERT_FILE` (a company proxy). Visitors check
//!   our certificate themselves; the relay cannot read their traffic.
//!
//! Sources: OpenTunnel (MIT) `docs/protocol.md`, `spec/vectors`;
//! RFC 2986 (PKCS#10), RFC 5280 (names, subjectAltName), RFC 5480
//! (EC keys), RFC 5758 (ecdsa-with-SHA256).

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConnection, ServerConnection, StreamOwned};

use crate::codec::json::Value;
use crate::crypto::ring::EcdsaP256;
use crate::error::{Error, ErrorKind, Result};
use crate::net::base64;
use crate::net::http::{Conn, Request, Version};
use crate::net::tunnel::PeerLookup;
use crate::net::websocket::{self, Message, Role};

/// The hosted service.
pub const DEFAULT_API: &str = "https://opentunnel.xyz";
/// Overrides the service (a self-hosted one: `https://HOST[:PORT]`;
/// tunnels are then `ID.HOST`).
pub const API_ENV: &str = "TVPIPE_OPENTUNNEL_API";
/// Overrides the identity directory (default: `opentunnel/` in the config
/// directory).
pub const DIR_ENV: &str = "TVPIPE_OPENTUNNEL_DIR";
/// Mozilla's root list (`net::roots`).
use super::roots;
const TIMEOUT: Duration = Duration::from_secs(15);
const ISSUE_TIMEOUT: Duration = Duration::from_secs(180);
/// How often the reader wakes up (heartbeat, stop).
const POLL: Duration = Duration::from_millis(100);
/// Largest data payload in one frame (protocol.md, Frames).
const MAX_PAYLOAD: usize = 32 * 1024;
/// Visitor connections at once (advertised as `max_conns`).
const MAX_CONNS: u32 = 64;
/// Bytes queued for one connection before its local socket is not read.
pub const CONN_QUEUE: usize = 256 * 1024;
/// How long a write to the local server may take before the visitor is
/// reset (the reader must not hang on it).
const LOCAL_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// Reconnect backoff (protocol.md, Reconnect).
const BACKOFF_MIN: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How often a running client looks for a renewed certificate.
const CERT_CHECK: Duration = Duration::from_secs(12 * 3600);

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// PEM and DER

/// The base64 bodies of the PEM blocks labelled `label`, decoded.
fn pem_blocks(text: &str, label: &str) -> Result<Vec<Vec<u8>>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(&begin) {
        let body = &rest[i + begin.len()..];
        let j = body
            .find(&end)
            .ok_or_else(|| Error::malformed(format!("pem: unterminated {label}")))?;
        let b64: String = body[..j].split_whitespace().collect();
        out.push(base64::decode(&b64)?);
        rest = &body[j + end.len()..];
    }
    Ok(out)
}

fn pem(label: &str, der: &[u8]) -> String {
    let b64 = base64::encode(der);
    let mut s = format!("-----BEGIN {label}-----\n");
    for line in b64.as_bytes().chunks(64) {
        s.push_str(&String::from_utf8_lossy(line));
        s.push('\n');
    }
    s.push_str(&format!("-----END {label}-----\n"));
    s
}

/// One DER TLV.
fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = content.len();
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes = n.to_be_bytes();
        let skip = bytes.iter().take_while(|&&b| b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
    out.extend_from_slice(content);
    out
}

fn seq(parts: &[&[u8]]) -> Vec<u8> {
    der(0x30, &parts.concat())
}

const OID_CN: &[u8] = &[0x06, 0x03, 0x55, 0x04, 0x03];
const OID_EC_PUBLIC_KEY: &[u8] = &[0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_P256: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const OID_EXTENSION_REQUEST: &[u8] = &[
    0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x0e,
];
const OID_SUBJECT_ALT_NAME: &[u8] = &[0x06, 0x03, 0x55, 0x1d, 0x11];
const OID_ECDSA_SHA256: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];

/// The `CertificationRequestInfo` for `hostname` and `*.hostname` with
/// an uncompressed P-256 public key (RFC 2986 §4.1).
fn csr_info(hostname: &str, public_key: &[u8]) -> Vec<u8> {
    let version = der(0x02, &[0]);
    let cn = seq(&[OID_CN, &der(0x0c, hostname.as_bytes())]);
    let subject = seq(&[&der(0x31, &cn)]);
    let mut bits = vec![0u8];
    bits.extend_from_slice(public_key);
    let spki = seq(&[&seq(&[OID_EC_PUBLIC_KEY, OID_P256]), &der(0x03, &bits)]);
    let names = seq(&[
        &der(0x82, hostname.as_bytes()),
        &der(0x82, format!("*.{hostname}").as_bytes()),
    ]);
    let san = seq(&[OID_SUBJECT_ALT_NAME, &der(0x04, &names)]);
    let extensions = seq(&[&san]);
    let attribute = seq(&[OID_EXTENSION_REQUEST, &der(0x31, &extensions)]);
    let attributes = der(0xa0, &attribute);
    seq(&[&version, &subject, &spki, &attributes])
}

/// A signed PKCS#10 request (DER).
fn csr(hostname: &str, key: &EcdsaP256) -> Result<Vec<u8>> {
    let info = csr_info(hostname, &key.public_key());
    let sig = key.sign(&info)?;
    let mut bits = vec![0u8];
    bits.extend_from_slice(&sig);
    Ok(seq(&[&info, &seq(&[OID_ECDSA_SHA256]), &der(0x03, &bits)]))
}

// ---------------------------------------------------------------------------
// The service

/// Where the service is and how to reach it.
#[derive(Debug, Clone)]
pub struct Service {
    pub host: String,
    pub port: u16,
    /// HTTPS and WSS (always, except a loopback test service).
    pub tls: bool,
    /// Tunnels are `ID.DOMAIN`.
    pub domain: String,
    /// Extra trust anchors (DER): `$SSL_CERT_FILE`, tests.
    pub extra_roots: Vec<Vec<u8>>,
}

impl Service {
    /// The hosted service, or `$TVPIPE_OPENTUNNEL_API`.
    pub fn from_env() -> Result<Service> {
        let api = std::env::var(API_ENV)
            .ok()
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| DEFAULT_API.into());
        let mut s = Service::parse(&api)?;
        if let Some(f) = std::env::var_os("SSL_CERT_FILE").filter(|f| !f.is_empty()) {
            match std::fs::read_to_string(&f) {
                Ok(text) => s.extra_roots = pem_blocks(&text, "CERTIFICATE")?,
                Err(e) => warn!("opentunnel: SSL_CERT_FILE: {e}"),
            }
        }
        Ok(s)
    }

    /// `https://HOST[:PORT]` (or `http://` for a loopback host).
    pub fn parse(api: &str) -> Result<Service> {
        let bad = || {
            Error::usage(format!(
                "{API_ENV}: expected https://HOST[:PORT], got {api:?}"
            ))
        };
        let (tls, rest) = if let Some(r) = api.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = api.strip_prefix("http://") {
            (false, r)
        } else {
            return Err(bad());
        };
        let rest = rest.trim_end_matches('/');
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().map_err(|_| bad())?),
            None => (rest, if tls { 443 } else { 80 }),
        };
        let host_ok = !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
        if !host_ok || rest.contains('/') {
            return Err(bad());
        }
        if !tls && !(host == "localhost" || host == "127.0.0.1") {
            return Err(Error::usage(format!(
                "{API_ENV}: plain http only for localhost"
            )));
        }
        Ok(Service {
            host: host.to_ascii_lowercase(),
            port,
            tls,
            domain: host.to_ascii_lowercase(),
            extra_roots: Vec::new(),
        })
    }

    fn authority(&self) -> String {
        if self.port == if self.tls { 443 } else { 80 } {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    fn client_config(&self) -> Result<Arc<rustls::ClientConfig>> {
        let mut roots = roots::store();
        roots.add_parsable_certificates(self.extra_roots.iter().cloned().map(CertificateDer::from));
        let mut cfg = rustls::ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::crypto(format!("opentunnel: TLS: {e}")))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(cfg))
    }

    /// A connection to the service (TLS handshake done).
    fn connect(&self) -> Result<Link> {
        let addr = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|e| Error::io(format!("opentunnel: resolve {}", self.host), e))?
            .next()
            .ok_or_else(|| Error::not_found(format!("opentunnel: no address for {}", self.host)))?;
        let mut tcp = TcpStream::connect_timeout(&addr, TIMEOUT)
            .map_err(|e| Error::io(format!("opentunnel: connect {}", self.host), e))?;
        let _ = tcp.set_nodelay(true);
        tcp.set_read_timeout(Some(TIMEOUT))
            .and_then(|()| tcp.set_write_timeout(Some(TIMEOUT)))
            .map_err(|e| Error::io("opentunnel: socket", e))?;
        if !self.tls {
            return Ok(Link::Plain(tcp));
        }
        let name = ServerName::try_from(self.host.clone())
            .map_err(|_| Error::usage(format!("opentunnel: bad host {}", self.host)))?;
        let mut conn = ClientConnection::new(self.client_config()?, name)
            .map_err(|e| Error::crypto(format!("opentunnel: TLS: {e}")))?;
        while conn.is_handshaking() {
            conn.complete_io(&mut tcp).map_err(|e| {
                Error::io(format!("opentunnel: TLS handshake with {}", self.host), e)
            })?;
        }
        Ok(Link::Tls(Box::new(StreamOwned::new(conn, tcp))))
    }

    /// One API call (`Connection: close`); the JSON body of a 2xx.
    fn api(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Result<Value> {
        let mut req = Request::new(method, path, Version::Http11)
            .header("Host", self.authority())
            .header("Accept", "application/json")
            .header("User-Agent", concat!("tvpipe/", env!("CARGO_PKG_VERSION")))
            .header("Connection", "close");
        if let Some(t) = token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        if let Some(b) = body {
            req = req.body("application/json", b.to_json().into_bytes());
        }
        let mut conn = Conn::new(self.connect()?);
        let resp = conn.request(&req)?;
        let text = String::from_utf8_lossy(&resp.body).into_owned();
        if !resp.is_success() {
            let msg = Value::parse(&text)
                .ok()
                .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_else(|| text.chars().take(200).collect());
            return Err(Error::protocol(format!(
                "opentunnel: {method} {path}: {} {msg}",
                resp.status
            )));
        }
        Value::parse(&text)
    }
}

/// A connection to the service.
enum Link {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Read for Link {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Link::Plain(s) => s.read(buf),
            Link::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Link {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Link::Plain(s) => s.write(buf),
            Link::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Link::Plain(s) => s.flush(),
            Link::Tls(s) => s.flush(),
        }
    }
}

type BoxRead = Box<dyn Read + Send>;
type BoxWrite = Box<dyn Write + Send>;

impl Link {
    /// A reader and a writer usable from two threads, and the socket.
    fn split(self) -> Result<(BoxRead, BoxWrite, TcpStream)> {
        let clone = |s: &TcpStream| {
            s.try_clone()
                .map_err(|e| Error::io("opentunnel: clone socket", e))
        };
        match self {
            Link::Plain(s) => Ok((Box::new(clone(&s)?), Box::new(clone(&s)?), s)),
            Link::Tls(s) => {
                let StreamOwned { conn, sock } = *s;
                let raw = clone(&sock)?;
                let read_sock = clone(&sock)?;
                let tls = Arc::new(Mutex::new(Tls { conn, sock }));
                Ok((
                    Box::new(TlsReader {
                        tls: tls.clone(),
                        sock: read_sock,
                        buf: vec![0u8; 32 * 1024],
                        pending: Vec::new(),
                    }),
                    Box::new(TlsWriter(tls)),
                    raw,
                ))
            }
        }
    }
}

/// The TLS connection to the service, split: one thread reads (blocking
/// on the socket without the lock), any thread writes.
struct Tls {
    conn: ClientConnection,
    sock: TcpStream,
}

struct TlsReader {
    tls: Arc<Mutex<Tls>>,
    sock: TcpStream,
    buf: Vec<u8>,
    /// Records read from the socket that rustls did not take yet (it
    /// takes no more while its plaintext buffer is full).
    pending: Vec<u8>,
}

impl Read for TlsReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        loop {
            {
                let mut tls = lock(&self.tls);
                match tls.conn.reader().read(out) {
                    Ok(n) => return Ok(n),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
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
            let n = self.sock.read(&mut self.buf)?;
            if n == 0 {
                return Ok(0);
            }
            self.pending.extend_from_slice(&self.buf[..n]);
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
// Identity

struct Identity {
    id: String,
    hostname: String,
    token: String,
    key_pkcs8: Vec<u8>,
    /// Leaf first.
    chain: Vec<Vec<u8>>,
}

/// The identity directory.
pub fn identity_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os(DIR_ENV).filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d));
    }
    Ok(crate::airplay::credentials::config_dir()?.join("opentunnel"))
}

fn read(dir: &Path, name: &str) -> Result<String> {
    std::fs::read_to_string(dir.join(name))
        .map_err(|e| Error::io(format!("opentunnel: read {}", dir.join(name).display()), e))
}

fn write(dir: &Path, name: &str, data: &str) -> Result<()> {
    crate::airplay::credentials::write_private(&dir.join(name), data.as_bytes())
}

/// Whether a tunnel ID and hostname are what the service makes (they end
/// up in file names and URLs).
fn valid_tunnel(id: &str, hostname: &str, domain: &str) -> bool {
    !id.is_empty()
        && id.len() <= 63
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && hostname.strip_prefix(id).and_then(|h| h.strip_prefix('.')) == Some(domain)
}

fn load(dir: &Path, service: &Service) -> Result<Identity> {
    let info = Value::parse(&read(dir, "tunnel.json")?)?;
    let id = info.require_str("id")?.to_owned();
    let hostname = info.require_str("hostname")?.to_owned();
    if !valid_tunnel(&id, &hostname, &service.domain) {
        return Err(Error::malformed(format!(
            "opentunnel: {}: not a tunnel of {}",
            dir.join("tunnel.json").display(),
            service.domain
        )));
    }
    let key = pem_blocks(&read(dir, "private-key.pem")?, "PRIVATE KEY")?
        .into_iter()
        .next()
        .ok_or_else(|| Error::malformed("opentunnel: no PKCS#8 key"))?;
    let mut chain = pem_blocks(&read(dir, "certificate.pem")?, "CERTIFICATE")?;
    chain.extend(pem_blocks(&read(dir, "chain.pem")?, "CERTIFICATE")?);
    if chain.is_empty() {
        return Err(Error::malformed("opentunnel: no certificate"));
    }
    Ok(Identity {
        id,
        hostname,
        token: read(dir, "token")?.trim().to_owned(),
        key_pkcs8: key,
        chain,
    })
}

fn tunnel_json(id: &str, hostname: &str, expiry: Option<&str>) -> String {
    Value::object()
        .with("id", id)
        .with("hostname", hostname)
        .with_opt("certificateExpiry", expiry)
        .to_json()
}

/// Saves a `ready` certificate state (if it changed); `Ok(None)` while
/// it is being issued, else whether the files changed.
fn save_certificate(dir: &Path, id: &str, hostname: &str, cert: &Value) -> Result<Option<bool>> {
    let state = cert.require("state")?;
    match state.require_str("type")? {
        "ready" => {
            let leaf = state.require_str("certificate")?;
            let chain = state.require_str("chain")?;
            let old = read(dir, "certificate.pem").unwrap_or_default();
            let changed = old.trim() != leaf.trim();
            if changed {
                write(dir, "certificate.pem", leaf)?;
                write(dir, "chain.pem", chain)?;
            }
            let expiry = state.get("expiry").and_then(Value::as_str);
            write(dir, "tunnel.json", &tunnel_json(id, hostname, expiry))?;
            Ok(Some(changed))
        }
        "failed" => Err(Error::protocol(format!(
            "opentunnel: certificate failed: {}",
            state.get("reason").and_then(Value::as_str).unwrap_or("?")
        ))),
        _ => Ok(None),
    }
}

/// Creates a tunnel and waits for its certificate (protocol.md,
/// Provisioning). Resumes a provision that was interrupted.
fn provision(service: &Service, dir: &Path, stop: &dyn Fn() -> bool) -> Result<Identity> {
    let (id, hostname, token, key_pkcs8) = match load_pending(dir, service) {
        Some(p) => p,
        None => {
            info!("tunnel: creating this machine's OpenTunnel tunnel (once)");
            let created = service.api("POST", "/api/tunnel", None, Some(&Value::object()))?;
            let tunnel = created.require("tunnel")?;
            let id = tunnel.require_str("id")?.to_owned();
            let hostname = tunnel.require_str("hostname")?.to_owned();
            let token = created.require_str("token")?.to_owned();
            if !valid_tunnel(&id, &hostname, &service.domain) {
                return Err(Error::protocol(format!(
                    "opentunnel: unexpected tunnel {id} {hostname}"
                )));
            }
            let key_pkcs8 = EcdsaP256::generate_pkcs8()?;
            write(dir, "token", &format!("{token}\n"))?;
            write(dir, "private-key.pem", &pem("PRIVATE KEY", &key_pkcs8))?;
            write(dir, "tunnel.json", &tunnel_json(&id, &hostname, None))?;
            (id, hostname, token, key_pkcs8)
        }
    };
    let key = EcdsaP256::from_pkcs8(&key_pkcs8)?;
    let request = pem("CERTIFICATE REQUEST", &csr(&hostname, &key)?);
    let path = format!("/api/tunnel/{id}/certificate");
    let body = Value::object().with("csr", request.as_str());
    let mut cert = service.api("POST", &path, Some(&token), Some(&body))?;
    info!("tunnel: waiting for {hostname}'s certificate (10-30 s, only this once)");
    let deadline = Instant::now() + ISSUE_TIMEOUT;
    while save_certificate(dir, &id, &hostname, &cert)?.is_none() {
        if Instant::now() > deadline {
            return Err(Error::timeout(
                "opentunnel: no certificate in time; try again later",
            ));
        }
        if stop() {
            return Err(Error::closed(
                "opentunnel: stopped while waiting for the certificate",
            ));
        }
        std::thread::sleep(Duration::from_secs(2));
        cert = service.api("GET", &path, Some(&token), None)?;
    }
    load(dir, service)
}

/// A tunnel created but without a certificate yet.
fn load_pending(dir: &Path, service: &Service) -> Option<(String, String, String, Vec<u8>)> {
    let info = Value::parse(&read(dir, "tunnel.json").ok()?).ok()?;
    let id = info.get("id")?.as_str()?.to_owned();
    let hostname = info.get("hostname")?.as_str()?.to_owned();
    if !valid_tunnel(&id, &hostname, &service.domain) {
        return None;
    }
    let token = read(dir, "token").ok()?.trim().to_owned();
    let key = pem_blocks(&read(dir, "private-key.pem").ok()?, "PRIVATE KEY")
        .ok()?
        .into_iter()
        .next()?;
    Some((id, hostname, token, key))
}

/// The saved identity, or a new one.
fn identity(service: &Service, dir: &Path, stop: &dyn Fn() -> bool) -> Result<Identity> {
    if dir.join("certificate.pem").is_file() {
        load(dir, service)
    } else {
        provision(service, dir, stop)
    }
}

/// Fetches the service's current certificate; whether it changed.
fn refresh_certificate(service: &Service, dir: &Path, id: &Identity) -> Result<bool> {
    let path = format!("/api/tunnel/{}/certificate", id.id);
    let cert = service.api("GET", &path, Some(&id.token), None)?;
    Ok(save_certificate(dir, &id.id, &id.hostname, &cert)?.unwrap_or(false))
}

fn server_config(id: &Identity) -> Result<Arc<rustls::ServerConfig>> {
    let chain: Vec<CertificateDer<'static>> =
        id.chain.iter().cloned().map(CertificateDer::from).collect();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(id.key_pkcs8.clone()));
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::crypto(format!("opentunnel: TLS: {e}")))?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|e| Error::crypto(format!("opentunnel: certificate: {e}")))?;
    // tvpipe's pages speak HTTP/1.1 only.
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(cfg))
}

// ---------------------------------------------------------------------------
// The bridge's outgoing side

fn control(kind: &str, conn: u32) -> String {
    Value::object()
        .with("type", kind)
        .with("conn", conn as u64)
        .to_json()
}

fn reset(conn: u32, code: &str) -> String {
    Value::object()
        .with("type", "reset")
        .with("conn", conn as u64)
        .with("code", code)
        .to_json()
}

fn data_frame(conn: u32, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(4 + payload.len());
    f.extend_from_slice(&conn.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

#[derive(Default)]
struct ConnOut {
    frames: VecDeque<Vec<u8>>,
    bytes: usize,
    /// Send `end` once the frames are out.
    end: bool,
}

#[derive(Default)]
struct OutState {
    control: VecDeque<String>,
    conns: HashMap<u32, ConnOut>,
    /// Connections with something to send, round-robin.
    ready: VecDeque<u32>,
    closed: bool,
}

/// What the writer thread sends: control messages first, then one frame
/// per connection in turn.
#[derive(Default)]
struct Outbox {
    state: Mutex<OutState>,
    cv: Condvar,
}

enum Item {
    Text(String),
    Data(u32, Vec<u8>),
}

impl Outbox {
    fn control(&self, text: String) {
        let mut st = lock(&self.state);
        if !st.closed {
            st.control.push_back(text);
            self.cv.notify_all();
        }
    }

    /// Queues `data` for `conn`; with `wait`, first waits while that
    /// connection's queue is full. `false`: the bridge or the connection
    /// is gone.
    fn data(&self, conn: u32, data: &[u8], wait: bool) -> bool {
        let mut st = lock(&self.state);
        loop {
            if st.closed {
                return false;
            }
            let full = st.conns.get(&conn).is_some_and(|c| c.bytes >= CONN_QUEUE);
            if !wait || !full {
                break;
            }
            st = self
                .cv
                .wait_timeout(st, POLL)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        let c = st.conns.entry(conn).or_default();
        if c.end {
            return false;
        }
        for chunk in data.chunks(MAX_PAYLOAD) {
            c.bytes += chunk.len();
            c.frames.push_back(data_frame(conn, chunk));
        }
        if !st.ready.contains(&conn) {
            st.ready.push_back(conn);
        }
        self.cv.notify_all();
        true
    }

    /// `end` after what is queued for `conn`.
    fn end(&self, conn: u32) {
        let mut st = lock(&self.state);
        st.conns.entry(conn).or_default().end = true;
        if !st.ready.contains(&conn) {
            st.ready.push_back(conn);
        }
        self.cv.notify_all();
    }

    /// Drops what is queued for `conn` (it was reset).
    fn forget(&self, conn: u32) {
        let mut st = lock(&self.state);
        st.conns.remove(&conn);
        st.ready.retain(|c| *c != conn);
        self.cv.notify_all();
    }

    fn close(&self) {
        lock(&self.state).closed = true;
        self.cv.notify_all();
    }

    fn is_closed(&self) -> bool {
        lock(&self.state).closed
    }

    fn next(&self) -> Option<Item> {
        let mut st = lock(&self.state);
        loop {
            if st.closed {
                return None;
            }
            if let Some(t) = st.control.pop_front() {
                return Some(Item::Text(t));
            }
            while let Some(conn) = st.ready.pop_front() {
                let Some(c) = st.conns.get_mut(&conn) else {
                    continue;
                };
                if let Some(f) = c.frames.pop_front() {
                    c.bytes -= f.len() - 4;
                    if !c.frames.is_empty() || c.end {
                        st.ready.push_back(conn);
                    }
                    self.cv.notify_all();
                    return Some(Item::Data(conn, f));
                }
                if c.end {
                    st.conns.remove(&conn);
                    return Some(Item::Text(control("end", conn)));
                }
            }
            st = self.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
    }
}

// ---------------------------------------------------------------------------
// Visitors

/// One visitor: its TLS session (we are the server) and the local
/// connection its plaintext goes to.
struct Visitor {
    id: u32,
    tls: Mutex<ServerConnection>,
    local: TcpStream,
    out: Arc<Outbox>,
    closed: AtomicBool,
}

impl Visitor {
    /// What rustls has for the visitor.
    fn tls_out(tls: &mut ServerConnection) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        while tls.wants_write() {
            tls.write_tls(&mut out)
                .map_err(|e| Error::io("opentunnel: TLS out", e))?;
        }
        Ok(out)
    }

    /// Bytes from the visitor (TLS records), on the reader thread: the
    /// TLS answers are queued without waiting, the plaintext goes to the
    /// local server (which may take [`LOCAL_WRITE_TIMEOUT`]).
    fn feed(&self, mut data: &[u8]) -> Result<()> {
        let mut plain = Vec::new();
        let mut eof = false;
        {
            let mut tls = lock(&self.tls);
            while !data.is_empty() {
                tls.read_tls(&mut data)
                    .map_err(|e| Error::io("opentunnel: TLS in", e))?;
                let state = tls.process_new_packets();
                let out = Visitor::tls_out(&mut tls)?;
                if !out.is_empty() {
                    self.out.data(self.id, &out, false);
                }
                let state =
                    state.map_err(|e| Error::crypto(format!("opentunnel: visitor TLS: {e}")))?;
                let n = state.plaintext_bytes_to_read();
                if n > 0 {
                    let start = plain.len();
                    plain.resize(start + n, 0);
                    tls.reader()
                        .read_exact(&mut plain[start..])
                        .map_err(|e| Error::io("opentunnel: TLS plaintext", e))?;
                }
                eof |= state.peer_has_closed();
            }
        }
        (&self.local)
            .write_all(&plain)
            .map_err(|e| Error::io("opentunnel: local write", e))?;
        if eof {
            let _ = self.local.shutdown(Shutdown::Write);
        }
        Ok(())
    }

    /// Copies the local side to the visitor until it ends (its own
    /// thread; waits while the connection's queue is full).
    fn pump(&self) {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = match (&self.local).read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let out = {
                let mut tls = lock(&self.tls);
                if tls.writer().write_all(&buf[..n]).is_err() {
                    break;
                }
                match Visitor::tls_out(&mut tls) {
                    Ok(o) => o,
                    Err(_) => break,
                }
            };
            if !self.out.data(self.id, &out, true) {
                break;
            }
        }
        if !self.closed.load(Ordering::SeqCst) {
            let out = {
                let mut tls = lock(&self.tls);
                tls.send_close_notify();
                Visitor::tls_out(&mut tls).unwrap_or_default()
            };
            self.out.data(self.id, &out, false);
            self.out.end(self.id);
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let _ = self.local.shutdown(Shutdown::Both);
    }
}

/// What every bridge session of one tunnel shares.
struct Ctx {
    service: Service,
    dir: PathBuf,
    port: u16,
    route: String,
    identity: Mutex<Identity>,
    server_config: Mutex<Arc<rustls::ServerConfig>>,
    cert_checked: Mutex<Instant>,
    /// Local address of our connection to the page server → visitor.
    peers: Arc<Mutex<HashMap<SocketAddr, String>>>,
    stop: AtomicBool,
    attaches: AtomicU64,
}

/// One bridge connection's visitors.
struct Session {
    ctx: Arc<Ctx>,
    out: Arc<Outbox>,
    visitors: Mutex<HashMap<u32, Arc<Visitor>>>,
}

impl Session {
    fn open(self: &Arc<Self>, id: u32, peer: &str) -> Result<()> {
        if lock(&self.visitors).len() >= MAX_CONNS as usize {
            self.out.control(reset(id, "too_many_connections"));
            return Ok(());
        }
        let config = lock(&self.ctx.server_config).clone();
        let mut tls = ServerConnection::new(config)
            .map_err(|e| Error::crypto(format!("opentunnel: TLS: {e}")))?;
        tls.set_buffer_limit(None);
        let local = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, self.ctx.port))
            .map_err(|e| Error::io("opentunnel: connect to the page's server", e))?;
        let _ = local.set_nodelay(true);
        let _ = local.set_write_timeout(Some(LOCAL_WRITE_TIMEOUT));
        let local_addr = local.local_addr().ok();
        if let Some(a) = local_addr {
            lock(&self.ctx.peers).insert(a, peer.to_owned());
        }
        let v = Arc::new(Visitor {
            id,
            tls: Mutex::new(tls),
            local,
            out: self.out.clone(),
            closed: AtomicBool::new(false),
        });
        lock(&self.visitors).insert(id, v.clone());
        let me = self.clone();
        std::thread::Builder::new()
            .name("tunnel-conn".into())
            .spawn(move || {
                v.pump();
                lock(&me.visitors).remove(&v.id);
                if let Some(a) = local_addr {
                    lock(&me.ctx.peers).remove(&a);
                }
            })
            .map_err(|e| Error::io("opentunnel: spawn", e))?;
        Ok(())
    }

    fn visitor(&self, id: u32) -> Option<Arc<Visitor>> {
        lock(&self.visitors).get(&id).cloned()
    }

    fn close_all(&self) {
        for v in lock(&self.visitors).values() {
            v.close();
        }
    }
}

/// How a bridge session ended.
enum Ended {
    Stopped,
    /// Reconnect now (`drain`).
    Drain,
    /// Reconnect after a backoff.
    Lost(Error),
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The bridge's reader, writer, socket and the attach's heartbeat and
/// idle timeout.
struct Attached {
    rx: websocket::Receiver<BoxRead, BoxWrite>,
    tx: websocket::Sender<BoxWrite>,
    sock: TcpStream,
    heartbeat: Duration,
    idle: Duration,
}

/// Opens the bridge and claims the route. `Err` with kind `Auth` or
/// `Malformed`: retrying will not help.
fn attach(ctx: &Ctx) -> Result<Attached> {
    let (id, token) = {
        let i = lock(&ctx.identity);
        (i.id.clone(), i.token.clone())
    };
    let mut link = ctx.service.connect()?;
    // The handshake runs inside TLS; `Host` is the bare authority (port
    // 80 is what `WsUrl::authority` leaves out).
    let url = websocket::WsUrl {
        host: ctx.service.authority(),
        port: 80,
        path: format!("/api/tunnel/{id}/connect"),
    };
    let (_, rest) =
        websocket::client_handshake(&mut link, &url, &[("Sec-WebSocket-Protocol", "opentunnel")])?;
    let (r, w, sock) = link.split()?;
    sock.set_read_timeout(Some(POLL))
        .map_err(|e| Error::io("opentunnel: socket", e))?;
    let tx = websocket::Sender::new(w, Role::Client);
    let mut rx = websocket::Receiver::new(
        r,
        rest,
        Role::Client,
        websocket::Limits {
            max_message: 64 * 1024,
        },
        tx.clone(),
    );
    let attach = Value::object()
        .with("type", "attach")
        .with("token", token.as_str())
        .with("transport", "ws")
        .with("routes", Value::array([ctx.route.as_str()]))
        .with(
            "client",
            Value::object()
                .with("version", concat!("tvpipe-", env!("CARGO_PKG_VERSION")))
                .with("max_conns", MAX_CONNS as u64),
        );
    tx.send_text(&attach.to_json())?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let msg = match rx.read_message() {
            Err(e) if e.kind() == ErrorKind::Timeout && Instant::now() < deadline => continue,
            r => r?,
        };
        let Message::Text(t) = msg else { continue };
        let v = Value::parse(&t)?;
        let ms =
            |k: &str, d: u64| Duration::from_millis(v.get(k).and_then(Value::as_u64).unwrap_or(d));
        match v.get("type").and_then(Value::as_str) {
            Some("attached") => {
                return Ok(Attached {
                    rx,
                    tx,
                    sock,
                    heartbeat: ms("heartbeat_ms", 15_000).max(Duration::from_millis(100)),
                    idle: ms("idle_timeout_ms", 45_000).max(Duration::from_millis(300)),
                })
            }
            Some("attach_error") => {
                let code = v.get("code").and_then(Value::as_str).unwrap_or("?");
                let msg = format!("opentunnel: attach refused: {code}");
                return Err(match code {
                    "bad_token" | "cert_not_ready" => Error::auth(msg),
                    _ => Error::protocol(msg),
                });
            }
            _ => {}
        }
    }
}

/// Runs one attached bridge until it ends.
fn run(ctx: &Arc<Ctx>, a: Attached) -> Ended {
    let Attached {
        mut rx,
        tx,
        sock,
        heartbeat,
        idle,
    } = a;
    let session = Arc::new(Session {
        ctx: ctx.clone(),
        out: Arc::new(Outbox::default()),
        visitors: Mutex::new(HashMap::new()),
    });
    // Debug builds: `TVPIPE_TUNNEL_DROP_AFTER=SECS` cuts the first bridge
    // after SECS, to watch a reconnect against the real service.
    #[cfg(debug_assertions)]
    if let Some(secs) = std::env::var("TVPIPE_TUNNEL_DROP_AFTER")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|_| ctx.attaches.load(Ordering::SeqCst) == 1)
    {
        if let Ok(s) = sock.try_clone() {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs_f64(secs));
                info!("tunnel: TVPIPE_TUNNEL_DROP_AFTER: cutting the bridge");
                let _ = s.shutdown(Shutdown::Both);
            });
        }
    }
    let out = session.out.clone();
    let writer_tx = tx.clone();
    let writer = std::thread::Builder::new()
        .name("tunnel-writer".into())
        .spawn(move || {
            while let Some(item) = out.next() {
                let r = match item {
                    Item::Text(t) => writer_tx.send_text(&t),
                    Item::Data(_, f) => writer_tx.send_binary(&f),
                };
                if let Err(e) = r {
                    debug!("opentunnel: bridge write: {e}");
                    out.close();
                }
            }
        });
    let end = match &writer {
        Ok(_) => read_loop(ctx, &session, &mut rx, heartbeat, idle),
        Err(e) => Ended::Lost(Error::closed(format!("opentunnel: spawn: {e}"))),
    };
    session.out.close();
    session.close_all();
    let _ = sock.shutdown(Shutdown::Both);
    if let Ok(w) = writer {
        let _ = w.join();
    }
    end
}

fn read_loop(
    ctx: &Arc<Ctx>,
    session: &Arc<Session>,
    rx: &mut websocket::Receiver<BoxRead, BoxWrite>,
    heartbeat: Duration,
    idle: Duration,
) -> Ended {
    let out = &session.out;
    let mut last_ping = Instant::now();
    let mut last_rx = Instant::now();
    loop {
        if ctx.stop.load(Ordering::SeqCst) {
            return Ended::Stopped;
        }
        if out.is_closed() {
            return Ended::Lost(Error::closed("opentunnel: the bridge write failed"));
        }
        if last_ping.elapsed() >= heartbeat {
            last_ping = Instant::now();
            out.control(
                Value::object()
                    .with("type", "ping")
                    .with("time_sent", now_ms())
                    .to_json(),
            );
        }
        if last_rx.elapsed() >= idle {
            return Ended::Lost(Error::timeout("opentunnel: the relay went silent"));
        }
        let msg = match rx.read_message() {
            Err(e) if e.kind() == ErrorKind::Timeout => continue,
            Err(e) => return Ended::Lost(e),
            Ok(m) => m,
        };
        last_rx = Instant::now();
        match msg {
            Message::Binary(b) => {
                let Some((head, payload)) = b.split_first_chunk::<4>() else {
                    return Ended::Lost(Error::malformed("opentunnel: short data frame"));
                };
                let id = u32::from_be_bytes(*head);
                if let Some(v) = session.visitor(id) {
                    if let Err(e) = v.feed(payload) {
                        debug!("opentunnel: conn {id}: {e}");
                        v.close();
                        out.forget(id);
                        out.control(reset(id, "upstream_io_error"));
                    }
                }
            }
            Message::Text(t) => {
                let Ok(v) = Value::parse(&t) else {
                    debug!("opentunnel: bad control message");
                    continue;
                };
                let conn = v
                    .get("conn")
                    .and_then(Value::as_u64)
                    .and_then(|c| u32::try_from(c).ok());
                match (v.get("type").and_then(Value::as_str), conn) {
                    (Some("open"), Some(id)) => {
                        let peer = v.get("peer").and_then(Value::as_str).unwrap_or("");
                        trace!("opentunnel: conn {id} from {peer}");
                        if let Err(e) = session.open(id, peer) {
                            debug!("opentunnel: conn {id}: {e}");
                            out.control(reset(id, "upstream_io_error"));
                        }
                    }
                    (Some("end"), Some(id)) => {
                        if let Some(v) = session.visitor(id) {
                            let _ = v.local.shutdown(Shutdown::Write);
                        }
                    }
                    (Some("reset"), Some(id)) => {
                        if let Some(v) = session.visitor(id) {
                            v.close();
                        }
                        out.forget(id);
                    }
                    (Some("ping"), _) => {
                        let t = v.get("time_sent").and_then(Value::as_u64).unwrap_or(0);
                        out.control(
                            Value::object()
                                .with("type", "pong")
                                .with("time_sent", t)
                                .to_json(),
                        );
                    }
                    (Some("drain"), _) => return Ended::Drain,
                    _ => {}
                }
            }
            Message::Close(_) => {
                return Ended::Lost(Error::closed("opentunnel: the relay closed the bridge"))
            }
        }
    }
}

/// Keeps the bridge up until stopped: reconnects with backoff.
fn supervise(ctx: Arc<Ctx>, first: Attached) {
    let mut next = Some(first);
    let mut failures = 0u32;
    let mut lost_at = Instant::now();
    loop {
        let ended = match next.take() {
            Some(a) => {
                failures = 0;
                run(&ctx, a)
            }
            None => Ended::Lost(Error::closed("not attached")),
        };
        if failures == 0 {
            lost_at = Instant::now();
        }
        let wait = match ended {
            Ended::Stopped => return,
            Ended::Drain => {
                info!("tunnel: the relay is restarting; reconnecting");
                Duration::ZERO
            }
            Ended::Lost(e) => {
                if failures == 0 {
                    warn!("tunnel: {e}; reconnecting");
                } else {
                    debug!("tunnel: {e}");
                }
                backoff(failures)
            }
        };
        failures = failures.saturating_add(1);
        if !sleep_unless_stopped(&ctx, wait) {
            return;
        }
        check_certificate(&ctx);
        match attach(&ctx) {
            Ok(a) => {
                ctx.attaches.fetch_add(1, Ordering::SeqCst);
                info!(
                    "tunnel: reconnected after {} ms (same URL)",
                    lost_at.elapsed().as_millis()
                );
                next = Some(a);
            }
            Err(e) if matches!(e.kind(), ErrorKind::Auth) => {
                error!("tunnel: {e}; the public link is down");
                return;
            }
            Err(e) => debug!("tunnel: reconnect: {e}"),
        }
    }
}

/// Backoff for the `n`th failure in a row: 250 ms doubling to 30 s, a
/// random 50-100 % of it.
fn backoff(n: u32) -> Duration {
    let full = BACKOFF_MIN
        .saturating_mul(1u32 << n.min(16))
        .min(BACKOFF_MAX);
    let r = crate::crypto::ring::random_array::<1>().map_or(255, |b| b[0]);
    full.mul_f64(0.5 + 0.5 * f64::from(r) / 255.0)
}

fn sleep_unless_stopped(ctx: &Ctx, d: Duration) -> bool {
    let until = Instant::now() + d;
    while Instant::now() < until {
        if ctx.stop.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(POLL.min(until.saturating_duration_since(Instant::now())));
    }
    !ctx.stop.load(Ordering::SeqCst)
}

/// Picks up a renewed certificate (at most every [`CERT_CHECK`]).
fn check_certificate(ctx: &Ctx) {
    {
        let mut at = lock(&ctx.cert_checked);
        if at.elapsed() < CERT_CHECK {
            return;
        }
        *at = Instant::now();
    }
    let changed = {
        let id = lock(&ctx.identity);
        refresh_certificate(&ctx.service, &ctx.dir, &id)
    };
    match changed {
        Ok(true) => match load(&ctx.dir, &ctx.service).and_then(|i| Ok((server_config(&i)?, i))) {
            Ok((cfg, id)) => {
                info!("tunnel: using the renewed certificate");
                *lock(&ctx.server_config) = cfg;
                *lock(&ctx.identity) = id;
            }
            Err(e) => warn!("tunnel: renewed certificate: {e}"),
        },
        Ok(false) => {}
        Err(e) => debug!("tunnel: certificate check: {e}"),
    }
}

/// A running tunnel; dropping it closes the bridge.
pub struct OpenTunnel {
    host: String,
    ctx: Arc<Ctx>,
    thread: Option<JoinHandle<()>>,
}

impl OpenTunnel {
    /// Serves `127.0.0.1:port` on a new public route of the hosted (or
    /// `$TVPIPE_OPENTUNNEL_API`) service.
    pub fn start(port: u16, stop: &dyn Fn() -> bool) -> Result<OpenTunnel> {
        OpenTunnel::start_with(Service::from_env()?, &identity_dir()?, port, stop)
    }

    /// [`start`](Self::start) with the service and identity directory
    /// given.
    pub fn start_with(
        service: Service,
        dir: &Path,
        port: u16,
        stop: &dyn Fn() -> bool,
    ) -> Result<OpenTunnel> {
        let id = identity(&service, dir, stop)?;
        let route = crate::util::hex::encode(&crate::crypto::ring::random_array::<8>()?);
        let host = format!("{route}.{}", id.hostname);
        let ctx = Arc::new(Ctx {
            server_config: Mutex::new(server_config(&id)?),
            service,
            dir: dir.to_owned(),
            port,
            route,
            identity: Mutex::new(id),
            // Checked at the first reconnect after a while.
            cert_checked: Mutex::new(Instant::now()),
            peers: Arc::new(Mutex::new(HashMap::new())),
            stop: AtomicBool::new(false),
            attaches: AtomicU64::new(1),
        });
        let first = attach(&ctx)?;
        let c = ctx.clone();
        let thread = std::thread::Builder::new()
            .name("tunnel".into())
            .spawn(move || supervise(c, first))
            .map_err(|e| Error::io("opentunnel: spawn", e))?;
        Ok(OpenTunnel {
            host,
            ctx,
            thread: Some(thread),
        })
    }

    /// The public hostname (`ROUTE.ID.opentunnel.xyz`).
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The visitor behind a local connection to the page's server.
    pub fn peer_lookup(&self) -> PeerLookup {
        let peers = self.ctx.peers.clone();
        Arc::new(move |a| lock(&peers).get(&a).cloned())
    }

    /// Successful attaches so far (the first and every reconnect).
    pub fn attaches(&self) -> u64 {
        self.ctx.attaches.load(Ordering::SeqCst)
    }
}

impl Drop for OpenTunnel {
    fn drop(&mut self) {
        self.ctx.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests;
