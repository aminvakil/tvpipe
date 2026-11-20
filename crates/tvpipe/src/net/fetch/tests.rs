//! The fetch client against local servers (plain and TLS with the test CA
//! of `testdata/opentunnel/`): no network.

use std::io::Write;
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

use super::*;
use crate::net::http::Request;

const CA: &str = include_str!("../testdata/opentunnel/ca.pem");
const KEY: &str = include_str!("../testdata/opentunnel/private-key.pem");
const CERT: &str = include_str!("../testdata/opentunnel/certificate.pem");

/// Names the test certificate covers (`*.tvtest.opentunnel.test`).
pub(crate) const HOST_A: &str = "a.tvtest.opentunnel.test";
pub(crate) const HOST_B: &str = "b.tvtest.opentunnel.test";

/// What a test server answers: the raw response bytes for a request.
pub(crate) type Handler = Arc<dyn Fn(&Request) -> Vec<u8> + Send + Sync>;

/// A server on 127.0.0.1 (plain or TLS) answering every request with its
/// handler, recording the requests.
pub(crate) struct TestServer {
    pub addr: SocketAddr,
    pub tls: bool,
    pub requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
}

/// A response: status, headers, body (with `Content-Length` unless the
/// headers set a transfer coding).
pub(crate) fn response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status} X\r\n");
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    if !headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("transfer-encoding"))
    {
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    out.push_str("\r\n");
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}

fn server_config() -> Arc<ServerConfig> {
    let certs = tls::pem_blocks(CERT, "CERTIFICATE")
        .unwrap()
        .into_iter()
        .map(CertificateDer::from)
        .collect();
    let key = tls::pem_blocks(KEY, "PRIVATE KEY").unwrap().remove(0);
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key));
    Arc::new(
        ServerConfig::builder_with_provider(tls::provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .unwrap(),
    )
}

impl TestServer {
    pub(crate) fn plain(h: Handler) -> TestServer {
        TestServer::start(h, false)
    }

    pub(crate) fn tls(h: Handler) -> TestServer {
        TestServer::start(h, true)
    }

