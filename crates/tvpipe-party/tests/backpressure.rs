//! Clients that do not read (docs/party-design.md §5): views are latest
//! wins once 64 KiB wait for a phone, and more than 256 KiB of anything
//! else ends it with `bye slow`; a socket without a `hello` is closed
//! after 5 s.

use std::net::TcpStream;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tvpipe::__web::websocket::{self, Message, Receiver, Sender};
use tvpipe_party::{json, Event, Join, Party, PartyOptions, Value};

type Socket = (Receiver<TcpStream, TcpStream>, Sender<TcpStream>);

fn party() -> Party {
    Party::start(PartyOptions::new().join(Join::Local)).unwrap()
}

fn connect(party: &Party) -> Socket {
    websocket::connect(
        &format!("ws://127.0.0.1:{}/party/ws", party.port()),
        Duration::from_secs(5),
        websocket::Limits::default(),
    )
    .unwrap()
}

/// Joins as a player and reads the `welcome`; nothing more is read.
fn join(party: &mut Party, name: &str) -> Socket {
    let (mut rx, tx) = connect(party);
    let hello = json!({
        "t": "hello", "v": 1, "room": party.code(), "role": "player",
        "key": party.__keys().0, "name": name,
    });
    tx.send_text(&hello.to_string()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !party
        .wait(Duration::from_millis(20))
        .iter()
        .any(|e| matches!(e, Event::Joined(_)))
    {
        assert!(Instant::now() < deadline, "no join");
    }
    party.flush();
    match rx.read_message().unwrap() {
        Message::Text(t) => assert!(t.starts_with(r#"{"t":"welcome""#), "{t}"),
        m => panic!("{m:?}"),
    }
    (rx, tx)
}

/// Reads everything on another thread, parsed.
fn reader(mut rx: Receiver<TcpStream, TcpStream>) -> mpsc::Receiver<Result<Value, Option<u16>>> {
    let (out, messages) = mpsc::channel();
    std::thread::spawn(move || loop {
        let m = match rx.read_message() {
            Ok(Message::Text(t)) => Ok(Value::parse(&t).unwrap()),
            Ok(Message::Close(c)) => Err(c.map(|(code, _)| code)),
            Ok(Message::Binary(_)) => panic!("binary"),
            Err(_) => Err(None),
        };
        let end = m.is_err();
        if out.send(m).is_err() || end {
            return;
        }
    });
    messages
}

/// 48 KiB that differ per `i`.
fn blob(i: usize) -> String {
    let c = char::from(b'a' + (i % 26) as u8);
    format!("{i}:{}", c.to_string().repeat(48 * 1024))
}

const ROUNDS: usize = 400; // ~19 MiB, well past any socket buffer

#[test]
fn views_are_latest_wins_for_a_phone_that_does_not_read() {
    let mut party = party();
    let (rx, _tx) = join(&mut party, "Ann");
    for i in 0..ROUNDS {
        party.room().set(json!({ "i": i, "blob": blob(i) }));
        party.flush();
    }
    // Now it reads: the host keeps polling, as a game does.
    let messages = reader(rx);
    let mut states = 0;
    let mut rev = 0;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < deadline, "the last view never came");
        party.wait(Duration::from_millis(5));
        party.flush();
        let Ok(m) = messages.try_recv() else {
            continue;
        };
        let m = m.expect("still open");
        assert_eq!(m.get("t").and_then(Value::as_str), Some("state"), "{m}");
        states += 1;
        let r = m.get("rev").and_then(Value::as_u64).unwrap();
        assert_eq!(r, rev + 1, "revisions without gaps");
        rev = r;
        let room = m.get("set").and_then(|s| s.get("room")).unwrap();
        let i = room.get("i").and_then(Value::as_u64).unwrap() as usize;
        assert_eq!(room.get("blob").and_then(Value::as_str), Some(&*blob(i)));
        if i == ROUNDS - 1 {
            break;
        }
    }
    assert!(
        states < ROUNDS / 2,
        "{states} views of {ROUNDS} sent: not coalesced"
    );
    assert!(party
        .player(&party.players()[0].id().clone())
        .unwrap()
        .connected());
}

#[test]
fn a_phone_that_does_not_read_is_dropped_as_slow() {
    let mut party = party();
    let (rx, _tx) = join(&mut party, "Ann");
    for i in 0..ROUNDS {
        party.broadcast("big", blob(i));
    }
    let messages = reader(rx);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut away = false;
    let mut bye = None;
    let code = loop {
        assert!(Instant::now() < deadline, "never closed");
        away |= party
            .wait(Duration::from_millis(5))
            .iter()
            .any(|e| matches!(e, Event::Away(_)));
        match messages.try_recv() {
            Ok(Ok(m)) if m.get("t").and_then(Value::as_str) == Some("bye") => bye = Some(m),
            Ok(Ok(m)) => {
                assert!(bye.is_none(), "after the bye: {m}");
                assert_eq!(m.get("type").and_then(Value::as_str), Some("big"));
            }
            Ok(Err(code)) => break code,
            Err(_) => {}
        }
    };
    assert_eq!(code, Some(4013));
    let bye = bye.expect("a bye before the close");
    assert_eq!(bye.get("reason").and_then(Value::as_str), Some("slow"));
    let end = Instant::now() + Duration::from_secs(5);
    while !away {
        assert!(Instant::now() < end, "the player never went away");
        away = party
            .wait(Duration::from_millis(20))
            .iter()
            .any(|e| matches!(e, Event::Away(_)));
    }
    // The player stays, away, and may come back.
    assert!(!party.players()[0].connected());
}

#[test]
fn a_socket_without_hello_is_closed() {
    let mut party = party();
    let (rx, _tx) = connect(&party);
    let messages = reader(rx);
    let start = Instant::now();
    let mut bye = None;
    let code = loop {
        assert!(start.elapsed() < Duration::from_secs(10), "never closed");
        party.wait(Duration::from_millis(20));
        match messages.try_recv() {
            Ok(Ok(m)) => bye = Some(m),
            Ok(Err(code)) => break code,
            Err(_) => {}
        }
    };
    assert!(start.elapsed() >= Duration::from_millis(4500));
    assert_eq!(code, Some(4002));
    let bye = bye.expect("a bye");
    assert_eq!(bye.get("reason").and_then(Value::as_str), Some("protocol"));
}
