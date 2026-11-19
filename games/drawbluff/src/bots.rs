//! Fake players (a port of `game/bots.ts`): party clients without a
//! browser that play drawbluff by themselves (doodles, silly lies, random
//! votes) over real WebSockets. For the tests and for trying the game
//! alone (`drawbluff --bots 3`).
//!
//! [`Client`] speaks the phone's side of the party protocol (hello,
//! welcome, state diffs, acted actions, resuming after a dropped
//! connection), like `@tvpipe/party/client` does in Node.

use std::collections::{BTreeMap, HashSet};
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use tvpipe::__web::websocket::{self, Message, Receiver, Sender};
use tvpipe_party::{json, PartyError, Value, PROTOCOL};

use crate::rules::Rng;

/// Called with the client and its view after every change.
pub type ViewFn = Box<dyn Fn(&Client, &Value) + Send + Sync>;

/// A player's connection to a party, with its view kept in sync.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    url: String,
    room: String,
    key: String,
    name: String,
    on_view: Option<ViewFn>,
    st: Mutex<St>,
    cond: Condvar,
}

#[derive(Default)]
struct St {
    view: Value,
    you: Value,
    rev: u64,
    tx: Option<Sender<TcpStream>>,
    /// Acted actions without an answer yet: id → (type, data, answer).
    pending: BTreeMap<i64, Pending>,
    next_act: i64,
    welcomed: bool,
    /// Ended: by `close`, or the server's `bye` (its reason).
    ended: Option<String>,
    /// Bumped per connection, so an old reader does not reconnect.
    generation: u64,
}

struct Pending {
    kind: String,
    data: Value,
    answer: Option<Result<Value, PartyError>>,
}

const TIMEOUT: Duration = Duration::from_secs(10);