    fn start(h: Handler, tls: bool) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (reqs, st) = (requests.clone(), stop.clone());
        let config = tls.then(server_config);
        std::thread::spawn(move || {
            while !st.load(Ordering::SeqCst) {
                let Ok((sock, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                let _ = sock.set_nonblocking(false);
                let _ = sock.set_read_timeout(Some(Duration::from_secs(10)));
                let (h, reqs, config) = (h.clone(), reqs.clone(), config.clone());
                std::thread::spawn(move || {
                    let serve = |s: &mut dyn ReadWrite| {
                        let mut conn = Conn::new(s);
                        let Ok(req) = conn.read_request() else { return };
                        let out = h(&req);
                        reqs.lock().unwrap().push(req);
                        let s = conn.get_mut();
                        let _ = s.write_all(&out);
                        let _ = s.flush();
                    };
                    match config {
                        Some(c) => {
                            let conn = ServerConnection::new(c).unwrap();
                            let mut s = StreamOwned::new(conn, sock);
                            serve(&mut s);
                            s.conn.send_close_notify();
                            let _ = s.flush();
                        }
                        None => {
                            let mut s = sock;
                            serve(&mut s);
                        }
                    }
                });
            }
        });
        TestServer {
            addr,
            tls,
            requests,
            stop,
        }
    }

    /// The base URL for `host` (TLS: one of the certificate's names).
    pub(crate) fn base(&self, host: &str) -> String {
        if self.tls {
            format!("https://{host}:{}", self.addr.port())
        } else {
            format!("http://127.0.0.1:{}", self.addr.port())
        }
    }

    /// A client trusting the test CA, the test names going here.
    pub(crate) fn client(&self) -> Client {
        Client {
            extra_roots: tls::pem_blocks(CA, "CERTIFICATE").unwrap(),
            resolve: vec![
                (HOST_A.to_owned(), self.addr),
                (HOST_B.to_owned(), self.addr),
            ],
            ..Client::default()
        }
    }

    pub(crate) fn targets(&self) -> Vec<String> {
        let r = self.requests.lock().unwrap();
        r.iter().map(|r| r.target.clone()).collect()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// `Read + Write` as one object.
pub(crate) trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

fn get(url: &str) -> Get {
    Get::new(Url::parse(url).unwrap(), 1 << 20)
}

#[test]
fn urls() {
    let u = Url::parse("HTTPS://Example.COM/a/b?c=d#frag").unwrap();
    assert_eq!(
        (u.https, u.host.as_str(), u.port, u.target.as_str()),
        (true, "example.com", 443, "/a/b?c=d")
    );
    assert_eq!(u.to_string(), "https://example.com/a/b?c=d");
    let u = Url::parse("http://127.0.0.1:8080").unwrap();
    assert_eq!(
        (u.port, u.target.as_str(), u.is_loopback()),
        (8080, "/", true)
    );
    assert_eq!(u.authority(), "127.0.0.1:8080");
    let u = Url::parse("http://[::1]:9/x").unwrap();
    assert_eq!(
        (u.host.as_str(), u.authority()),
        ("::1", "[::1]:9".to_owned())
    );
    assert!(u.is_loopback());
    assert!(Url::parse("http://localhost/").unwrap().is_loopback());
    assert!(!Url::parse("http://example.com/").unwrap().is_loopback());
    for bad in [
        "ftp://x/",
        "https://",
        "https://user@host/",
        "https://host:0/",
        "https://host:99999/",
        "https://ho st/",
        "https://host/a b",
        "file:///etc/passwd",
        "//host/",
    ] {
        assert!(Url::parse(bad).is_err(), "{bad}");
    }
    let base = Url::parse("https://github.com/o/r/releases/latest/download/x.zip").unwrap();
    for (loc, want) in [
        ("https://other.example/a?b", "https://other.example/a?b"),
        ("//cdn.example/f", "https://cdn.example/f"),
        (
            "/o/r/releases/download/v1/x.zip",
            "https://github.com/o/r/releases/download/v1/x.zip",
        ),
        (
            "y.zip",
            "https://github.com/o/r/releases/latest/download/y.zip",
        ),
        (
            "?q=1",
            "https://github.com/o/r/releases/latest/download/x.zip?q=1",
        ),
        ("/a#frag", "https://github.com/a"),
    ] {
        assert_eq!(base.join(loc).unwrap().to_string(), want, "{loc}");
    }
    assert!(base.join("javascript://x").is_err());
}

#[test]
fn bodies_by_length_chunks_and_close() {
    let s = TestServer::plain(Arc::new(|r: &Request| {
        match r.target.as_str() {
        "/len" => response(200, &[], b"hello"),
        "/chunked" => {
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;x=y\r\nde\r\n0\r\nT: 1\r\n\r\n"
                .to_vec()
        }
        _ => b"HTTP/1.1 200 OK\r\n\r\nuntil the end".to_vec(),
    }
    }));
    let c = s.client();
    let base = s.base("");
    let mut seen = Vec::new();
    let mut body = Vec::new();
    let r = c
        .get(
            &get(&format!("{base}/len")),
            &mut |d, t| seen.push((d, t)),
            &mut |d| {
                body.extend_from_slice(d);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!((r.status, body.as_slice()), (200, &b"hello"[..]));
    assert_eq!(seen.first(), Some(&(0, Some(5))));
    assert_eq!(seen.last(), Some(&(5, Some(5))));
    let (_, b) = c.get_bytes(&get(&format!("{base}/chunked"))).unwrap();
    assert_eq!(b, b"abcde");
    let (_, b) = c.get_bytes(&get(&format!("{base}/close"))).unwrap();
    assert_eq!(b, b"until the end");
    let req = &s.requests.lock().unwrap()[0];
    assert_eq!(req.headers.get("connection"), Some("close"));
    assert_eq!(req.headers.get("accept-encoding"), Some("identity"));
    assert!(req
        .headers
        .get("user-agent")
        .unwrap()
        .starts_with("tvpipe/"));
    assert_eq!(
        req.headers.get("host"),
        Some(format!("127.0.0.1:{}", s.addr.port()).as_str())
    );
}

#[test]
fn redirects_are_followed_and_bounded() {
    let s = TestServer::plain(Arc::new(|r: &Request| match r.target.as_str() {
        "/a" => response(302, &[("Location", "/b/c")], b""),
        "/b/c" => response(307, &[("Location", "d?x=1")], b""),
        "/b/d?x=1" => response(200, &[], b"there"),
        "/loop" => response(301, &[("Location", "/loop")], b""),
        "/none" => response(302, &[], b""),
        _ => response(404, &[], b"{\"message\":\"Not Found\"}"),
    }));
    let c = s.client();
    let base = s.base("");
    let (r, b) = c.get_bytes(&get(&format!("{base}/a"))).unwrap();
    assert_eq!((r.status, b.as_slice()), (200, &b"there"[..]));
    assert_eq!(r.url.target, "/b/d?x=1");
    let e = c.get_bytes(&get(&format!("{base}/loop"))).unwrap_err();
    assert!(e.message().contains("redirects"), "{e}");
    assert_eq!(
        s.targets().iter().filter(|t| *t == "/loop").count(),
        MAX_REDIRECTS + 1
    );
    assert!(c.get_bytes(&get(&format!("{base}/none"))).is_err());
    let (r, b) = c.get_bytes(&get(&format!("{base}/missing"))).unwrap();
    assert_eq!((r.status, b.len()), (404, 0));
    assert_eq!(r.error_body, b"{\"message\":\"Not Found\"}");
}

#[test]
fn limits_and_truncation() {
    let s = TestServer::plain(Arc::new(|r: &Request| match r.target.as_str() {
        "/big" => response(200, &[], &[b'x'; 2000]),
        "/bigchunks" => {
            let mut out = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
            for _ in 0..30 {
                out.extend_from_slice(b"64\r\n");
                out.extend_from_slice(&[b'y'; 100]);
                out.extend_from_slice(b"\r\n");
            }
            out.extend_from_slice(b"0\r\n\r\n");
            out
        }
        "/bigclose" => {
            let mut out = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
            out.extend_from_slice(&[b'z'; 3000]);
            out
        }
        _ => b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort".to_vec(),
    }));
    let c = s.client();
    let base = s.base("");
    for path in ["/big", "/bigchunks", "/bigclose"] {
        let mut g = get(&format!("{base}{path}"));
        g.max_body = 1000;
        let e = c.get_bytes(&g).unwrap_err();
        assert!(e.message().contains("allowed"), "{path}: {e}");
    }
    let e = c.get_bytes(&get(&format!("{base}/short"))).unwrap_err();
    assert!(e.message().contains("closed"), "{e}");
}

#[test]
fn many_chunks_are_not_a_header_section() {
    let s = TestServer::plain(Arc::new(|_: &Request| {
        let mut out = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for _ in 0..20_000 {
            out.extend_from_slice(b"1\r\nq\r\n");
        }
        out.extend_from_slice(b"0\r\n\r\n");
        out
    }));
    let (_, b) = s.client().get_bytes(&get(&s.base(""))).unwrap();
    assert_eq!(b.len(), 20_000);
}

#[test]
fn plain_http_only_for_loopback_or_when_allowed() {
    let s = TestServer::plain(Arc::new(|_: &Request| response(200, &[], b"ok")));
    let mut c = s.client();
    c.resolve.push(("games.example".to_owned(), s.addr));
    let url = format!("http://games.example:{}/g.zip", s.addr.port());
    let e = c.get_bytes(&get(&url)).unwrap_err();
    assert!(e.message().contains("plain http"), "{e}");
    assert!(s.targets().is_empty());
    let mut g = get(&url);
    g.allow_http = true;
    assert_eq!(c.get_bytes(&g).unwrap().1, b"ok");
}

#[test]
fn tls_with_the_test_ca() {
    let s = TestServer::tls(Arc::new(|r: &Request| match r.target.as_str() {
        "/down" => response(302, &[("Location", "http://127.0.0.1:1/x")], b""),
        _ => response(200, &[], b"secure"),
    }));
    let c = s.client();
    let (r, b) = c.get_bytes(&get(&format!("{}/x", s.base(HOST_A)))).unwrap();
    assert_eq!((r.status, b.as_slice()), (200, &b"secure"[..]));
    // Without the CA: refused.
    let mut untrusting = s.client();
    untrusting.extra_roots.clear();
    assert!(untrusting
        .get_bytes(&get(&format!("{}/x", s.base(HOST_A))))
        .is_err());
    // https never redirects to http.
    let e = c
        .get_bytes(&get(&format!("{}/down", s.base(HOST_A))))
        .unwrap_err();
    assert!(e.message().contains("https to"), "{e}");
}

#[test]
fn authorization_stays_with_its_host() {
    let port = Arc::new(Mutex::new(0u16));
    let p = port.clone();
    let s = TestServer::tls(Arc::new(move |r: &Request| {
        let host = r.headers.get("host").unwrap_or("");
        if host.starts_with(HOST_A) && r.target == "/start" {
            let to = format!("https://{HOST_B}:{}/asset", *p.lock().unwrap());
            response(302, &[("Location", to.as_str())], b"")
        } else {
            response(200, &[], b"asset")
        }
    }));
    *port.lock().unwrap() = s.addr.port();
    let mut g = get(&format!("{}/start", s.base(HOST_A)));
    g.auth = Some((HOST_A.to_owned(), "Bearer secret".to_owned()));
    let (_, b) = s.client().get_bytes(&g).unwrap();
    assert_eq!(b, b"asset");
    let reqs = s.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].headers.get("authorization"), Some("Bearer secret"));
    assert_eq!(reqs[1].headers.get("authorization"), None);
}

#[test]
fn silence_times_out_and_stop_ends_a_transfer() {
    let s = TestServer::plain(Arc::new(|r: &Request| {
        if r.target == "/slow" {
            std::thread::sleep(Duration::from_millis(800));
        }
        response(200, &[], &[b'x'; 100_000])
    }));
    let mut c = s.client();
    c.read_timeout = Duration::from_millis(200);
    assert!(c.get_bytes(&get(&format!("{}/slow", s.base("")))).is_err());
    let stop = Stop::new();
    let c = Client {
        stop: Some(stop.clone()),
        ..s.client()
    };
    let e = c
        .get(
            &get(&format!("{}/fast", s.base(""))),
            &mut |_, _| {},
            &mut |_| {
                stop.request();
                Ok(())
            },
        )
        .unwrap_err();
    assert!(e.message().contains("stopped"), "{e}");
}

#[test]
fn send_methods_bodies_and_no_redirects() {
    let s = TestServer::plain(Arc::new(|r: &Request| match r.target.as_str() {
        "/moved" => response(302, &[("Location", "/elsewhere")], b""),
        "/big" => response(200, &[], &[b'x'; 100]),
        "/err" => response(500, &[], &[b'e'; 100]),
        _ => response(
            201,
            &[],
            format!("{} {}", r.method, r.body.len()).as_bytes(),
        ),
    }));
    let c = s.client();
    let url = |p: &str| Url::parse(&format!("{}{p}", s.base(""))).unwrap();
    let (status, body) = c
        .send(
            "POST",
            &url("/api"),
            &[("Authorization", "Bearer x")],
            Some(("application/json", b"{\"a\":1}".to_vec())),
            1024,
        )
        .unwrap();
    assert_eq!((status, body.as_slice()), (201, &b"POST 7"[..]));
    {
        let reqs = s.requests.lock().unwrap();
        let r = &reqs[0];
        assert_eq!(r.headers.get("authorization"), Some("Bearer x"));
        assert_eq!(r.headers.get("content-type"), Some("application/json"));
        assert_eq!(r.headers.get("content-length"), Some("7"));
        assert_eq!(r.body, b"{\"a\":1}");
    }
    let (status, body) = c.send("DELETE", &url("/api"), &[], None, 1024).unwrap();
    assert_eq!((status, body.as_slice()), (201, &b"DELETE 0"[..]));
    // A redirect is only a status.
    let (status, _) = c.send("GET", &url("/moved"), &[], None, 1024).unwrap();
    assert_eq!(status, 302);
    assert_eq!(s.targets().len(), 3);
    // Too big a 2xx answer is an error; an error's body is cut.
    assert!(c.send("GET", &url("/big"), &[], None, 10).is_err());
    let (status, body) = c.send("GET", &url("/err"), &[], None, 10).unwrap();
    assert_eq!((status, body.len()), (500, 10));
    // Plain http only to loopback.
    let far = Url::parse("http://example.com/").unwrap();
    assert!(c.send("GET", &far, &[], None, 10).is_err());
}

#[test]
fn send_over_tls() {
    let s = TestServer::tls(Arc::new(|r: &Request| {
        response(200, &[], format!("{} {}", r.method, r.target).as_bytes())
    }));
    let url = Url::parse(&format!("{}/api/links/BCDF", s.base(HOST_A))).unwrap();
    let (status, body) = s
        .client()
        .send(
            "PUT",
            &url,
            &[],
            Some(("application/json", b"{}".to_vec())),
            1024,
        )
        .unwrap();
    assert_eq!(
        (status, body.as_slice()),
        (200, &b"PUT /api/links/BCDF"[..])
    );
}
