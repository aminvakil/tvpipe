//! The WebRTC fast path end to end (feature `webrtc`): a party on
//! 127.0.0.1 and a phone made of a WebSocket and an `rtc` offerer on a
//! real UDP socket, as the browser client does it.

#![cfg(feature = "webrtc")]

use std::net::{TcpStream, UdpSocket};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use rtc::data_channel::RTCDataChannelInit;
use rtc::peer_connection::configuration::RTCConfigurationBuilder;
use rtc::peer_connection::event::{RTCDataChannelEvent, RTCPeerConnectionEvent};
use rtc::peer_connection::message::RTCMessage;
use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::peer_connection::transport::{CandidateConfig, CandidateHostConfig, RTCIceCandidate};
use rtc::peer_connection::RTCPeerConnectionBuilder;
use rtc::sansio::Protocol;
use rtc::shared::{TaggedBytesMut, TransportContext, TransportProtocol};
use tvpipe::__web::websocket::{self, Message, Receiver, Sender};
use tvpipe_party::{json, Event, Join, Party, PartyOptions, PlayerId, Value};

/// What the phone saw.
#[derive(Debug)]
enum Seen {
    /// On the WebSocket.
    Ws(Value),
    /// On `r` or `u` (true: `r`).
    Channel(bool, Value),
    Open,
    Closed,
}

enum Do {
    Ws(Value),
    Channel(bool, Value),
    Offer,
    CloseWs,
}

struct Phone {
    tx: mpsc::Sender<Do>,
    rx: mpsc::Receiver<Seen>,
}

fn party() -> Party {
    Party::start(
        PartyOptions::new()
            .join(Join::Local)
            .fast_port(0)
            .fast_addrs(&["127.0.0.1".parse().unwrap()]),
    )
    .unwrap()
}

/// Joins (with `caps: ["rtc"]` if `rtc`) and runs the phone on a thread.
fn phone(party: &mut Party, name: &str, rtc: bool) -> (Phone, Value) {
    let (rx, tx) = websocket::connect(
        &format!("ws://127.0.0.1:{}/party/ws", party.port()),
        Duration::from_secs(5),
        websocket::Limits::default(),
    )
    .unwrap();
    let mut hello = json!({
        "t": "hello", "v": 1, "room": party.code(), "role": "player",
        "key": party.__keys().0, "name": name,
    });
    if rtc {
        if let Value::Object(o) = &mut hello {
            o.insert("caps", json!(["rtc"]));
        }
    }
    tx.send_text(&hello.to_string()).unwrap();
    let (do_tx, do_rx) = mpsc::channel();
    let (seen_tx, seen_rx) = mpsc::channel();
    let (ws_tx, ws_rx) = mpsc::channel();
    thread::spawn(move || ws_reader(rx, ws_tx));
    thread::spawn(move || run_phone(tx, ws_rx, do_rx, seen_tx));
    let phone = Phone {
        tx: do_tx,
        rx: seen_rx,
    };
    let welcome = phone.wait(party, |s| match s {
        Seen::Ws(v) if v.get("t").and_then(Value::as_str) == Some("welcome") => Some(v.clone()),
        _ => None,
    });
    (phone, welcome)
}

fn ws_reader(mut rx: Receiver<TcpStream, TcpStream>, out: mpsc::Sender<Value>) {
    while let Ok(m) = rx.read_message() {
        match m {
            Message::Text(t) => {
                if out.send(Value::parse(&t).unwrap()).is_err() {
                    return;
                }
            }
            Message::Close(_) => return,
            _ => {}
        }
    }
}

impl Phone {
    /// Pumps the party until the phone saw something `f` picks.
    fn wait<T>(&self, party: &mut Party, mut f: impl FnMut(&Seen) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "timed out");
            party.wait(Duration::from_millis(5));
            party.flush();
            while let Ok(s) = self.rx.try_recv() {
                if let Some(t) = f(&s) {
                    return t;
                }
            }
        }
    }

    fn send(&self, d: Do) {
        self.tx.send(d).unwrap();
    }
}

