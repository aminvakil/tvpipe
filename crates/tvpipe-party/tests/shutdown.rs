//! Ending a party from outside its loop: [`Party::waker`] ends a wait
//! (Ctrl-C handlers), [`Party::shutdown`] closes and waits until the
//! clients got their `bye room-closed`.

use std::time::{Duration, Instant};

use tvpipe::__web::websocket::{self, Message};
use tvpipe_party::{json, Event, Join, Party, PartyOptions, Value};

#[test]
fn a_waker_ends_a_wait() {
    let mut party = Party::start(PartyOptions::new().join(Join::Local)).unwrap();
    let waker = party.waker();
    let start = Instant::now();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        waker.wake();
    });
    assert!(party.wait(Duration::from_secs(10)).is_empty());
    assert!(start.elapsed() < Duration::from_secs(5));
    // A wake before the wait ends the next one.
    party.waker().wake();
    party.wait(Duration::from_secs(10));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn shutdown_waits_for_the_goodbyes() {
    let mut party = Party::start(PartyOptions::new().join(Join::Local)).unwrap();
    let (mut rx, tx) = websocket::connect(
        &format!("ws://127.0.0.1:{}/party/ws", party.port()),
        Duration::from_secs(5),
        websocket::Limits::default(),
    )
    .unwrap();
    let hello = json!({
        "t": "hello", "v": 1, "room": party.code(), "role": "player",
        "key": party.__keys().0, "name": "Ann",
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
    // The phone reads until the close (answering it, as browsers do).
    let phone = std::thread::spawn(move || {
        let mut bye = None;
        loop {
            match rx.read_message() {
                Ok(Message::Text(t)) => {
                    let v = Value::parse(&t).unwrap();
                    if v.get("t").and_then(Value::as_str) == Some("bye") {
                        bye = v.get("reason").and_then(Value::as_str).map(str::to_owned);
                    }
                }
                Ok(Message::Close(c)) => {
                    let _ = tx.close(1000, "");
                    return (bye, c.map(|(code, _)| code));
                }
                Ok(_) => {}
                Err(_) => return (bye, None),
            }
        }
    });
    assert!(party.shutdown(Duration::from_secs(5)));
    assert!(party.closed());
    let (bye, code) = phone.join().unwrap();
    assert_eq!(bye.as_deref(), Some("room-closed"));
    assert!(code.is_some());
}