impl Client {
    /// Joins a party as a player (`url` is `ws://HOST:PORT/party/ws`):
    /// returns once welcomed, or with the server's refusal (`locked`,
    /// `name-taken`, …).
    pub fn join(
        url: &str,
        room: &str,
        key: &str,
        name: &str,
        on_view: Option<ViewFn>,
    ) -> Result<Client, PartyError> {
        let client = Client {
            inner: Arc::new(Inner {
                url: url.to_owned(),
                room: room.to_owned(),
                key: key.to_owned(),
                name: name.to_owned(),
                on_view,
                st: Mutex::new(St {
                    next_act: 1,
                    view: Value::object(),
                    ..St::default()
                }),
                cond: Condvar::new(),
            }),
        };
        client.connect(None)?;
        let end = Instant::now() + TIMEOUT;
        let mut st = client.lock();
        while !st.welcomed && st.ended.is_none() {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(PartyError::new("timeout", "No welcome"));
            }
            st = client.wait(st, left);
        }
        match &st.ended {
            Some(reason) if !st.welcomed => Err(PartyError::new(reason.clone(), "refused")),
            _ => {
                drop(st);
                Ok(client)
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, St> {
        self.inner.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wait<'a>(&self, st: MutexGuard<'a, St>, d: Duration) -> MutexGuard<'a, St> {
        match self.inner.cond.wait_timeout(st, d) {
            Ok((st, _)) => st,
            Err(e) => e.into_inner().0,
        }
    }

    /// Opens a connection and its reader thread.
    fn connect(&self, resume: Option<String>) -> Result<(), PartyError> {
        let limits = websocket::Limits::default();
        let (rx, tx) = websocket::connect(&self.inner.url, Duration::from_secs(5), limits)
            .map_err(|e| PartyError::new("connect", e.to_string()))?;
        let mut hello = json!({
            "t": "hello",
            "v": PROTOCOL,
            "room": self.inner.room.as_str(),
            "role": "player",
            "key": self.inner.key.as_str(),
            "name": self.inner.name.as_str(),
            "agent": "drawbluff-bot",
        });
        if let (Some(r), Value::Object(o)) = (resume, &mut hello) {
            o.insert("resume", r);
        }
        tx.send_text(&hello.to_string())
            .map_err(|e| PartyError::new("connect", e.to_string()))?;
        let generation = {
            let mut st = self.lock();
            st.generation += 1;
            st.tx = Some(tx);
            st.generation
        };
        let me = self.clone();
        thread::Builder::new()
            .name("drawbluff-bot".into())
            .spawn(move || me.read(rx, generation))
            .map_err(|e| PartyError::new("connect", e.to_string()))?;
        Ok(())
    }

    fn read(&self, mut rx: Receiver<TcpStream, TcpStream>, generation: u64) {
        loop {
            match rx.read_message() {
                Ok(Message::Text(text)) => {
                    if let Ok(msg) = Value::parse(&text) {
                        self.message(&msg);
                    }
                }
                Ok(Message::Binary(_)) => {}
                Ok(Message::Close(_)) | Err(_) => break,
            }
        }
        // Gone: come back (with the resume token) unless it ended.
        let resume = {
            let mut st = self.lock();
            if st.generation != generation || st.ended.is_some() {
                return;
            }
            st.tx = None;
            st.you
                .get("resume")
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        for attempt in 1..=20 {
            thread::sleep(Duration::from_millis(50 * attempt));
            if self.lock().ended.is_some() {
                return;
            }
            if self.connect(resume.clone()).is_ok() {
                return;
            }
        }
        let mut st = self.lock();
        st.ended = Some("gone".into());
        self.inner.cond.notify_all();
    }

    fn message(&self, msg: &Value) {
        let mut st = self.lock();
        let mut changed = false;
        match msg.get("t").and_then(Value::as_str) {
            Some("welcome") => {
                st.you = msg.get("you").cloned().unwrap_or_default();
                st.view = msg.get("view").cloned().unwrap_or_else(Value::object);
                st.rev = msg.get("rev").and_then(Value::as_u64).unwrap_or(0);
                if !st.welcomed {
                    let last = st.you.get("lastAct").and_then(Value::as_i64).unwrap_or(0);
                    st.next_act = st.next_act.max(last + 1);
                }
                st.welcomed = true;
                // Resend what the server has not answered.
                if let Some(tx) = st.tx.clone() {
                    for (id, p) in st.pending.iter().filter(|(_, p)| p.answer.is_none()) {
                        let m = json!({ "t": "act", "id": *id, "type": p.kind.as_str(), "data": p.data.clone() });
                        let _ = tx.send_text(&m.to_string());
                    }
                }
                changed = true;
            }
            Some("state") => {
                if let Some(full) = msg.get("full") {
                    st.view = full.clone();
                } else {
                    let rev = msg.get("rev").and_then(Value::as_u64).unwrap_or(0);
                    if rev != st.rev + 1 {
                        if let Some(tx) = &st.tx {
                            let _ = tx.send_text(r#"{"t":"resync"}"#);
                        }
                    }
                    apply_diff(&mut st.view, msg);
                }
                st.rev = msg.get("rev").and_then(Value::as_u64).unwrap_or(st.rev);
                changed = true;
            }
            Some("ack") => {
                let id = msg.get("id").and_then(Value::as_i64).unwrap_or(-1);
                if let Some(p) = st.pending.get_mut(&id) {
                    p.answer = Some(if msg.get("ok").and_then(Value::as_bool) == Some(true) {
                        Ok(msg.get("result").cloned().unwrap_or_default())
                    } else {
                        let e = msg.get("error");
                        let field = |k: &str| {
                            e.and_then(|e| e.get(k))
                                .and_then(Value::as_str)
                                .unwrap_or("error")
                                .to_owned()
                        };
                        Err(PartyError::new(field("code"), field("message")))
                    });
                }
            }
            Some("bye") => {
                let reason = msg.get("reason").and_then(Value::as_str).unwrap_or("bye");
                st.ended = Some(reason.to_owned());
            }
            _ => {}
        }
        self.inner.cond.notify_all();
        let view = changed.then(|| st.view.clone());
        drop(st);
        if let (Some(view), Some(f)) = (view, &self.inner.on_view) {
            f(self, &view);
        }
    }

    /// Sends an action and waits for its answer (10 s at most).
    pub fn act(&self, kind: &str, data: Value) -> Result<Value, PartyError> {
        let mut st = self.lock();
        let id = st.next_act;
        st.next_act += 1;
        let m = json!({ "t": "act", "id": id, "type": kind, "data": data.clone() });
        if let Some(tx) = &st.tx {
            let _ = tx.send_text(&m.to_string());
        }
        st.pending.insert(
            id,
            Pending {
                kind: kind.to_owned(),
                data,
                answer: None,
            },
        );
        let end = Instant::now() + TIMEOUT;
        loop {
            if let Some(answer) = st.pending.get_mut(&id).and_then(|p| p.answer.take()) {
                st.pending.remove(&id);
                return answer;
            }
            if let Some(reason) = &st.ended {
                let e = PartyError::new(reason.clone(), "disconnected");
                st.pending.remove(&id);
                return Err(e);
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                st.pending.remove(&id);
                return Err(PartyError::new("timeout", "No answer"));
            }
            st = self.wait(st, left);
        }
    }

    /// The view: `{roster, room, me}`.
    pub fn view(&self) -> Value {
        self.lock().view.clone()
    }

    /// The player's id (`p1`, …).
    pub fn id(&self) -> String {
        let st = self.lock();
        st.you
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    /// The name this client joined with.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Whether a connection is up (not dropped, not ended).
    pub fn connected(&self) -> bool {
        let st = self.lock();
        st.tx.is_some() && st.ended.is_none()
    }

    /// Why it ended (`kicked`, `room-closed`, `closed` after
    /// [`Client::close`]), if it did.
    pub fn ended(&self) -> Option<String> {
        self.lock().ended.clone()
    }

    /// Closes the connection for good (the player becomes away).
    pub fn close(&self) {
        let mut st = self.lock();
        st.ended.get_or_insert_with(|| "closed".into());
        if let Some(tx) = st.tx.take() {
            let _ = tx.close(1000, "");
            tx.with_stream(|s| s.shutdown(Shutdown::Both)).ok();
        }
        self.inner.cond.notify_all();
    }

    /// Drops the connection as a network would; the client comes back
    /// with its resume token (tests).
    pub fn drop_connection(&self) {
        let st = self.lock();
        if let Some(tx) = &st.tx {
            tx.with_stream(|s| s.shutdown(Shutdown::Both)).ok();
        }
    }
}

/// Applies a `state` message's `set` (merged into objects) and `del`
/// (paths) to a view, as the browser client does.
pub fn apply_diff(view: &mut Value, diff: &Value) {
    if let Some(set) = diff.get("set") {
        merge(view, set);
    }
    for path in diff.get("del").and_then(Value::as_array).unwrap_or(&[]) {
        let Some(path) = path.as_array() else {
            continue;
        };
        let Some((last, parents)) = path.split_last() else {
            continue;
        };
        let mut o = &mut *view;
        for k in parents {
            match (o, k.as_str()) {
                (Value::Object(m), Some(k)) if m.get(k).is_some() => {
                    o = m.get_mut(k).expect("checked");
                }
                _ => return,
            }
        }
        if let (Value::Object(m), Some(k)) = (o, last.as_str()) {
            m.remove(k);
        }
    }
}

fn merge(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        return;
    };
    if !matches!(target, Value::Object(_)) {
        *target = Value::object();
    }
    let Value::Object(t) = target else {
        return;
    };
    for (k, pv) in patch.iter() {
        match t.get_mut(k) {
            Some(tv @ Value::Object(_)) if matches!(pv, Value::Object(_)) => merge(tv, pv),
            _ => t.insert(k, pv.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// The bots

/// How a bot plays.
#[derive(Debug, Clone)]
pub struct BotOptions {
    /// The WebSocket URL (`ws://127.0.0.1:PORT/party/ws`).
    pub url: String,
    /// The room code.
    pub room: String,
    /// The join key.
    pub key: String,
    /// The seed of its choices.
    pub seed: u32,
    /// Start the game when VIP and enough players are there.
    pub auto_start: bool,
    /// How long it thinks before answering, ms (a random time in this range).
    pub think: (u64, u64),
    /// Never answer (to test deadlines).
    pub silent: bool,
}

impl BotOptions {
    /// Bots for a party on this computer.
    pub fn new(port: u16, room: &str, key: &str) -> BotOptions {
        BotOptions {
            url: format!("ws://127.0.0.1:{port}/party/ws"),
            room: room.to_owned(),
            key: key.to_owned(),
            seed: 1,
            auto_start: false,
            think: (0, 300),
            silent: false,
        }
    }
}

const LIES: [&str; 10] = [
    "a sad potato",
    "my uncle on monday",
    "a confused banana",
    "the cat's revenge",
    "a dancing lamp",
    "abstract art",
    "a sneaky pancake",
    "a happy accident",
    "the evil teapot",
    "a very tired frog",
];

/// A fake player.
pub struct Bot {
    /// Its connection.
    pub client: Client,
    errors: Arc<Mutex<Vec<String>>>,
}

struct Brain {
    o: BotOptions,
    rng: Mutex<Rng>,
    handled: Mutex<HashSet<String>>,
    errors: Arc<Mutex<Vec<String>>>,
}

impl Bot {
    /// Joins as `name` and starts playing.
    pub fn join(name: &str, o: BotOptions) -> Result<Bot, PartyError> {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let brain = Arc::new(Brain {
            rng: Mutex::new(Rng::new(o.seed)),
            o: o.clone(),
            handled: Mutex::new(HashSet::new()),
            errors: errors.clone(),
        });
        let b = brain.clone();
        let on_view: ViewFn = Box::new(move |c, view| b.on_view(c, view));
        let client = Client::join(&o.url, &o.room, &o.key, name, Some(on_view))?;
        Ok(Bot { client, errors })
    }

    /// The codes of the errors the host answered its actions with.
    pub fn errors(&self) -> Vec<String> {
        self.errors.lock().map(|e| e.clone()).unwrap_or_default()
    }

    /// Leaves (closes its connection).
    pub fn close(&self) {
        self.client.close();
    }
}

impl Brain {
    fn random(&self) -> f64 {
        self.rng.lock().map_or(0.5, |mut r| r.next_f64())
    }

    fn on_view(self: &Arc<Self>, c: &Client, view: &Value) {
        if self.o.silent {
            return;
        }
        let me = view.get("me").cloned().unwrap_or_default();
        let room = view.get("room").cloned().unwrap_or_default();
        let field = |v: &Value, k: &str| v.get(k).map(|x| x.to_string()).unwrap_or_default();
        // `me.key` names the drawing a lie or vote is for (`round.spot`).
        let task_key = me
            .get("key")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| field(&me, "deadline"));
        let turn = format!("{}:{task_key}", field(&room, "round"));
        let done = me.get("done").and_then(Value::as_bool) == Some(true);
        match me.get("task").and_then(Value::as_str) {
            Some("lobby") => {
                let roster = view.get("roster").and_then(Value::as_array).unwrap_or(&[]);
                let id = c.id();
                let vip = roster.iter().any(|r| {
                    r.get("id").and_then(Value::as_str) == Some(&id)
                        && r.get("vip").and_then(Value::as_bool) == Some(true)
                });
                let ready = roster
                    .iter()
                    .filter(|r| r.get("connected").and_then(Value::as_bool) == Some(true))
                    .count();
                let min = room.get("minPlayers").and_then(Value::as_u64).unwrap_or(3);
                if self.o.auto_start && vip && ready as u64 >= min {
                    self.once(c, format!("start:{ready}"), "start", Value::Null);
                }
            }
            Some("draw") if !done => {
                let drawing = doodle(&mut self.rng.lock().unwrap_or_else(|e| e.into_inner()));
                self.once(c, format!("draw:{turn}"), "draw", drawing);
            }
            Some("lie") if !done => {
                let i = (self.random() * LIES.len() as f64) as usize % LIES.len();
                // Now and then the same idea as another bot (merged lies).
                let lie = if self.random() < 0.3 {
                    json!({ "text": LIES[i] })
                } else {
                    json!({ "text": format!("{} {}", LIES[i], c.name()) })
                };
                self.once(c, format!("lie:{turn}"), "lie", lie);
            }
            Some("vote") if !done => {
                let options = me.get("options").and_then(Value::as_array).unwrap_or(&[]);
                if options.is_empty() {
                    return;
                }
                let i = (self.random() * options.len() as f64) as usize % options.len();
                let pick = options[i].get("id").cloned().unwrap_or_default();
                self.once(c, format!("vote:{turn}"), "vote", json!({ "option": pick }));
            }
            _ => {}
        }
    }

    /// Acts once per key, after thinking.
    fn once(self: &Arc<Self>, c: &Client, key: String, kind: &'static str, data: Value) {
        let fresh = self
            .handled
            .lock()
            .map(|mut h| h.insert(key))
            .unwrap_or(false);
        if !fresh {
            return;
        }
        let (lo, hi) = self.o.think;
        let delay = lo + (self.random() * (hi.saturating_sub(lo)) as f64) as u64;
        let (c, me) = (c.clone(), self.clone());
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(delay));
            if let Err(e) = c.act(kind, data) {
                if let Ok(mut errors) = me.errors.lock() {
                    errors.push(e.code);
                }
            }
        });
    }
}

/// `n` bots joined to a party, named Ada, Bea, Cy, …; the first starts
/// the game when `auto_start`. Each gets its own seed.
pub fn bots(n: usize, o: &BotOptions) -> Result<Vec<Bot>, PartyError> {
    const NAMES: [&str; 8] = ["Ada", "Bea", "Cy", "Dot", "Eli", "Fay", "Gus", "Hal"];
    (0..n)
        .map(|i| {
            let mut name = NAMES[i % NAMES.len()].to_owned();
            if i >= NAMES.len() {
                name.push_str(&i.to_string());
            }
            let mut bo = o.clone();
            bo.auto_start = o.auto_start && i == 0;
            bo.seed = o.seed.wrapping_add(i as u32);
            Bot::join(&name, bo)
        })
        .collect()
}

/// A recognisable doodle: a face, a house or a sun, in a random spot.
pub fn doodle(rng: &mut Rng) -> Value {
    let cx = 300.0 + rng.below(400) as f64;
    let cy = 300.0 + rng.below(400) as f64;
    let r = 120.0 + rng.below(80) as f64;
    let circle = |x: f64, y: f64, rad: f64, color: f64, width: f64| {
        let mut s = vec![color, width];
        for i in 0..=24 {
            let a = f64::from(i) / 24.0 * std::f64::consts::TAU;
            s.push(x + a.cos() * rad);
            s.push(y + a.sin() * rad);
        }
        s
    };
    let strokes: Vec<Vec<f64>> = match rng.below(3) {
        0 => vec![
            circle(cx, cy, r, 0.0, 14.0),
            circle(cx - r / 3.0, cy - r / 4.0, 12.0, 0.0, 18.0),
            circle(cx + r / 3.0, cy - r / 4.0, 12.0, 0.0, 18.0),
            vec![
                2.0,
                14.0,
                cx - r / 2.0,
                cy + r / 4.0,
                cx - r / 4.0,
                cy + r / 2.0,
                cx + r / 4.0,
                cy + r / 2.0,
                cx + r / 2.0,
                cy + r / 4.0,
            ],
        ],
        1 => vec![
            vec![
                6.0,
                16.0,
                cx - r,
                cy,
                cx - r,
                cy + r,
                cx + r,
                cy + r,
                cx + r,
                cy,
                cx - r,
                cy,
            ],
            vec![2.0, 16.0, cx - r - 30.0, cy, cx, cy - r, cx + r + 30.0, cy],
            vec![
                3.0,
                14.0,
                cx - 25.0,
                cy + r,
                cx - 25.0,
                cy + r / 3.0,
                cx + 25.0,
                cy + r / 3.0,
                cx + 25.0,
                cy + r,
            ],
        ],
        _ => {
            let mut s = vec![circle(cx, cy, r, 4.0, 30.0)];
            for i in 0..8 {
                let a = f64::from(i) / 8.0 * std::f64::consts::TAU;
                s.push(vec![
                    3.0,
                    12.0,
                    cx + a.cos() * (r + 20.0),
                    cy + a.sin() * (r + 20.0),
                    cx + a.cos() * (r + 90.0),
                    cy + a.sin() * (r + 90.0),
                ]);
            }
            s
        }
    };
    let strokes: Vec<Value> = strokes
        .into_iter()
        .map(|s| {
            Value::Array(
                s.into_iter()
                    .map(|v| Value::from(v.round().clamp(0.0, 1000.0) as i64))
                    .collect(),
            )
        })
        .collect();
    json!({ "strokes": strokes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffs_like_the_browser_client() {
        let mut view = json!({ "room": { "a": 1, "deep": { "x": 1, "y": 2 } }, "me": {} });
        apply_diff(
            &mut view,
            &json!({ "set": { "room": { "deep": { "z": 3 } } }, "del": [["room", "deep", "x"], ["room", "a"], ["nope", "x"]] }),
        );
        assert_eq!(
            view.to_string(),
            r#"{"room":{"deep":{"y":2,"z":3}},"me":{}}"#
        );
        apply_diff(&mut view, &json!({ "set": { "me": [1, 2] } }));
        assert_eq!(view["me"], json!([1, 2]));
    }

    #[test]
    fn doodles_are_valid_drawings() {
        let mut rng = Rng::new(3);
        for _ in 0..30 {
            let d = doodle(&mut rng);
            let v = crate::rules::validate_drawing(&d).unwrap();
            assert_eq!(v.to_value(), d);
        }
    }
}