/// The phone's rtc side, driven on its own thread: an offerer with the
/// client's two negotiated channels.
fn run_phone(
    ws: Sender<TcpStream>,
    ws_in: mpsc::Receiver<Value>,
    rx: mpsc::Receiver<Do>,
    out: mpsc::Sender<Seen>,
) {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(2)))
        .unwrap();
    let local = sock.local_addr().unwrap();
    let mut pc: Option<rtc::peer_connection::RTCPeerConnection> = None;
    let (mut r, mut u) = (None, None);
    let mut opened = 0;
    let mut buf = vec![0u8; 2048];
    loop {
        while let Ok(v) = ws_in.try_recv() {
            if t(&v) == "rtc" {
                if let (Some(p), Some(sdp)) = (&mut pc, v.get("sdp").and_then(Value::as_str)) {
                    let answer = RTCSessionDescription::answer(sdp.to_owned()).unwrap();
                    p.set_remote_description(Instant::now(), answer).unwrap();
                }
            }
            let _ = out.send(Seen::Ws(v));
        }
        match rx.try_recv() {
            Ok(Do::Ws(v)) => {
                let _ = ws.send_text(&v.to_string());
            }
            Ok(Do::CloseWs) => {
                let _ = ws.close(1000, "");
            }
            Ok(Do::Offer) => {
                let mut p = RTCPeerConnectionBuilder::new()
                    .with_configuration(RTCConfigurationBuilder::new().build())
                    .build(Instant::now())
                    .unwrap();
                let init = |id, ordered, max_retransmits| RTCDataChannelInit {
                    ordered,
                    max_retransmits,
                    negotiated: Some(id),
                    ..Default::default()
                };
                r = Some(
                    p.create_data_channel("r", Some(init(0, true, None)))
                        .unwrap()
                        .id(),
                );
                u = Some(
                    p.create_data_channel("u", Some(init(1, false, Some(0))))
                        .unwrap()
                        .id(),
                );
                let c = CandidateHostConfig {
                    base_config: CandidateConfig {
                        network: "udp".into(),
                        address: local.ip().to_string(),
                        port: local.port(),
                        component: 1,
                        ..Default::default()
                    },
                    ..Default::default()
                }
                .new_candidate_host()
                .unwrap();
                p.add_local_candidate(RTCIceCandidate::from(&c).to_json().unwrap())
                    .unwrap();
                let offer = p.create_offer(None).unwrap();
                p.set_local_description(Instant::now(), offer.clone())
                    .unwrap();
                let _ = ws.send_text(&json!({"t": "rtc", "sdp": offer.sdp.as_str()}).to_string());
                pc = Some(p);
            }
            Ok(Do::Channel(reliable, v)) => {
                if let (Some(p), Some(r), Some(u)) = (&mut pc, r, u) {
                    let id = if reliable { r } else { u };
                    if let Some(mut dc) = p.data_channel(id) {
                        let _ = dc.send_text(Instant::now(), v.to_string());
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => return,
        }
        let Some(p) = &mut pc else {
            thread::sleep(Duration::from_millis(2));
            continue;
        };
        if let Ok((n, from)) = sock.recv_from(&mut buf) {
            let _ = p.handle_read(TaggedBytesMut {
                now: Instant::now(),
                transport: TransportContext {
                    local_addr: local,
                    peer_addr: from,
                    ecn: None,
                    transport_protocol: TransportProtocol::UDP,
                },
                message: buf[..n].into(),
            });
        }
        let _ = p.handle_timeout(Instant::now());
        while let Some(w) = p.poll_write() {
            let _ = sock.send_to(&w.message, w.transport.peer_addr);
        }
        while let Some(e) = p.poll_event() {
            match e {
                RTCPeerConnectionEvent::OnDataChannel(RTCDataChannelEvent::OnOpen(_)) => {
                    opened += 1;
                    if opened == 2 {
                        let _ = out.send(Seen::Open);
                    }
                }
                RTCPeerConnectionEvent::OnDataChannel(RTCDataChannelEvent::OnClose(_)) => {
                    let _ = out.send(Seen::Closed);
                }
                _ => {}
            }
        }
        while let Some(m) = p.poll_read() {
            if let RTCMessage::DataChannelMessage(c, m) = m.message {
                let v = Value::parse(&String::from_utf8_lossy(&m.data)).unwrap();
                let _ = out.send(Seen::Channel(Some(c) == r, v));
            }
        }
    }
}

fn t(v: &Value) -> &str {
    v.get("t").and_then(Value::as_str).unwrap_or("")
}

fn upgraded(party: &mut Party, name: &str) -> (Phone, PlayerId) {
    let (phone, welcome) = phone(party, name, true);
    assert_eq!(welcome.get("rtc"), Some(&Value::Bool(true)), "{welcome}");
    let id = player(party, name);
    phone.send(Do::Offer);
    let answer = phone.wait(party, |s| match s {
        Seen::Ws(v) if t(v) == "rtc" => Some(v.clone()),
        _ => None,
    });
    let sdp = answer
        .get("sdp")
        .and_then(Value::as_str)
        .expect("an answer");
    assert!(sdp.contains("a=ice-lite"), "{sdp}");
    assert!(sdp.contains("127.0.0.1"), "{sdp}");
    phone.wait(party, |s| matches!(s, Seen::Open).then_some(()));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !party.fast_path(&id) {
        assert!(
            Instant::now() < deadline,
            "the host never saw the channels open"
        );
        party.wait(Duration::from_millis(5));
    }
    (phone, id)
}

fn player(party: &Party, name: &str) -> PlayerId {
    party
        .players()
        .iter()
        .find(|p| p.name() == name)
        .unwrap()
        .id()
        .clone()
}

#[test]
fn a_phone_upgrades_and_both_channels_carry_the_protocol() {
    let mut party = party();
    assert_eq!(party.fast_addrs().len(), 1);
    let (phone, id) = upgraded(&mut party, "Ann");

    // An act on `r`: the action, then its ack and the state on `r`.
    phone.send(Do::Channel(
        true,
        json!({"t": "act", "id": 1, "type": "go", "data": 7}),
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    let action = loop {
        assert!(Instant::now() < deadline);
        let e = party.wait(Duration::from_millis(5));
        if let Some(a) = e.into_iter().find_map(|e| match e {
            Event::Action(a) => Some(a),
            _ => None,
        }) {
            break a;
        }
    };
    assert_eq!((action.kind(), action.data()), ("go", &json!(7)));
    party.room().set(json!({"phase": "go"}));
    party.reply(action, Ok(json!("ok")));
    party.flush();
    let mut got = Vec::new();
    while got.len() < 2 {
        got.push(phone.wait(&mut party, |s| match s {
            Seen::Channel(true, v) => Some(v.clone()),
            _ => None,
        }));
    }
    assert_eq!(t(&got[0]), "state", "{got:?}");
    assert_eq!(
        got[0].get("set").and_then(|s| s.get("room")),
        Some(&json!({"phase": "go"}))
    );
    assert_eq!(t(&got[1]), "ack");
    assert_eq!(got[1].get("result"), Some(&json!("ok")));

    // A ping on `r` gets its pong there.
    phone.send(Do::Channel(true, json!({"t": "ping", "c": 5})));
    let pong = phone.wait(&mut party, |s| match s {
        Seen::Channel(true, v) if t(v) == "pong" => Some(v.clone()),
        _ => None,
    });
    assert_eq!(pong.get("c"), Some(&json!(5)));

    // `in` on `u`: the latest per poll, older sequence numbers ignored.
    for s in 1..=3 {
        phone.send(Do::Channel(
            false,
            json!({"t": "in", "s": s, "data": [s, 0]}),
        ));
    }
    let mut last = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while last != Some(json!([3, 0])) {
        assert!(Instant::now() < deadline, "{last:?}");
        for e in party.wait(Duration::from_millis(5)) {
            if let Event::Input { player, data } = e {
                assert_eq!(player, id);
                last = Some(data);
            }
        }
    }
    phone.send(Do::Channel(
        false,
        json!({"t": "in", "s": 2, "data": "old"}),
    ));
    phone.send(Do::Channel(true, json!({"t": "ping", "c": 6})));
    phone.wait(&mut party, |s| {
        matches!(s, Seen::Channel(true, v) if t(v) == "pong").then_some(())
    });
    for _ in 0..20 {
        for e in party.wait(Duration::from_millis(5)) {
            assert!(!matches!(e, Event::Input { .. }), "{e:?}");
        }
    }
}

#[test]
fn input_over_the_websocket_too() {
    let mut party = party();
    let (phone, _) = phone(&mut party, "Bo", false);
    phone.send(Do::Ws(json!({"t": "in", "s": 1, "data": {"x": 1}})));
    phone.send(Do::Ws(json!({"t": "in", "s": 2, "data": {"x": 2}})));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = None;
    while last != Some(json!({"x": 2})) {
        assert!(Instant::now() < deadline, "{last:?}");
        for e in party.wait(Duration::from_millis(5)) {
            if let Event::Input { data, .. } = e {
                last = Some(data);
            }
        }
    }
}

#[test]
fn no_upgrade_without_the_capability() {
    let mut party = party();
    let (phone, welcome) = phone(&mut party, "Cy", false);
    assert_eq!(welcome.get("rtc"), None);
    phone.send(Do::Offer);
    let reply = phone.wait(&mut party, |s| match s {
        Seen::Ws(v) if t(v) == "rtc" => Some(v.clone()),
        _ => None,
    });
    assert_eq!(reply, json!({"t": "rtc", "state": "failed"}));
}

#[test]
fn a_bad_offer_is_refused() {
    let mut party = party();
    let (phone, _) = phone(&mut party, "Di", true);
    phone.send(Do::Ws(json!({"t": "rtc", "sdp": "v=0\r\nnonsense"})));
    let reply = phone.wait(&mut party, |s| match s {
        Seen::Ws(v) if t(v) == "rtc" => Some(v.clone()),
        _ => None,
    });
    assert_eq!(reply, json!({"t": "rtc", "state": "failed"}));
}

#[test]
fn the_channels_end_with_the_websocket() {
    let mut party = party();
    let (phone, id) = upgraded(&mut party, "Ed");
    phone.send(Do::CloseWs);
    phone.wait(&mut party, |s| matches!(s, Seen::Closed).then_some(()));
    assert!(!party.fast_path(&id));
}

#[test]
fn off_when_asked() {
    let party = Party::start(PartyOptions::new().join(Join::Local).fast_path(false)).unwrap();
    assert!(party.fast_addrs().is_empty());
}
