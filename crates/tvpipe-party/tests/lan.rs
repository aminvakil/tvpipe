//! LAN mode (docs/multiscreen-design.md §3.2): a second listener on a LAN
//! address that serves exactly `Host: IP:PORT` and WebSockets from
//! `Origin: http://IP:PORT`. Bound to 127.0.0.1 here (a LAN listener may
//! bind loopback), so the tests need no network.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use tvpipe_party::{assets, Assets, Join, Link, Party, PartyOptions, LAN_PORT};

static PHONE: Assets = assets!(
    "tests/pages/phone",
    ["index.html", "app.js", "sub/index.html"]
);

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

fn lan_party(port: u16) -> Party {
    Party::start(
        PartyOptions::new()
            .phone(&PHONE)
            .join(Join::Lan(Some(LOOPBACK)))
            .lan_port(port),
    )
    .unwrap()
}

fn raw(addr: SocketAddr, request: &str) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

fn get(addr: SocketAddr, host: &str) -> String {
    raw(
        addr,
        &format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
    )
}

fn upgrade(addr: SocketAddr, host: &str, origin: &str) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(
        format!(
            "GET /party/ws HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
        )
        .as_bytes(),
    )
    .unwrap();
    let mut buf = [0u8; 256];
    let n = s.read(&mut buf).unwrap_or(0);
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

#[test]
fn the_lan_link() {
    let p = lan_party(0);
    let lan = p.lan_addr().unwrap();
    assert_eq!(p.link(), Link::Lan);
    assert_ne!(lan.port(), p.port());
    let (join, _, watch) = p.__keys();
    assert_eq!(
        p.join_url(),
        format!("http://{lan}/?r={}#k={join}", p.code())
    );
    assert_eq!(
        p.watch_url().unwrap(),
        format!("http://{lan}/screen/#r={}&k={watch}&role=watch", p.code())
    );
    // The local links stay on the loopback listener.
    assert!(p
        .screen_url()
        .starts_with(&format!("http://127.0.0.1:{}/", p.port())));
    let view = p.screen_view();
    assert_eq!(view["join"]["mode"].as_str(), Some("lan"));
    assert_eq!(view["join"]["url"].as_str(), Some(p.join_url().as_str()));
}

#[test]
fn hosts_and_origins_on_the_lan_listener() {
    let p = lan_party(0);
    let lan = p.lan_addr().unwrap();
    let local = SocketAddr::from((Ipv4Addr::LOCALHOST, p.port()));
    assert!(get(lan, &lan.to_string()).starts_with("HTTP/1.1 200"));
    // Only IP:PORT: no names (DNS rebinding), not the loopback listener's.
    for host in [
        "evil.example".to_owned(),
        format!("localhost:{}", lan.port()),
        format!("127.0.0.1:{}", p.port()),
        lan.ip().to_string(),
    ] {
        assert!(get(lan, &host).starts_with("HTTP/1.1 421"), "{host}");
    }
    // The loopback listener does not serve the LAN name.
    assert!(get(local, &lan.to_string()).starts_with("HTTP/1.1 421"));
    assert!(get(local, &local.to_string()).starts_with("HTTP/1.1 200"));

    let host = lan.to_string();
    assert!(upgrade(lan, &host, &format!("http://{lan}")).starts_with("HTTP/1.1 101"));
    for origin in [
        "https://evil.example".to_owned(),
        format!("https://{lan}"),
        format!("http://localhost:{}", lan.port()),
        format!("http://127.0.0.1:{}", p.port()),
    ] {
        assert!(
            upgrade(lan, &host, &origin).starts_with("HTTP/1.1 403"),
            "{origin}"
        );
    }
    // Pages loaded locally keep working on the loopback listener.
    let r = upgrade(local, &local.to_string(), &format!("http://{local}"));
    assert!(r.starts_with("HTTP/1.1 101"), "{r}");
}

#[test]
fn the_lan_port_is_fixed_or_the_next_free() {
    let a = lan_party(LAN_PORT);
    let b = lan_party(LAN_PORT);
    let (pa, pb) = (a.lan_addr().unwrap().port(), b.lan_addr().unwrap().port());
    assert_ne!(pa, pb);
    for p in [pa, pb] {
        assert!((LAN_PORT..=LAN_PORT + 20).contains(&p), "{p}");
    }
}

#[test]
fn a_secure_context_refuses_the_lan() {
    let e = Party::start(
        PartyOptions::new()
            .join(Join::Lan(Some(LOOPBACK)))
            .require_secure_context(),
    )
    .unwrap_err();
    assert!(e.message().contains("secure context"), "{e}");
}

#[test]
#[ignore = "listens on the LAN (a firewall may ask)"]
fn the_default_route_address() {
    // Machines without a network have none; there is nothing to check then.
    let Ok(p) = Party::start(PartyOptions::new().join(Join::Lan(None)).lan_port(0)) else {
        return;
    };
    let lan = p.lan_addr().unwrap();
    assert!(!lan.ip().is_loopback() && !lan.ip().is_unspecified());
    assert!(get(lan, &lan.to_string()).starts_with("HTTP/1.1 404"));
}

#[test]
#[ignore = "starts a tunnel attempt (network) and listens on the LAN"]
fn auto_falls_back_to_the_lan_without_a_tunnel_in_time() {
    let p = Party::start(
        PartyOptions::new()
            .join(Join::Auto)
            .tunnel_timeout(Duration::from_millis(1))
            .lan_port(0),
    )
    .unwrap();
    assert_eq!(p.link(), Link::Lan);
    assert!(p.tunnel().is_none());
    assert!(p
        .join_url()
        .starts_with(&format!("http://{}/", p.lan_addr().unwrap())));
    let e = Party::start(
        PartyOptions::new()
            .join(Join::Auto)
            .tunnel_timeout(Duration::from_millis(1))
            .require_secure_context(),
    )
    .unwrap_err();
    assert!(e.message().contains("no tunnel"), "{e}");
}
