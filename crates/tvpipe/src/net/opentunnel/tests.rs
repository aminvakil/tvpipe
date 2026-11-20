//! The client against a fake relay on loopback: provisioning, visitors
//! end to end (TLS terminated by the client), the visitor's address,
//! reconnects (dropped bridge, `drain`, a silent relay) under the same
//! route, a refused token, and fairness between connections.

use std::net::TcpListener;
use std::sync::atomic::AtomicU32;

use super::*;
use crate::net::http::Response;
use crate::net::httpd::{self, Reply};
use rustls::pki_types::ServerName;
use rustls::RootCertStore;

const DOMAIN: &str = "opentunnel.test";
const ID: &str = "tvtest";
const TOKEN: &str = "tok_test";
const PEER: &str = "203.0.113.9:4000";

const CA: &str = include_str!("../testdata/opentunnel/ca.pem");
const KEY: &str = include_str!("../testdata/opentunnel/private-key.pem");
const CERT: &str = include_str!("../testdata/opentunnel/certificate.pem");
const CHAIN: &str = include_str!("../testdata/opentunnel/chain.pem");

fn never() -> bool {
    false
}

fn wait_for(what: &str, timeout: Duration, f: impl Fn() -> bool) {
    let until = Instant::now() + timeout;
    while !f() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A fresh directory under target/test-tmp.
fn temp_dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-tmp")
        .join(format!("opentunnel-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// An identity with the fixture certificate (`tvtest.opentunnel.test`).
fn seeded_dir(name: &str, token: &str) -> PathBuf {
    let d = temp_dir(name);
    std::fs::write(
        d.join("tunnel.json"),
        tunnel_json(ID, &format!("{ID}.{DOMAIN}"), None),
    )
    .unwrap();
    std::fs::write(d.join("token"), token).unwrap();
    std::fs::write(d.join("private-key.pem"), KEY).unwrap();
    std::fs::write(d.join("certificate.pem"), CERT).unwrap();
    std::fs::write(d.join("chain.pem"), CHAIN).unwrap();
    d
}

#[derive(Default)]
struct RelayState {
    bridge: Option<(websocket::Sender<TcpStream>, TcpStream)>,
    /// Write halves of the visitors' sockets.
    visitors: HashMap<u32, TcpStream>,
    attaches: Vec<Value>,
    csrs: Vec<String>,
    /// Do not answer pings (a relay gone silent).
    mute: bool,
    /// Pause between bridge frames read (a slow relay).
    throttle: Option<Duration>,
    /// What `attached` says (`idle_timeout_ms`; 0: 1000).
    idle_ms: u64,
}

/// A fake OpenTunnel service: the API and the bridge on one loopback
/// port, visitors on another.
struct Relay {
    http: httpd::Server,
    public: SocketAddr,
    state: Arc<Mutex<RelayState>>,
}

impl Relay {
    fn start() -> Relay {
        let state = Arc::new(Mutex::new(RelayState::default()));
        let st = state.clone();
        let handler: httpd::Handler = Arc::new(move |req: &Request| api(&st, req));
        let http = httpd::Server::bind("127.0.0.1:0".parse().unwrap(), handler).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let public = listener.local_addr().unwrap();
        let next_conn = Arc::new(AtomicU32::new(1));
        let (st, next) = (state.clone(), next_conn.clone());
        std::thread::spawn(move || {
            for v in listener.incoming() {
                let Ok(v) = v else { return };
                let id = next.fetch_add(1, Ordering::SeqCst);
                visitor(&st, id, v);
            }
        });
        Relay {
            http,
            public,
            state,
        }
    }

    fn service(&self) -> Service {
        Service {
            host: "127.0.0.1".into(),
            port: self.http.local_addr().port(),
            tls: false,
            domain: DOMAIN.into(),
            extra_roots: Vec::new(),
        }
    }

    fn attaches(&self) -> usize {
        lock(&self.state).attaches.len()
    }

    fn routes(&self) -> Vec<String> {
        lock(&self.state)
            .attaches
            .iter()
            .map(|a| {
                a.get("routes")
                    .unwrap()
                    .at(0)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    fn drop_bridge(&self) {
        if let Some((_, s)) = lock(&self.state).bridge.take() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }

    fn send(&self, text: &str) {
        if let Some((tx, _)) = &lock(&self.state).bridge {
            tx.send_text(text).unwrap();
        }
    }
}

fn json(status: u16, v: &Value) -> Reply {
    Reply::Response(httpd::response(
        status,
        "OK",
        "application/json",
        v.to_json().into_bytes(),
    ))
}

fn api(st: &Arc<Mutex<RelayState>>, req: &Request) -> Reply {
    let cert_path = format!("/api/tunnel/{ID}/certificate");
    let authorized = req.headers.get("authorization") == Some(&format!("Bearer {TOKEN}"));
    match (req.method.as_str(), req.target.as_str()) {
        ("POST", "/api/tunnel") => json(
            201,
            &Value::object()
                .with(
                    "tunnel",
                    Value::object()
                        .with("id", ID)
                        .with("hostname", format!("{ID}.{DOMAIN}"))
                        .with("state", "offline"),
                )
                .with("token", TOKEN),
        ),
        ("POST", p) if p == cert_path && authorized => {
            let body = Value::parse(&String::from_utf8_lossy(&req.body)).unwrap();
            lock(st)
                .csrs
                .push(body.require_str("csr").unwrap().to_owned());
            json(
                202,
                &Value::object()
                    .with("id", "c1")
                    .with("state", Value::object().with("type", "issuing")),
            )
        }
        ("GET", p) if p == cert_path && authorized => json(
            200,
            &Value::object().with("id", "c1").with(
                "state",
                Value::object()
                    .with("type", "ready")
                    .with("certificate", CERT)
                    .with("chain", CHAIN)
                    .with("expiry", "2126-01-01T00:00:00.000Z"),
            ),
        ),
        ("GET", p) if p == format!("/api/tunnel/{ID}/connect") => {
            let protocol = req.headers.get("sec-websocket-protocol").unwrap_or("");
            assert_eq!(protocol, "opentunnel");
            let mut resp = websocket::server_response(req).unwrap();
            resp.headers.add("Sec-WebSocket-Protocol", "opentunnel");
            let st = st.clone();
            Reply::Upgrade(resp, Box::new(move |tcp, rest| bridge(&st, tcp, rest)))
        }
        _ => Reply::Response(httpd::error_response(404, "Not Found")),
    }
}

/// The relay's side of one bridge.
fn bridge(st: &Arc<Mutex<RelayState>>, tcp: TcpStream, rest: Vec<u8>) {
    let raw = tcp.try_clone().unwrap();
    let (mut rx, tx) =
        websocket::pair(tcp, rest, Role::Server, websocket::Limits::default()).unwrap();
    let Ok(Message::Text(t)) = rx.read_message() else {
        return;
    };
    let attach = Value::parse(&t).unwrap();
    assert_eq!(attach.get("type").unwrap().as_str(), Some("attach"));
    if attach.get("token").unwrap().as_str() != Some(TOKEN) {
        let _ = tx.send_text(r#"{"type":"attach_error","code":"bad_token"}"#);
        let _ = tx.close(1008, "");
        return;
    }
    {
        let mut s = lock(st);
        s.attaches.push(attach);
        s.bridge = Some((tx.clone(), raw));
    }
    let idle = match lock(st).idle_ms {
        0 => 1000,
        ms => ms,
    };
    let attached = Value::object()
        .with("type", "attached")
        .with("session", "s")
        .with("routes", Value::array(Vec::<Value>::new()))
        .with("heartbeat_ms", 200u64)
        .with("idle_timeout_ms", idle);
    tx.send_text(&attached.to_json()).unwrap();
    loop {
        let msg = match rx.read_message() {
            Ok(m) => m,
            Err(_) => break,
        };
        let throttle = lock(st).throttle;
        match msg {
            Message::Binary(b) => {
                let id = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
                let w = lock(st).visitors.get(&id).and_then(|s| s.try_clone().ok());
                if let Some(mut w) = w {
                    let _ = w.write_all(&b[4..]);
                }
                if let Some(d) = throttle {
                    std::thread::sleep(d);
                }
            }
            Message::Text(t) => {
                let v = Value::parse(&t).unwrap();
                let conn = v.get("conn").and_then(Value::as_u64).map(|c| c as u32);
                match (v.get("type").and_then(Value::as_str), conn) {
                    (Some("ping"), _) if !lock(st).mute => {
                        let pong = Value::object().with("type", "pong").with(
                            "time_sent",
                            v.get("time_sent").cloned().unwrap_or(Value::Null),
                        );
                        let _ = tx.send_text(&pong.to_json());
                    }
                    (Some("end"), Some(id)) => {
                        if let Some(s) = lock(st).visitors.get(&id) {
                            let _ = s.shutdown(Shutdown::Write);
                        }
                    }
                    (Some("reset"), Some(id)) => {
                        if let Some(s) = lock(st).visitors.remove(&id) {
                            let _ = s.shutdown(Shutdown::Both);
                        }
                    }
                    _ => {}
                }
            }
            Message::Close(_) => break,
        }
    }
}

/// A visitor's TCP connection: `open`, then its bytes as frames.
fn visitor(st: &Arc<Mutex<RelayState>>, id: u32, mut sock: TcpStream) {
    let Some(tx) = lock(st).bridge.as_ref().map(|(tx, _)| tx.clone()) else {
        return;
    };
    lock(st).visitors.insert(id, sock.try_clone().unwrap());
    let open = Value::object()
        .with("type", "open")
        .with("conn", id as u64)
        .with("peer", PEER)
        .with("sni", "")
        .with("alpn", "http/1.1");
    tx.send_text(&open.to_json()).unwrap();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match sock.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send_binary(&data_frame(id, &buf[..n])).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = tx.send_text(&control("end", id));
    });
}

/// The page's server: `/` says hello and who asked, `/big` is 4 MiB.
struct Target {
    http: httpd::Server,
    /// The tunnel's peer lookup, once it runs.
    lookup: Arc<std::sync::OnceLock<PeerLookup>>,
    /// Who each request came from, by the lookup.
    peers: Arc<Mutex<Vec<Option<String>>>>,
}

fn target() -> Target {
    let peers = Arc::new(Mutex::new(Vec::new()));
    let lookup: Arc<std::sync::OnceLock<PeerLookup>> = Arc::default();
    let (p, l) = (peers.clone(), lookup.clone());
    let handler: httpd::Handler = Arc::new(move |req: &Request| {
        if let (Some(a), Some(l)) = (httpd::current_peer(), l.get()) {
            lock(&p).push(l(a));
        }
        let body = match req.target.as_str() {
            "/big" => vec![b'x'; 4 << 20],
            _ => b"hello".to_vec(),
        };
        let mut r: Response = httpd::response(200, "OK", "text/plain", body);
        r.headers.set("Connection", "close");
        Reply::Response(r)
    });
    Target {
        http: httpd::Server::bind("127.0.0.1:0".parse().unwrap(), handler).unwrap(),
        lookup,
        peers,
    }
}

/// A browser: TLS to `host` through the relay (trusting the test CA),
/// one GET; the response.
fn visit(public: SocketAddr, host: &str, path: &str) -> String {
    let mut roots = RootCertStore::empty();
    for d in pem_blocks(CA, "CERTIFICATE").unwrap() {
        roots.add(CertificateDer::from(d)).unwrap();
    }
    let cfg = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let conn = ClientConnection::new(
        Arc::new(cfg),
        ServerName::try_from(host.to_owned()).unwrap(),
    )
    .unwrap();
    let tcp = TcpStream::connect(public).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut s = StreamOwned::new(conn, tcp);
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

fn start(relay: &Relay, target: &Target, dir: &Path) -> Result<OpenTunnel> {
    OpenTunnel::start_with(
        relay.service(),
        dir,
        target.http.local_addr().port(),
        &never,
    )
}

#[test]
fn serves_visitors_end_to_end() {
    let relay = Relay::start();
    let target = target();
    let dir = seeded_dir("e2e", TOKEN);
    let t = start(&relay, &target, &dir).unwrap();
    let _ = target.lookup.set(t.peer_lookup());
    let route = &relay.routes()[0];
    assert_eq!(route.len(), 16);
    assert_eq!(t.host(), format!("{route}.{ID}.{DOMAIN}"));
    let r = visit(relay.public, t.host(), "/");
    assert!(
        r.starts_with("HTTP/1.1 200 OK") && r.ends_with("hello"),
        "{r}"
    );
    // The page's server can tell who it was.
    assert_eq!(lock(&target.peers)[0].as_deref(), Some(PEER));
    // A large response arrives whole.
    let r = visit(relay.public, t.host(), "/big");
    assert!(
        r.ends_with(&"x".repeat(1000)) && r.len() > 4 << 20,
        "{}",
        r.len()
    );
}

#[test]
fn reconnects_under_the_same_route() {
    let relay = Relay::start();
    let target = target();
    let dir = seeded_dir("reconnect", TOKEN);
    let t = start(&relay, &target, &dir).unwrap();
    // The relay drops the bridge.
    relay.drop_bridge();
    wait_for("a reconnect", Duration::from_secs(5), || {
        relay.attaches() == 2
    });
    assert!(visit(relay.public, t.host(), "/").ends_with("hello"));
    // The relay restarts (`drain`): at once.
    let asked = Instant::now();
    relay.send(r#"{"type":"drain","reason":"deploy"}"#);
    wait_for("a reconnect after drain", Duration::from_secs(5), || {
        relay.attaches() == 3
    });
    assert!(
        asked.elapsed() < Duration::from_secs(1),
        "{:?}",
        asked.elapsed()
    );
    // The relay goes silent: the client notices (idle timeout 1 s).
    lock(&relay.state).mute = true;
    wait_for("a reconnect after silence", Duration::from_secs(6), || {
        relay.attaches() == 4
    });
    lock(&relay.state).mute = false;
    assert!(visit(relay.public, t.host(), "/").ends_with("hello"));
    let routes = relay.routes();
    assert!(routes.iter().all(|r| *r == routes[0]), "{routes:?}");
    assert_eq!(t.attaches(), 4);
    drop(t);
}

#[test]
fn refused_token_fails_the_start() {
    let relay = Relay::start();
    let target = target();
    let dir = seeded_dir("token", "wrong");
    let e = start(&relay, &target, &dir).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Auth, "{e}");
}

#[test]
fn provisions_an_identity_once() {
    let relay = Relay::start();
    let dir = temp_dir("provision");
    let id = provision(&relay.service(), &dir, &never).unwrap();
    assert_eq!((id.id.as_str(), id.token.as_str()), (ID, TOKEN));
    assert_eq!(id.hostname, format!("{ID}.{DOMAIN}"));
    assert_eq!(id.chain.len(), 2);
    // The request is for the tunnel and its subdomains, with our key.
    let csrs = lock(&relay.state).csrs.clone();
    let der = &pem_blocks(&csrs[0], "CERTIFICATE REQUEST").unwrap()[0];
    let key = EcdsaP256::from_pkcs8(&id.key_pkcs8).unwrap();
    let info = csr_info(&id.hostname, &key.public_key());
    assert!(der.windows(info.len()).any(|w| w == info.as_slice()));
    let saved = Value::parse(&std::fs::read_to_string(dir.join("tunnel.json")).unwrap()).unwrap();
    assert_eq!(
        saved.get("certificateExpiry").and_then(Value::as_str),
        Some("2126-01-01T00:00:00.000Z")
    );
    #[cfg(unix)]
    for f in ["token", "private-key.pem"] {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join(f)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{f}");
    }
    // The next start uses what is saved.
    assert!(identity(&relay.service(), &dir, &never).is_ok());
    assert_eq!(lock(&relay.state).csrs.len(), 1);
}

/// A slow relay with a big download in flight: another visitor still
/// gets through (its frames take turns with the download's in the
/// client; `outbox_order_and_limits` checks the order) and the bridge
/// stays up. How long the second visitor waits depends on the socket
/// buffers between client and relay, which hold data of both alike: the
/// player's in-flight limit keeps that small (docs/tunnels.md).
#[test]
fn a_slow_relay_with_a_big_download() {
    let relay = Relay::start();
    // Pongs wait behind the backlog in the socket buffers.
    lock(&relay.state).idle_ms = 60_000;
    let target = target();
    let dir = seeded_dir("slow", TOKEN);
    let t = start(&relay, &target, &dir).unwrap();
    // The relay takes a frame every 10 ms (1-3 MB/s).
    lock(&relay.state).throttle = Some(Duration::from_millis(10));
    let host = t.host().to_owned();
    let big = {
        let (public, host) = (relay.public, host.clone());
        std::thread::spawn(move || visit(public, &host, "/big").len())
    };
    std::thread::sleep(Duration::from_millis(300));
    assert!(visit(relay.public, &host, "/").ends_with("hello"));
    assert!(big.join().unwrap() > 4 << 20);
    assert_eq!(relay.attaches(), 1);
}

#[test]
fn outbox_order_and_limits() {
    let out = Outbox::default();
    assert!(out.data(1, &[1; 10], false));
    assert!(out.data(2, &[2; 10], false));
    assert!(out.data(1, &[1; 10], false));
    out.end(1);
    out.control("c".into());
    let next = || match out.next().unwrap() {
        Item::Text(t) => t,
        Item::Data(c, f) => format!("{c}:{}", f.len()),
    };
    // Control first, then the connections in turn, `end` after the data.
    assert_eq!(next(), "c");
    assert_eq!(next(), "1:14");
    assert_eq!(next(), "2:14");
    assert_eq!(next(), "1:14");
    assert_eq!(next(), control("end", 1));
    // Payloads are split at 32 KiB.
    assert!(out.data(3, &vec![0; MAX_PAYLOAD + 1], false));
    assert_eq!(next(), format!("3:{}", MAX_PAYLOAD + 4));
    assert_eq!(next(), "3:5");
    // A full connection waits (only with `wait`); a reset drops its queue.
    assert!(out.data(4, &vec![0; CONN_QUEUE], false));
    let out = Arc::new(out);
    let o = out.clone();
    let waiter = std::thread::spawn(move || o.data(4, &[0; 1], true));
    std::thread::sleep(Duration::from_millis(300));
    assert!(!waiter.is_finished());
    assert!(out.data(5, &[0; 1], true));
    out.forget(4);
    assert!(waiter.join().unwrap());
    out.close();
    assert!(out.next().is_none());
    assert!(!out.data(6, &[0; 1], false));
}

#[test]
fn service_urls() {
    let s = Service::parse("https://opentunnel.xyz").unwrap();
    assert_eq!(
        (s.host.as_str(), s.port, s.tls),
        ("opentunnel.xyz", 443, true)
    );
    assert_eq!(s.authority(), "opentunnel.xyz");
    let s = Service::parse("https://tunnel.example.com:8443/").unwrap();
    assert_eq!(
        (s.port, s.authority().as_str()),
        (8443, "tunnel.example.com:8443")
    );
    assert!(Service::parse("http://opentunnel.xyz").is_err());
    assert!(Service::parse("http://127.0.0.1:9").is_ok());
    assert!(Service::parse("ftp://x").is_err());
    assert!(Service::parse("https://a/b").is_err());
    assert!(Service::parse("https://u@x").is_err());
}

#[test]
fn backoff_grows_to_the_cap() {
    for n in 0..20 {
        let b = backoff(n);
        let full = BACKOFF_MIN.saturating_mul(1 << n.min(16)).min(BACKOFF_MAX);
        assert!(b >= full / 2 && b <= full, "{n}: {b:?}");
    }
}

#[test]
fn der_lengths() {
    assert_eq!(der(0x04, &[1, 2]), vec![0x04, 2, 1, 2]);
    let long = der(0x04, &[0u8; 200]);
    assert_eq!(&long[..3], &[0x04, 0x81, 200]);
    let longer = der(0x04, &[0u8; 300]);
    assert_eq!(&longer[..4], &[0x04, 0x82, 0x01, 0x2c]);
}

/// The request info for a fixed key (`openssl asn1parse`: version 0,
/// CN, id-ecPublicKey/prime256v1, extensionRequest with the two
/// dNSNames; a request signed by `csr` passes `openssl req -verify`).
#[test]
fn csr_info_known_answer() {
    let mut pk = vec![0x04];
    pk.extend((1..=64).map(|i| i as u8));
    let info = csr_info("abc.opentunnel.xyz", &pk);
    let want = concat!(
        "3081c5020100301d311b301906035504030c126162632e6f70656e74756e6e65",
        "6c2e78797a3059301306072a8648ce3d020106082a8648ce3d03010703420004",
        "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
        "2122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f40",
        "a046304406092a864886f70d01090e3137303530330603551d11042c302a8212",
        "6162632e6f70656e74756e6e656c2e78797a82142a2e6162632e6f70656e7475",
        "6e6e656c2e78797a",
    );
    assert_eq!(crate::util::hex::encode(&info), want);
}

#[test]
fn signed_csr_round_trips_through_pem() {
    let pkcs8 = EcdsaP256::generate_pkcs8().unwrap();
    let key = EcdsaP256::from_pkcs8(&pkcs8).unwrap();
    let req = csr("abc.opentunnel.xyz", &key).unwrap();
    assert_eq!(req[0], 0x30);
    let text = pem("CERTIFICATE REQUEST", &req);
    assert_eq!(pem_blocks(&text, "CERTIFICATE REQUEST").unwrap(), vec![req]);
}

const ROOTS_PEM: &str = include_str!("../roots/mozilla.pem");
const ROOTS_RS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/net/roots/mozilla.rs");

/// `roots/mozilla.rs` from `roots/mozilla.pem`: the trust anchors rustls
/// takes from each certificate.
fn roots_source() -> String {
    let mut store = RootCertStore::empty();
    let ders = pem_blocks(ROOTS_PEM, "CERTIFICATE").unwrap();
    let n = ders.len();
    let (added, skipped) =
        store.add_parsable_certificates(ders.into_iter().map(CertificateDer::from));
    assert_eq!((added, skipped), (n, 0));
    let bytes = |b: &[u8]| {
        let mut s = String::from("b\"");
        for &c in b {
            if c.is_ascii_alphanumeric() || c == b' ' {
                s.push(c as char);
            } else {
                s.push_str(&format!("\\x{c:02x}"));
            }
        }
        s.push('"');
        s
    };
    let mut out = String::from(
        "// Generated from mozilla.pem by `cargo test regenerate_roots -- --ignored`\n\
         // (scripts/update-roots.sh). Do not edit. CDLA-Permissive-2.0\n\
         // (LICENSES/CDLA-Permissive-2.0-ccadb.txt).\n\
         // (subject, subjectPublicKeyInfo, nameConstraints) per root.\n\
         #[rustfmt::skip]\n\
         pub(super) static ROOTS: &[Anchor] = &[\n",
    );
    for a in &store.roots {
        out.push_str(&format!(
            "({}, {}, {}),\n",
            bytes(a.subject.as_ref()),
            bytes(a.subject_public_key_info.as_ref()),
            a.name_constraints
                .as_ref()
                .map_or("None".to_owned(), |c| format!(
                    "Some({})",
                    bytes(c.as_ref())
                ))
        ));
    }
    out.push_str("];\n");
    out
}

#[test]
fn roots_match_the_pem() {
    assert!(crate::net::roots::ROOTS.len() > 100);
    let current = std::fs::read_to_string(ROOTS_RS).unwrap();
    assert!(
        current == roots_source(),
        "src/net/roots/mozilla.rs is out of date: cargo test regenerate_roots -- --ignored"
    );
    Service::parse(DEFAULT_API)
        .unwrap()
        .client_config()
        .unwrap();
}

#[test]
#[ignore = "writes src/net/roots/mozilla.rs (scripts/update-roots.sh)"]
fn regenerate_roots() {
    std::fs::write(ROOTS_RS, roots_source()).unwrap();
}

#[test]
fn frames() {
    // spec/vectors/frames.json
    assert_eq!(data_frame(0, b""), vec![0, 0, 0, 0]);
    assert_eq!(
        crate::util::hex::encode(&data_frame(305419896, &[0x16, 0x03, 0x01, 0x00])),
        "1234567816030100"
    );
    assert_eq!(control("end", 7), r#"{"type":"end","conn":7}"#);
    assert_eq!(
        reset(4294967295, "unknown_route"),
        r#"{"type":"reset","conn":4294967295,"code":"unknown_route"}"#
    );
}

#[test]
fn tunnel_names() {
    assert!(valid_tunnel(
        "oxr3okatzvis",
        "oxr3okatzvis.opentunnel.xyz",
        "opentunnel.xyz"
    ));
    assert!(!valid_tunnel(
        "oxr3okatzvis",
        "other.opentunnel.xyz",
        "opentunnel.xyz"
    ));
    assert!(!valid_tunnel(
        "../x",
        "../x.opentunnel.xyz",
        "opentunnel.xyz"
    ));
    assert!(!valid_tunnel("", ".opentunnel.xyz", "opentunnel.xyz"));
    assert!(!valid_tunnel("abc", "abc.opentunnel.xyz", "evil.example"));
}
