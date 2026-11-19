//! Replays `tests/vectors/sessions.txt` (written from `@tvpipe/party`, the
//! reference; format in `npm/party/test/vectors.ts`) against this host.

use std::collections::{HashMap, VecDeque};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use tvpipe::__web::websocket::{self, Message};
use tvpipe_party::{
    json, Event, Join, LateJoin, Limits, Object, Party, PartyError, PartyOptions, PlayerId, Role,
    Source, Value,
};

// ---------------------------------------------------------------------------
// The vector file

struct Block {
    line: usize,
    entries: Vec<(String, String)>,
}

impl Block {
    fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

fn blocks(text: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut open = false;
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            open = false;
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let (k, v) = line.split_once(" = ").expect("key = value");
        if !open {
            out.push(Block {
                line: i + 1,
                entries: Vec::new(),
            });
            open = true;
        }
        out.last_mut()
            .unwrap()
            .entries
            .push((k.trim().to_owned(), v.to_owned()));
    }
    out
}

// ---------------------------------------------------------------------------
// The host: a party driven by a loop thread, as a game would

enum Host {
    Step(String, mpsc::Sender<()>),
    Stop,
}

fn host_loop(mut party: Party, rx: mpsc::Receiver<Host>) {
    let mut inputs = Object::new();
    loop {
        for event in party.wait(Duration::from_millis(2)) {
            if let Event::Input { player, data } = &event {
                inputs.insert(player.as_str(), data.clone());
            }
            if let Event::Action(a) = event {
                let data = a.data().clone();
                let result = match a.kind() {
                    "echo" => Ok(data),
                    "fail" => Err(PartyError::new(
                        data.get("code").and_then(Value::as_str).unwrap_or("failed"),
                        data.get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("failed"),
                    )),
                    "set-room" | "set-screen" | "set-me" if matches!(data, Value::Object(_)) => {
                        match a.kind() {
                            "set-room" => party.room().set(data),
                            "set-screen" => party.screen().set(data),
                            _ => {
                                if let Source::Player(p) = a.from() {
                                    let p = p.clone();
                                    if let Some(s) = party.player_state(&p) {
                                        s.set(data);
                                    }
                                }
                            }
                        }
                        Ok(Value::Null)
                    }
                    "set-room" | "set-screen" | "set-me" => Ok(Value::Null),
                    "inputs" => Ok(Value::Object(inputs.clone())),
                    // Unanswered: `unknown-action` at the flush.
                    _ => {
                        drop(a);
                        continue;
                    }
                };
                party.reply(a, result);
            }
        }
        party.flush();
        loop {
            match rx.try_recv() {
                Ok(Host::Step(step, done)) => {
                    host_step(&mut party, &step);
                    party.flush();
                    let _ = done.send(());
                }
                Ok(Host::Stop) | Err(mpsc::TryRecvError::Disconnected) => return,
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
    }
}

fn host_step(party: &mut Party, step: &str) {
    let (verb, rest) = step.split_once(' ').unwrap_or((step, ""));
    let json = |s: &str| Value::parse(s).expect("host step JSON");
    let id = |s: &str| {
        party
            .players()
            .iter()
            .find(|p| p.id().as_str() == s)
            .map(|p| p.id().clone())
            .expect("a player")
    };
    match verb {
        "room.set" => party.room().set(json(rest)),
        "room.replace" => party.room().replace(json(rest)),
        "screen.set" => party.screen().set(json(rest)),
        "me" => {
            let (who, data) = rest.split_once(' ').unwrap();
            let who: PlayerId = id(who);
            party.player_state(&who).unwrap().set(json(data));
        }
        "lock" => party.lock(),
        "late-join" => party.set_late_join(LateJoin::parse(rest).expect("a mode")),
        "in-progress" => party.set_in_progress(rest == "true"),
        "activate" => {
            let who = id(rest);
            party.activate(&who);
        }
        "unlock" => party.unlock(),
        "kick" => {
            let who = id(rest);
            party.kick(&who, "kicked");
        }
        "close" => party.close(),
        "send" => {
            let mut it = rest.splitn(3, ' ');
            let (who, kind, data) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
            let who = id(who);
            party.send(&who, kind, json(data));
        }
        "broadcast" => {
            let (kind, data) = rest.split_once(' ').unwrap();
            party.broadcast_to(&[Role::Player, Role::Screen, Role::Watch], kind, json(data));
        }
        _ => panic!("bad host step {step}"),
    }
}

// ---------------------------------------------------------------------------
// Fake phones

#[derive(Default)]
struct Inbox {
    messages: VecDeque<String>,
    /// The server's close code (`Some(None)`: the socket died).
    closed: Option<Option<u16>>,
    /// We closed it ourselves: ignore the end.
    ours: bool,
}

struct Client {
    tx: websocket::Sender<std::net::TcpStream>,
    inbox: Arc<(Mutex<Inbox>, Condvar)>,
}

fn open(port: u16) -> Client {
    let url = format!("ws://127.0.0.1:{port}/party/ws");
    let (mut rx, tx) = websocket::connect(
        &url,
        Duration::from_secs(5),
        websocket::Limits {
            max_message: 16 << 20,
        },
    )
    .expect("connect");
    let inbox: Arc<(Mutex<Inbox>, Condvar)> = Arc::default();
    let ib = inbox.clone();
    std::thread::spawn(move || loop {
        let m = rx.read_message();
        let (lock, cv) = &*ib;
        let mut st = lock.lock().unwrap();
        match m {
            Ok(Message::Text(t)) => st.messages.push_back(t),
            Ok(Message::Binary(_)) => st.messages.push_back("<binary>".into()),
            Ok(Message::Close(c)) => {
                st.closed = Some(c.map(|(code, _)| code));
                cv.notify_all();
                return;
            }
            Err(e) if e.kind() == tvpipe::ErrorKind::Timeout => continue,
            Err(_) => {
                st.closed = Some(None);
                cv.notify_all();
                return;
            }
        }
        cv.notify_all();
    });
    Client { tx, inbox }
}

enum Got {
    Message(String),
    Closed(Option<u16>),
}

impl Client {
    fn next(&self, what: &str) -> Got {
        let (lock, cv) = &*self.inbox;
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut st = lock.lock().unwrap();
        loop {
            if let Some(m) = st.messages.pop_front() {
                return Got::Message(m);
            }
            if let Some(c) = st.closed.take() {
                st.ours = true;
                return Got::Closed(c);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "{what}: timed out");
            st = cv.wait_timeout(st, left).unwrap().0;
        }
    }
}

// ---------------------------------------------------------------------------
// Matching

fn substitute(v: &Value, vars: &HashMap<String, Value>) -> Value {
    match v {
        Value::String(s) if s.starts_with('$') => {
            if s == "$room" {
                return Value::from(vars["ROOM"].as_str().unwrap().to_lowercase());
            }
            vars.get(&s[1..]).cloned().unwrap_or_else(|| v.clone())
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, vars)).collect()),
        Value::Object(o) => {
            let mut out = Object::new();
            for (k, x) in o.iter() {
                out.insert(k, substitute(x, vars));
            }
            Value::Object(out)
        }
        v => v.clone(),
    }
}

