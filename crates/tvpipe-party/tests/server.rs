//! The HTTP side of the party server: pages, the client, Host and Origin
//! checks, the WebSocket's refusals.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use tvpipe_party::{assets, Assets, Join, Party, PartyOptions};

static PHONE: Assets = assets!(
    "tests/pages/phone",
    ["index.html", "app.js", "sub/index.html"]
);
static SCREEN: Assets = assets!("tests/pages/screen", ["index.html"]);

fn party() -> Party {
    Party::start(
        PartyOptions::new()
            .phone(&PHONE)
            .screen(&SCREEN)
            .join(Join::Local),
    )
    .unwrap()
}

fn raw(port: u16, request: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

fn get(port: u16, path: &str, extra: &str) -> String {
    raw(
        port,
        &format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n{extra}\r\n"
        ),
    )
}

#[test]
fn embedded_lists_match_the_directories() {
    PHONE.check_dir().unwrap();
    SCREEN.check_dir().unwrap();
}

#[test]
fn pages_and_client() {
    let p = party();
    let port = p.port();
    let r = get(port, "/", "");
    assert!(r.starts_with("HTTP/1.1 200 OK\r\n"), "{r}");
    assert!(
        r.contains("Content-Type: text/html; charset=utf-8\r\n"),
        "{r}"
    );
    assert!(
        r.contains("Content-Security-Policy: default-src 'self'"),
        "{r}"
    );
    assert!(
        r.contains("Cache-Control: no-cache\r\n") && r.contains("X-Content-Type-Options: nosniff")
    );
    assert!(r.ends_with("<title>phone</title>\n"), "{r}");
    let etag = r
        .lines()
        .find_map(|l| l.strip_prefix("ETag: "))
        .unwrap()
        .to_owned();
    let r = get(port, "/", &format!("If-None-Match: {etag}\r\n"));
    assert!(r.starts_with("HTTP/1.1 304 Not Modified\r\n"), "{r}");
    let r = get(port, "/app.js?v=1", "");
    assert!(
        r.contains("text/javascript") && r.ends_with("export const x = 1;\n"),
        "{r}"
    );
    assert!(get(port, "/sub", "").starts_with("HTTP/1.1 301"));
    assert!(get(port, "/sub/", "").ends_with("sub\n"));
    assert!(get(port, "/screen/", "").ends_with("<title>screen</title>\n"));
    let r = get(port, "/screen?x", "");
    assert!(
        r.starts_with("HTTP/1.1 301") && r.contains("Location: /screen/?x\r\n"),
        "{r}"
    );
    for path in ["/.secret", "/../Cargo.toml", "/nope", "/party/other"] {
        assert!(get(port, path, "").starts_with("HTTP/1.1 404"), "{path}");
    }
    let r = get(port, "/party/client.js", "");
    assert!(
        r.starts_with("HTTP/1.1 200 OK") && r.contains("export class PartyClient"),
        "{}",
        &r[..200]
    );
    let r = raw(port, &format!("POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"));
    assert!(
        r.starts_with("HTTP/1.1 405") && r.contains("Allow: GET, HEAD"),
        "{r}"
    );
    let r = raw(
        port,
        &format!("HEAD / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
    );
    assert!(
        r.starts_with("HTTP/1.1 200") && r.ends_with("\r\n\r\n"),
        "{r}"
    );
}

#[test]
fn hosts_and_origins() {
    let p = party();
    let port = p.port();
    for host in ["evil.example", "127.0.0.1", "127.0.0.1:1"] {
        let r = raw(
            port,
            &format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
        );
        assert!(r.starts_with("HTTP/1.1 421"), "{host}: {r}");
    }
    for host in [format!("localhost:{port}"), format!("[::1]:{port}")] {
        let r = raw(
            port,
            &format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
        );
        assert!(r.starts_with("HTTP/1.1 200"), "{host}: {r}");
    }
    let upgrade = |origin: &str, path: &str| {
        raw(
            port,
            &format!(
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                 Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n{origin}\r\n"
            ),
        )
    };
    assert!(upgrade("Origin: https://evil.example\r\n", "/party/ws").starts_with("HTTP/1.1 403"));
    assert!(upgrade("Origin: http://localhost:1\r\n", "/party/ws").starts_with("HTTP/1.1 403"));
    assert!(upgrade("", "/elsewhere").starts_with("HTTP/1.1 404"));
    // A good upgrade, then no hello: `bye protocol` after 5 s is covered by
    // the session vectors' shape; here only that it upgrades.
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(
        format!(
            "GET /party/ws HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
        )
        .as_bytes(),
    )
    .unwrap();
    let mut buf = [0u8; 256];
    let n = s.read(&mut buf).unwrap();
    assert!(
        buf[..n].starts_with(b"HTTP/1.1 101"),
        "{}",
        String::from_utf8_lossy(&buf[..n])
    );
}

#[test]
fn links() {
    let p = party();
    let code = p.code().to_owned();
    assert_eq!(code.len(), 4);
    let (join, screen, _) = p.__keys();
    assert_eq!(
        p.join_url(),
        format!("http://127.0.0.1:{}/?r={code}#k={join}", p.port())
    );
    assert!(p
        .screen_url()
        .ends_with(&format!("/screen/#r={code}&k={screen}&role=screen")));
    assert!(p.watch_url().is_none());
    let qr = p.join_qr().unwrap();
    let view = p.screen_view();
    assert_eq!(view["join"]["url"].as_str(), Some(p.join_url().as_str()));
    assert_eq!(view["join"]["qr"]["size"].as_u64(), Some(qr.size() as u64));
}
