//! `Collect` with real phones (WebSocket clients) on a local party.

use std::time::{Duration, Instant};

use tvpipe::__web::websocket::{self, Message};
use tvpipe_party::{Collect, Event, Join, Party, PartyError, PartyOptions, PlayerId, Value};

struct Phone {
    rx: websocket::Receiver<std::net::TcpStream, std::net::TcpStream>,
    tx: websocket::Sender<std::net::TcpStream>,
}

impl Phone {
    fn join(party: &mut Party, name: &str) -> (Phone, PlayerId) {
        let (rx, tx) = websocket::connect(
            &format!("ws://127.0.0.1:{}/party/ws", party.port()),
            Duration::from_secs(5),
            websocket::Limits::default(),
        )
        .unwrap();
        let mut phone = Phone { rx, tx };
        let (key, _, _) = party.__keys();
        let hello = format!(
            r#"{{"t":"hello","v":1,"room":"{}","role":"player","key":"{key}","name":"{name}"}}"#,
            party.code()
        );
        phone.tx.send_text(&hello).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(Instant::now() < deadline, "no join");
            for e in party.wait(Duration::from_millis(20)) {
                if let Event::Joined(id) = e {
                    party.flush();
                    let welcome = phone.next();
                    assert_eq!(welcome["t"].as_str(), Some("welcome"));
                    return (phone, id);
                }
            }
            party.flush();
        }
    }

    fn next(&mut self) -> Value {
        loop {
            match self.rx.read_message().unwrap() {
                Message::Text(t) => {
                    let v = Value::parse(&t).unwrap();
                    if v["t"].as_str() == Some("state") {
                        continue;
                    }
                    return v;
                }
                m => panic!("{m:?}"),
            }
        }
    }

    fn act(&mut self, id: u64, kind: &str, data: &str) {
        self.tx
            .send_text(&format!(
                r#"{{"t":"act","id":{id},"type":"{kind}","data":{data}}}"#
            ))
            .unwrap();
    }
}

fn offer(party: &mut Party, c: &mut Collect<u32>) {
    for e in party.wait(Duration::from_millis(10)) {
        if let Event::Action(a) = e {
            if let Some(a) = c.offer(party, a, |data, _| {
                let n = data
                    .as_u64()
                    .ok_or_else(|| PartyError::new("bad-data", "A number"))?;
                Ok((n as u32, Value::from(n * 2)))
            }) {
                // Not ours: no reply, refused at the flush.
                drop(a);
            }
        }
    }
    party.flush();
}

fn pump(party: &mut Party, c: &mut Collect<u32>, until: impl Fn(&Party, &Collect<u32>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !until(party, c) {
        assert!(Instant::now() < deadline, "timed out");
        offer(party, c);
    }
}

/// Sends an action and runs the party until the phone has the ack.
fn exchange(
    party: &mut Party,
    c: &mut Collect<u32>,
    phone: &mut Phone,
    id: u64,
    kind: &str,
    data: &str,
) -> Value {
    phone.act(id, kind, data);
    phone
        .rx
        .with_stream(|s| s.set_read_timeout(Some(Duration::from_millis(5))))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "no ack");
        offer(party, c);
        match phone.rx.read_message() {
            Ok(Message::Text(t)) => {
                let v = Value::parse(&t).unwrap();
                if v["t"].as_str() == Some("ack") {
                    return v;
                }
            }
            Ok(m) => panic!("{m:?}"),
            Err(e) if e.kind() == tvpipe::ErrorKind::Timeout => {}
            Err(e) => panic!("{e}"),
        }
    }
}

#[test]
fn one_answer_each() {
    let mut party = Party::start(PartyOptions::new().join(Join::Local)).unwrap();
    let (mut ann, a) = Phone::join(&mut party, "Ann");
    let (mut bob, b) = Phone::join(&mut party, "Bob");
    let mut c = Collect::<u32>::new("answer", [a.clone(), b.clone()]).until(party.now() + 60_000);
    assert!(!c.done(&party));
    let ack = exchange(&mut party, &mut c, &mut ann, 1, "answer", "\"x\"");
    assert_eq!(ack["error"]["code"].as_str(), Some("bad-data"));
    let ack = exchange(&mut party, &mut c, &mut ann, 2, "answer", "21");
    assert_eq!(ack["result"].as_u64(), Some(42));
    let ack = exchange(&mut party, &mut c, &mut ann, 3, "answer", "1");
    assert_eq!(ack["error"]["code"].as_str(), Some("already-answered"));
    let ack = exchange(&mut party, &mut c, &mut ann, 4, "other", "1");
    assert_eq!(ack["error"]["code"].as_str(), Some("unknown-action"));
    assert!(!c.done(&party));
    let ack = exchange(&mut party, &mut c, &mut bob, 1, "answer", "5");
    assert_eq!(ack["result"].as_u64(), Some(10));
    assert!(c.done(&party));
    assert_eq!(c.answer(&a), Some(&21));
    assert_eq!(c.take(), vec![(a, 21), (b, 5)]);
}

#[test]
fn deadline_and_away_players() {
    let mut party = Party::start(PartyOptions::new().join(Join::Local)).unwrap();
    let (mut ann, a) = Phone::join(&mut party, "Ann");
    let (bob, b) = Phone::join(&mut party, "Bob");
    // Without a deadline, an away player is not waited for.
    drop(bob);
    pump(&mut party, &mut Collect::new("x", []), |p, _| {
        p.players().iter().filter(|p| p.connected()).count() == 1
    });
    let mut c = Collect::<u32>::new("answer", [a.clone(), b.clone()]);
    exchange(&mut party, &mut c, &mut ann, 1, "answer", "7");
    assert!(c.done(&party));
    assert_eq!(c.answers().len(), 1);
    // With a deadline, they are, until the deadline plus grace.
    let mut c = Collect::<u32>::new("answer", [a, b])
        .until(party.now() + 100)
        .grace(50);
    let t0 = Instant::now();
    pump(&mut party, &mut c, |p, c| c.done(p));
    assert!(t0.elapsed() >= Duration::from_millis(100));
    assert!(c.answers().is_empty());
}
