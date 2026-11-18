use std::net::TcpListener;

use super::h2::tests::Edge;
use super::*;
use crate::net::httpd::{self, Handler, Reply, Server};

pub fn socket_pair() -> (TcpStream, TcpStream) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let a = TcpStream::connect(l.local_addr().unwrap()).unwrap();
    let (b, _) = l.accept().unwrap();
    b.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    (a, b)
}

#[test]
fn credentials() {
    let body = r#"{"success":true,"result":{"id":"6b0c1a6e-35b4-4f39-a0e2-6a3d1c2b9f01",
        "name":"qt-x","hostname":"some-words-here.trycloudflare.com",
        "account_tag":"abc123","secret":"AQID"},"errors":[]}"#;
    let c = parse_credentials(body).unwrap();
    assert_eq!(c.hostname, "some-words-here.trycloudflare.com");
    assert_eq!(c.tunnel_id[..2], [0x6b, 0x0c]);
    assert_eq!(
        (c.account_tag.as_str(), c.secret.as_slice()),
        ("abc123", &[1u8, 2, 3][..])
    );
    let refused =
        r#"{"success":false,"result":null,"errors":[{"code":1015,"message":"rate limited"}]}"#;
    let e = parse_credentials(refused).err().unwrap();
    assert!(e.to_string().contains("rate limited"), "{e}");
    let odd = body.replace("some-words-here.trycloudflare.com", "evil.com");
    assert!(parse_credentials(&odd).is_err());
    assert!(parse_credentials("[]").is_err());
    assert!(quick_host("a-b-1.trycloudflare.com"));
    for bad in [
        "api.trycloudflare.com",
        ".trycloudflare.com",
        "-a.trycloudflare.com",
        "A.trycloudflare.com",
        "a.trycloudflare.com.evil",
    ] {
        assert!(!quick_host(bad), "{bad}");
    }
}

#[test]
fn header_serialization() {
    // connection/header_test.go: base64 without padding, `;` between.
    let h = [("Content-Type", "text/html"), ("X", "")];
    assert_eq!(
        serialize_headers(h.into_iter()),
        "Q29udGVudC1UeXBl:dGV4dC9odG1s;WA:"
    );
}

fn page_server() -> Server {
    let handler: Handler = Arc::new(|req: &Request| match req.target.as_str() {
        "/" => Reply::Response(httpd::response(200, "OK", "text/plain", b"hello".to_vec())),
        "/echo" => {
            let host = req.headers.get("host").unwrap_or("").to_owned();
            let mut body = host.into_bytes();
            body.extend_from_slice(&req.body);
            Reply::Response(httpd::response(200, "OK", "text/plain", body))
        }
        "/ws" => {
            let resp = crate::net::websocket::server_response(req).unwrap();
            Reply::Upgrade(
                resp,
                Box::new(|mut tcp: TcpStream, rest: Vec<u8>| {
                    // Echo, upper-cased, until the tunnel side ends.
                    let _ = tcp.write_all(&rest.to_ascii_uppercase());
                    let mut buf = [0u8; 1024];
                    while let Ok(n @ 1..) = tcp.read(&mut buf) {
                        let _ = tcp.write_all(&buf[..n].to_ascii_uppercase());
                    }
                }),
            )
        }
        _ => Reply::Response(httpd::error_response(404, "Not Found")),
    });
    Server::bind("127.0.0.1:0".parse().unwrap(), handler).unwrap()
}

fn header<'a>(h: &'a [Header], name: &str) -> Option<&'a str> {
    h.iter().find(|x| x.0 == name).map(|x| x.1.as_str())
}

/// The serialized origin headers, decoded.
fn user_headers(h: &[Header]) -> Vec<(String, String)> {
    let pad = |s: &str| format!("{s}{}", "=".repeat((4 - s.len() % 4) % 4));
    let dec = |s: &str| String::from_utf8(base64::decode(&pad(s)).unwrap()).unwrap();
    header(h, "cf-cloudflared-response-headers")
        .unwrap()
        .split(';')
        .map(|kv| {
            let (k, v) = kv.split_once(':').unwrap();
            (dec(k), dec(v))
        })
        .collect()
}