fn matches(expected: &Value, actual: &Value, vars: &mut HashMap<String, Value>) -> bool {
    match expected {
        Value::String(s) if s.starts_with('$') => {
            let name = &s[1..];
            if name == "_" {
                return true;
            }
            match vars.get(name) {
                Some(v) => v == actual,
                None => {
                    vars.insert(name.to_owned(), actual.clone());
                    true
                }
            }
        }
        Value::Array(e) => match actual {
            Value::Array(a) => {
                e.len() == a.len() && e.iter().zip(a).all(|(x, y)| matches(x, y, vars))
            }
            _ => false,
        },
        Value::Object(e) => match actual {
            Value::Object(a) => {
                e.len() == a.len()
                    && e.iter()
                        .all(|(k, x)| a.get(k).is_some_and(|y| matches(x, y, vars)))
            }
            _ => false,
        },
        Value::Number(n) => actual.as_number().is_some_and(|m| m == n),
        _ => expected == actual,
    }
}

// ---------------------------------------------------------------------------

fn options(o: Option<&str>) -> PartyOptions {
    let mut opts = PartyOptions::new().join(Join::Local);
    let Some(o) = o.map(|s| Value::parse(s).unwrap()) else {
        return opts;
    };
    if let Some(n) = o.get("maxPlayers").and_then(Value::as_u64) {
        opts = opts.max_players(n as usize);
    }
    if let Some(m) = o.get("lateJoin").and_then(Value::as_str) {
        opts = opts.late_join(LateJoin::parse(m).expect("a mode"));
    }
    if let Some(l) = o.get("limits") {
        let mut limits = Limits::default();
        if let Some(r) = l.get("rate").and_then(Value::as_f64) {
            limits.rate = r;
        }
        if let Some(b) = l.get("burst").and_then(Value::as_f64) {
            limits.burst = b;
        }
        if let Some(s) = l.get("screens").and_then(Value::as_u64) {
            limits.screens = s as usize;
        }
        opts = opts.limits(limits);
    }
    opts
}

