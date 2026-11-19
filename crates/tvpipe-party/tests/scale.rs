//! 50 phones and a screen on one party: what a state change costs the
//! host (time per flush, bytes per change), the numbers
//! docs/drawbluff-scale.md §11 quotes (as `npm/party/test/scale.test.ts`).
//! `PARTY_SCALE_REPORT=1` prints them.

use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tvpipe::__web::websocket::{self, Message, Receiver, Sender};
use tvpipe_party::{json, Event, Join, Party, PartyOptions, Value};

type Socket = (Receiver<TcpStream, TcpStream>, Sender<TcpStream>);

fn connect(party: &Party) -> Socket {
    websocket::connect(
        &format!("ws://127.0.0.1:{}/party/ws", party.port()),
        Duration::from_secs(5),
        websocket::Limits::default(),
    )
    .unwrap()
}

/// A client that reads everything on a thread, counting `state` messages.
struct Phone {
    tx: Sender<TcpStream>,
    states: Arc<AtomicUsize>,
}

fn hello(party: &mut Party, hello: Value, joins: bool) -> Phone {
    let (mut rx, tx) = connect(party);
    tx.send_text(&hello.to_string()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !party.wait(Duration::from_millis(5)).iter().any(|e| {
        if joins {
            matches!(e, Event::Joined(_))
        } else {
            matches!(e, Event::Screen { .. })
        }
    }) {
        assert!(Instant::now() < deadline, "no welcome");
    }
    party.flush();
    let states = Arc::new(AtomicUsize::new(0));
    let count = states.clone();
    std::thread::spawn(move || {
        while let Ok(m) = rx.read_message() {
            if let Message::Text(t) = m {
                if t.starts_with(r#"{"t":"state""#) {
                    count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    });
    Phone { tx, states }
}

/// ~15 KB of strokes, as a phone sends a drawing.
fn drawing(seed: u64) -> Value {
    let mut x = seed;
    let mut r = || {
        x = (x * 1_103_515_245 + 12_345) % 2_147_483_648;
        x % 1000
    };
    let strokes: Vec<Value> = (0..40)
        .map(|_| {
            let mut s = vec![Value::from(r() % 8), Value::from(20)];
            s.extend((0..60).map(|_| Value::from(r())));
            Value::Array(s)
        })
        .collect();
    json!({ "strokes": strokes })
}

#[test]
fn fifty_phones_one_answer_one_phase_change_one_roster_change() {
    let mut party = Party::start(PartyOptions::new().join(Join::Local).max_players(50)).unwrap();
    let (code, join_key, screen_key) = {
        let (j, s, _) = party.__keys();
        (party.code().to_owned(), j.to_owned(), s.to_owned())
    };
    let mut phones: Vec<Phone> = (0..50)
        .map(|i| {
            let h = json!({ "t": "hello", "v": 1, "room": code.as_str(), "role": "player", "key": join_key.as_str(), "name": format!("P{i}") });
            hello(&mut party, h, true)
        })
        .collect();
    let screen = {
        let h = json!({ "t": "hello", "v": 1, "room": code.as_str(), "role": "screen", "key": screen_key.as_str() });
        hello(&mut party, h, false)
    };
    let report = std::env::var_os("PARTY_SCALE_REPORT").is_some();
    let measure = |party: &mut Party,
                   what: &str,
                   n: usize,
                   change: &mut dyn FnMut(&mut Party, usize)| {
        let before = party.stats();
        for i in 0..n {
            change(party, i);
            party.flush();
        }
        let after = party.stats();
        let per = |a: u64, b: u64| (a - b) as f64 / n as f64;
        let line = json!({
            "what": what,
            "changes": n,
            "flushMsPerChange": (after.flush_time - before.flush_time).as_secs_f64() * 1000.0 / n as f64,
            "statesPerChange": per(after.states, before.states),
            "bytesPerChange": per(after.state_bytes, before.state_bytes).round(),
        });
        if report {
            println!("{line}");
        }
        line
    };
    let d = drawing(1);
    let ids: Vec<_> = party.players().iter().map(|p| p.id().clone()).collect();
    let phase = measure(
        &mut party,
        "room: a new phase with a 15 KB drawing (all 51 clients)",
        1,
        &mut |party, _| {
            party
                .room()
                .replace(json!({ "phase": "vote", "drawing": d.clone(), "deadline": 1 }));
            let waiting: Vec<&str> = ids.iter().map(|p| p.as_str()).collect();
            party.screen().replace(
                json!({ "phase": "vote", "drawing": d.clone(), "waiting": waiting, "done": [] }),
            );
            for id in &ids {
                if let Some(s) = party.player_state(id) {
                    s.replace(json!({ "task": "vote", "options": [{ "id": "o1", "text": "a sad potato" }], "done": false }));
                }
            }
        },
    );
    assert_eq!(phase["statesPerChange"].as_f64(), Some(51.0));
    let mut done: Vec<String> = Vec::new();
    let answers = measure(
        &mut party,
        "an answer: one phone's state and the screen's done list",
        50,
        &mut |party, i| {
            let id = ids[i].clone();
            if let Some(s) = party.player_state(&id) {
                s.set(json!({ "done": true, "choice": "o1" }));
            }
            done.push(id.as_str().to_owned());
            party.screen().set(json!({ "done": done.clone() }));
        },
    );
    assert_eq!(answers["statesPerChange"].as_f64(), Some(2.0));
    // A player leaves: the roster changes for everybody.
    let gone = phones.pop().unwrap();
    gone.tx.send_text(r#"{"t":"leave"}"#).unwrap();
    let before = party.stats();
    let deadline = Instant::now() + Duration::from_secs(5);
    while party.stats().states == before.states {
        assert!(Instant::now() < deadline, "no roster change");
        party.wait(Duration::from_millis(5));
        party.flush();
    }
    let after = party.stats();
    if report {
        let line = json!({
            "what": "roster: a player leaves (all clients)",
            "flushMs": (after.flush_time - before.flush_time).as_secs_f64() * 1000.0,
            "states": after.states - before.states,
            "bytes": after.state_bytes - before.state_bytes,
        });
        println!("{line}");
    }
    assert!(after.states - before.states >= 50);
    // Everybody got the phase and their answer; the screen every change.
    let deadline = Instant::now() + Duration::from_secs(5);
    while phones.iter().any(|p| p.states.load(Ordering::SeqCst) < 2)
        || screen.states.load(Ordering::SeqCst) < 51
    {
        assert!(Instant::now() < deadline, "messages missing");
        std::thread::sleep(Duration::from_millis(5));
    }
}