/// The whole path over loopback: a fake edge registers us on the control
/// stream, then sends a GET, a POST, a WebSocket and a TCP stream.
#[test]
fn fake_edge() {
    let page = page_server();
    let inner = Arc::new(Inner {
        port: page.local_addr().port(),
        creds: Credentials {
            hostname: "x.trycloudflare.com".into(),
            tunnel_id: [7; 16],
            account_tag: "acct".into(),
            secret: vec![1, 2, 3],
        },
        client_id: [0; 16],
        stop: AtomicBool::new(false),
        state: Mutex::new(State::default()),
        peers: Mutex::new(HashMap::new()),
        changed: Condvar::new(),
    });
    let (a, b) = socket_pair();
    let h2 = h2::Conn::new(Box::new(a.try_clone().unwrap())).unwrap();
    let i = inner.clone();
    let reader = std::thread::spawn(move || h2::serve(&h2, a, &i.stop, dispatch(&i, 3)));
    let mut edge = Edge::new(b);

    // Registration.
    edge.request(
        1,
        &[
            (":method", "GET"),
            (":path", "/"),
            (UPGRADE, "control-stream"),
        ],
        false,
    );
    let (k, _, _, p) = edge.frame();
    assert_eq!(k, 1);
    assert_eq!(
        edge.hpack.decode(&p).unwrap(),
        vec![(":status".into(), "200".into())]
    );
    let mut buf = Vec::new();
    let mut msgs = Vec::new();
    while msgs.len() < 2 {
        let (_, _, id, p) = edge.frame();
        assert_eq!(id, 1);
        buf.extend(p);
        while let Some((m, n)) = capnp::Message::read(&buf).unwrap() {
            buf.drain(..n);
            msgs.push(m);
        }
    }
    assert_eq!(capnp::parse(&msgs[0]).unwrap().0, 8); // Bootstrap
    let call = msgs[1].root().unwrap().ptr_struct(0).unwrap().unwrap();
    assert_eq!(call.u64(8), capnp::REGISTRATION_SERVER);
    let params = call
        .ptr_struct(1)
        .unwrap()
        .unwrap()
        .ptr_struct(0)
        .unwrap()
        .unwrap();
    assert_eq!(params.bytes(1).unwrap(), &[7; 16]);
    let opts = params.ptr_struct(2).unwrap().unwrap();
    assert_eq!(opts.u16(2) & 0xff, 3); // previous attempts
                                       // The answer: ConnectionDetails{locationName: "tst01"}.
    let mut b = capnp::Builder::new();
    let m = b.root(1, 1);
    b.set(m, 0, &capnp::RETURN.to_le_bytes());
    let r = b.new_struct(m.ptr(0), 2, 1);
    b.set(r, 0, &1u32.to_le_bytes());
    let payload = b.new_struct(r.ptr(0), 0, 2);
    let results = b.new_struct(payload.ptr(0), 0, 1);
    let resp = b.new_struct(results.ptr(0), 1, 1);
    b.set(resp, 0, &1u16.to_le_bytes());
    let details = b.new_struct(resp.ptr(0), 1, 2);
    b.bytes(details.ptr(1), b"tst01", true);
    let ret = b.finish();
    edge.data(1, &ret[..10], false);
    edge.data(1, &ret[10..], false);
    let t = Instant::now();
    while lock(&inner.state).registered.is_none() && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let got = lock(&inner.state).registered.take().unwrap().unwrap();
    assert_eq!(got, "tst01");
    let (_, _, _, fin) = edge.frame();
    assert_eq!(
        capnp::parse(&capnp::Message::read(&fin).unwrap().unwrap().0)
            .unwrap()
            .0,
        4
    );

    // A GET.
    edge.request(
        3,
        &[
            (":method", "GET"),
            (":path", "/"),
            (":authority", "x.trycloudflare.com"),
        ],
        true,
    );
    let (h, body) = edge.response(3);
    assert_eq!(header(&h, ":status"), Some("200"));
    assert_eq!(header(&h, "content-length"), Some("5"));
    assert_eq!(
        header(&h, "cf-cloudflared-response-meta"),
        Some(r#"{"src":"origin"}"#)
    );
    assert!(user_headers(&h).contains(&("Content-Type".into(), "text/plain".into())));
    assert_eq!(body, b"hello");

    // A POST with its body in two frames; Host comes from :authority.
    edge.request(
        5,
        &[
            (":method", "POST"),
            (":path", "/echo"),
            (":authority", "x.trycloudflare.com"),
        ],
        false,
    );
    edge.data(5, b"ab", false);
    edge.data(5, b"cd", true);
    let (h, body) = edge.response(5);
    assert_eq!(header(&h, ":status"), Some("200"));
    assert_eq!(body, b"x.trycloudflare.comabcd");

    // A WebSocket: 101 becomes 200, then bytes both ways.
    edge.request(
        7,
        &[
            (":method", "GET"),
            (":path", "/ws"),
            (":authority", "x.trycloudflare.com"),
            (UPGRADE, "websocket"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ("cf-connecting-ip", "203.0.113.5"),
        ],
        false,
    );
    let (k, f, _, p) = edge.frame();
    assert_eq!((k, f & 1), (1, 0));
    let h = edge.hpack.decode(&p).unwrap();
    assert_eq!(header(&h, ":status"), Some("200"));
    let uh = user_headers(&h);
    assert!(
        uh.contains(&(
            "Sec-WebSocket-Accept".into(),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=".into()
        )),
        "{uh:?}"
    );
    assert!(
        uh.contains(&("Upgrade".into(), "websocket".into())),
        "{uh:?}"
    );
    edge.data(7, b"ping", false);
    assert_eq!(edge.read_data(7, 4), b"PING");
    // While it is open, the visitor behind its local connection is known.
    let peers: Vec<String> = lock(&inner.peers).values().cloned().collect();
    assert_eq!(peers, ["203.0.113.5"]);
    edge.data(7, b"", true);
    let (k, f, id, p) = edge.frame();
    assert_eq!((k, f & 1, id, p.len()), (0, 1, 7, 0));

    // A TCP stream (WARP routing) is refused, never proxied.
    edge.request(
        9,
        &[
            (":method", "GET"),
            (":path", "/"),
            ("cf-cloudflared-proxy-src", "1.2.3.4"),
        ],
        true,
    );
    let (h, _) = edge.response(9);
    assert_eq!(header(&h, ":status"), Some("502"));

    // A page server that is gone is a 502 too.
    drop(page);
    edge.request(
        11,
        &[(":method", "GET"), (":path", "/"), (":authority", "x")],
        true,
    );
    let (h, _) = edge.response(11);
    assert_eq!(header(&h, ":status"), Some("502"));

    // The edge hanging up ends the connection (and the control stream).
    drop(edge);
    assert!(reader.join().unwrap().is_err());
}

/// Against the real service: `cargo test --release live_quick_tunnel --
/// --ignored --nocapture` (anonymous; makes one quick tunnel).
#[test]
#[ignore]
fn live_quick_tunnel() {
    let page = page_server();
    let t0 = Instant::now();
    let tunnel = Tunnel::start(page.local_addr().port(), &|| false).unwrap();
    let host = tunnel.host().to_owned();
    // `start` waited for DNS: the system's resolver can be asked now.
    eprintln!(
        "registered and in DNS after {:?}: https://{host}/",
        t0.elapsed()
    );
    let get = |path: &str| -> Result<Response> {
        let (conn, tcp) = dial(&host, 443, &host, API_ROOTS)?;
        let req = Request::new("GET", path, Version::Http11)
            .header("Host", host.clone())
            .header("Connection", "close");
        Conn::new(StreamOwned::new(conn, tcp)).request(&req)
    };
    loop {
        match get("/") {
            Ok(r) if r.status == 200 => break,
            Ok(r) => eprintln!("{} {}", r.status, r.reason),
            Err(e) => eprintln!("{e}"),
        }
        assert!(t0.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(250));
    }
    eprintln!("first GET after {:?}", t0.elapsed());
    tunnel.drop_connection();
    let t1 = Instant::now();
    loop {
        if matches!(get("/"), Ok(r) if r.status == 200) {
            break;
        }
        assert!(t1.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!(
        "same URL back after a dropped connection: {:?}",
        t1.elapsed()
    );
    drop(tunnel);
}