fn run(block: &Block) {
    let name = block.get("name").unwrap_or("?");
    let mut party = Party::start(options(block.get("options"))).unwrap();
    party.__set_join(json!({
        "code": (party.code()),
        "url": "https://party.test/",
        "qr": {"size": 1, "rows": ["1"]}
    }));
    party.flush();
    let port = party.port();
    let (join, screen, watch) = party.__keys();
    let mut vars: HashMap<String, Value> = HashMap::from([
        ("ROOM".to_owned(), Value::from(party.code())),
        ("JOIN".to_owned(), Value::from(join)),
        ("SCREEN".to_owned(), Value::from(screen)),
        ("WATCH".to_owned(), Value::from(watch)),
    ]);
    let (host_tx, host_rx) = mpsc::channel();
    let host = std::thread::spawn(move || host_loop(party, host_rx));
    let mut clients: HashMap<String, Client> = HashMap::new();
    for step in block
        .entries
        .iter()
        .filter(|(k, _)| k == "step")
        .map(|(_, v)| v)
    {
        let what = format!("{name} (line {}): {step}", block.line);
        let (who, rest) = step.split_once(' ').unwrap();
        let (verb, arg) = rest.split_once(' ').unwrap_or((rest, ""));
        if who == "host" {
            let (done_tx, done_rx) = mpsc::channel();
            host_tx.send(Host::Step(rest.to_owned(), done_tx)).unwrap();
            done_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("host step");
            continue;
        }
        match verb {
            "open" => {
                clients.insert(who.to_owned(), open(port));
            }
            "close" => {
                let c = &clients[who];
                c.inbox.0.lock().unwrap().ours = true;
                let _ = c.tx.close(1000, "");
            }
            "<" => match clients[who].next(&what) {
                Got::Message(m) => {
                    let expected = Value::parse(arg).unwrap();
                    let actual = Value::parse(&m).unwrap();
                    assert!(matches(&expected, &actual, &mut vars), "{what}\n  got {m}");
                }
                Got::Closed(c) => panic!("{what}\n  got closed {c:?}"),
            },
            "closed" => match clients[who].next(&what) {
                Got::Closed(c) => assert_eq!(c, Some(arg.parse().unwrap()), "{what}"),
                Got::Message(m) => panic!("{what}\n  got {m}"),
            },
            v if v == ">" || v.starts_with(">*") => {
                let times: usize = v.strip_prefix(">*").map_or(1, |n| n.parse().unwrap());
                let text = substitute(&Value::parse(arg).unwrap(), &vars).to_string();
                for _ in 0..times {
                    clients[who].tx.send_text(&text).unwrap();
                }
            }
            ">pad" => {
                let (n, json) = arg.split_once(' ').unwrap();
                let mut msg = substitute(&Value::parse(json).unwrap(), &vars);
                if let Value::Object(o) = &mut msg {
                    o.insert("pad", "x".repeat(n.parse().unwrap()));
                }
                clients[who].tx.send_text(&msg.to_string()).unwrap();
            }
            ">text" => clients[who].tx.send_text(arg).unwrap(),
            ">binary" => {
                let bytes: Vec<u8> = (0..arg.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&arg[i..i + 2], 16).unwrap())
                    .collect();
                clients[who].tx.send_binary(&bytes).unwrap();
            }
            _ => panic!("{what}: bad step"),
        }
    }
    std::thread::sleep(Duration::from_millis(100));
    for (who, c) in &clients {
        let st = c.inbox.0.lock().unwrap();
        assert!(
            st.messages.is_empty(),
            "{name}: {who} has unread messages: {:?}",
            st.messages
        );
        assert!(
            st.ours || st.closed.is_none(),
            "{name}: {who} was closed unexpectedly ({:?})",
            st.closed
        );
    }
    let _ = host_tx.send(Host::Stop);
    host.join().unwrap();
}

#[test]
fn sessions_from_node() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors/sessions.txt");
    let text = std::fs::read_to_string(path).unwrap();
    let blocks = blocks(&text);
    assert!(blocks.len() > 20);
    let only = std::env::var("SESSION").ok();
    for b in &blocks {
        if only
            .as_deref()
            .is_some_and(|o| !b.get("name").unwrap_or("").contains(o))
        {
            continue;
        }
        run(b);
    }
}
