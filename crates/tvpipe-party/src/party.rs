//! One party: its players and screens, their views, actions and messages
//! (docs/party-design.md §5). The game owns the [`Party`] and drives it
//! from its own loop: [`Party::poll`] (or [`Party::wait`]) returns what
//! happened, [`Party::flush`] sends what changed.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::conn::{Conn, ConnId, Inbound, Incoming, STATE_HIGH_WATER};
use crate::limits::{TokenBucket, WindowCounter};
use crate::names::{clean_text, name_key, MAX_NAME};
use crate::sync::diff;
use crate::value::{dedupe, Object, Value};

/// The protocol version (`hello.v`).
pub const PROTOCOL: i64 = 1;

/// Bytes queued to one socket before it is closed as too slow.
const SLOW_LIMIT: usize = 256 * 1024;
/// Results of earlier actions kept per player, for replays.
const ACK_CACHE: usize = 32;
/// Largest message other than `act`.
const MAX_CONTROL_MESSAGE: usize = 16 * 1024;
/// Largest exact integer in JavaScript.
const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

/// Why a client was sent away (`bye`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bye {
    Protocol,
    BadKey,
    NoRoom,
    NoPlayer,
    NameTaken,
    NameInvalid,
    RateLimited,
    Replaced,
    Kicked,
    RoomClosed,
    Slow,
    Full,
    Locked,
}

impl Bye {
    fn reason(self) -> &'static str {
        match self {
            Bye::Protocol => "protocol",
            Bye::BadKey => "bad-key",
            Bye::NoRoom => "no-room",
            Bye::NoPlayer => "no-player",
            Bye::NameTaken => "name-taken",
            Bye::NameInvalid => "name-invalid",
            Bye::RateLimited => "rate-limited",
            Bye::Replaced => "replaced",
            Bye::Kicked => "kicked",
            Bye::RoomClosed => "room-closed",
            Bye::Slow => "slow",
            Bye::Full => "full",
            Bye::Locked => "locked",
        }
    }

    fn code(self) -> u16 {
        match self {
            Bye::Protocol => 4002,
            Bye::BadKey => 4003,
            Bye::NoRoom => 4004,
            Bye::NoPlayer => 4005,
            Bye::NameTaken => 4006,
            Bye::NameInvalid => 4007,
            Bye::RateLimited => 4008,
            Bye::Replaced => 4009,
            Bye::Kicked => 4010,
            Bye::RoomClosed => 4011,
            Bye::Slow => 4013,
            Bye::Full | Bye::Locked => 4029,
        }
    }
}

fn bye(conn: &Conn, why: Bye, message: Option<&str>) {
    let mut msg = Object::new();
    msg.insert("t", "bye");
    msg.insert("reason", why.reason());
    if let Some(m) = message.filter(|m| !m.is_empty()) {
        msg.insert("message", m);
    }
    conn.send(Value::Object(msg).to_string());
    conn.close(why.code(), why.reason());
}

/// An error for the phone: its `act()` rejects with this code and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartyError {
    /// A short machine-readable code (`not-vip`, `too-late`).
    pub code: String,
    /// For people.
    pub message: String,
}

impl PartyError {
    /// An error with a code and a message.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> PartyError {
        PartyError {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for PartyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PartyError {}

/// A random token: 128 bits, base64url.
pub(crate) fn token() -> String {
    let mut b = [0u8; 16];
    random(&mut b);
    base64url(&b)
}

fn random(buf: &mut [u8]) {
    use ring::rand::SecureRandom;
    // The system's generator does not fail on supported platforms; if it
    // ever did, refusing to run beats predictable keys.
    ring::rand::SystemRandom::new()
        .fill(buf)
        .expect("the system random generator failed");
}

fn base64url(b: &[u8]) -> String {
    tvpipe::__web::base64::encode(b)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

/// Constant-time comparison (keys and tokens).
pub(crate) fn same_token(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a
        .bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

/// Room codes: four consonants (no words, no 0/O confusion).
pub(crate) fn room_code() -> String {
    const LETTERS: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";
    let mut b = [0u8; 4];
    random(&mut b);
    b.iter()
        .map(|x| LETTERS[*x as usize % LETTERS.len()] as char)
        .collect()
}

/// Milliseconds since the Unix epoch (`Date.now()`).
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A player's id (`p1`, `p2`, … in join order; stable for the party).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlayerId(String);

impl PlayerId {
    /// The id as sent to phones.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PlayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&PlayerId> for Value {
    fn from(id: &PlayerId) -> Value {
        Value::from(id.0.as_str())
    }
}

/// A piece of state the host sets and clients see: the room (every
/// player), the screen (screens), a player's own (`me`). Changes made
/// before a [`Party::flush`] go out as one diff.
#[derive(Debug, Clone, Default)]
pub struct State {
    value: Object,
    dirty: bool,
}

impl State {
    /// The current state.
    pub fn get(&self) -> &Object {
        &self.value
    }

    /// Merges the top-level fields of `partial` (an object) into the state.
    ///
    /// # Panics
    /// When `partial` is not an object (a bug in the game).
    pub fn set(&mut self, partial: impl Into<Value>) {
        match partial.into() {
            Value::Object(o) => {
                for (k, v) in o.iter() {
                    self.value.insert(k, v.clone());
                }
                self.dirty = true;
            }
            v => panic!("party state must be an object, not {v}"),
        }
    }

    /// Replaces the whole state.
    ///
    /// # Panics
    /// When `value` is not an object.
    pub fn replace(&mut self, value: impl Into<Value>) {
        match value.into() {
            Value::Object(o) => {
                self.value = o;
                self.dirty = true;
            }
            v => panic!("party state must be an object, not {v}"),
        }
    }

    /// Removes a top-level field.
    pub fn remove(&mut self, key: &str) {
        if self.value.remove(key).is_some() {
            self.dirty = true;
        }
    }

    /// Changes the state in place.
    pub fn update(&mut self, f: impl FnOnce(&mut Object)) {
        f(&mut self.value);
        self.dirty = true;
    }
}

/// A player: a person with a phone, across reconnects.
#[derive(Debug)]
pub struct Player {
    id: PlayerId,
    name: String,
    vip: bool,
    /// What only this player sees (`view.me`).
    pub state: State,
    resume: String,
    last_act: i64,
    results: VecDeque<(i64, Value)>,
    client: Option<ConnId>,
}

impl Player {
    /// The id.
    pub fn id(&self) -> &PlayerId {
        &self.id
    }

    /// The cleaned name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this player is the VIP (starts the game).
    pub fn vip(&self) -> bool {
        self.vip
    }

    /// Whether the player's phone is connected.
    pub fn connected(&self) -> bool {
        self.client.is_some()
    }

    fn roster_entry(&self) -> Value {
        let mut o = Object::new();
        o.insert("id", self.id.as_str());
        o.insert("name", self.name.as_str());
        o.insert("vip", self.vip);
        o.insert("connected", self.connected());
        Value::Object(o)
    }
}

/// Who a socket is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A phone.
    Player,
    /// The TV page (or anything with the screen key).
    Screen,
    /// A read-only screen (the watch link).
    Watch,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Player => "player",
            Role::Screen => "screen",
            Role::Watch => "watch",
        }
    }
}

/// Who sent an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A player.
    Player(PlayerId),
    /// A screen, by its connection's id (`s1`, …).
    Screen(String),
}

/// An action from a phone or a screen. Answer it with [`Party::reply`]
/// before the next [`Party::flush`]; one left unanswered is refused as
/// `unknown-action` (the game has no handler for it).
#[derive(Debug)]
#[must_use = "an action needs a reply (Party::reply)"]
pub struct Action {
    key: u64,
    id: i64,
    kind: String,
    data: Value,
    from: Source,
}

impl Action {
    /// The action's type (`start`, `answer`).
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Its data (`null` when none).
    pub fn data(&self) -> &Value {
        &self.data
    }

    /// Who sent it.
    pub fn from(&self) -> &Source {
        &self.from
    }

    /// The player who sent it (`None` for screens).
    pub fn player(&self) -> Option<&PlayerId> {
        match &self.from {
            Source::Player(p) => Some(p),
            Source::Screen(_) => None,
        }
    }
}

/// What happened, from [`Party::poll`].
#[derive(Debug)]
#[non_exhaustive]
pub enum Event {
    /// A new player joined.
    Joined(PlayerId),
    /// A player's phone is back.
    Back(PlayerId),
    /// A player's phone went away (the player stays).
    Away(PlayerId),
    /// A player left for good (`left`, `kicked`).
    Left {
        /// Who (no longer in the party).
        player: PlayerId,
        /// Their name.
        name: String,
        /// `left` or `kicked`.
        reason: String,
    },
    /// The VIP changed.
    Vip(Option<PlayerId>),
    /// A screen connected or left.
    Screen {
        /// Its id (`s1`, …).
        id: String,
        /// Screen or watcher.
        role: Role,
        /// Whether it came or went.
        connected: bool,
    },
    /// An action to answer.
    Action(Action),
}

/// What [`Moderate`] decides about a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moderation {
    /// Show it as it is.
    Accept,
    /// Show this instead.
    Replace(String),
    /// Refuse it.
    Refuse,
}

/// Checks names (and whatever the game passes to [`Party::moderate`]):
/// the text and its kind (`name`, or the game's).
pub type Moderate = Box<dyn Fn(&str, &str) -> Moderation + Send>;

/// Rate and size limits of a party.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Messages per second per socket (default 20).
    pub rate: f64,
    /// Messages a socket may send at once (default 50).
    pub burst: f64,
    /// Screens and watchers at once (default 16).
    pub screens: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            rate: 20.0,
            burst: 50.0,
            screens: 16,
        }
    }
}

/// A socket after its `hello`.
struct Client {
    conn: Conn,
    role: Role,
    /// `p1` or `s1`.
    id: String,
    player: Option<PlayerId>,
    /// The view this client has.
    sent: Object,
    rev: u64,
    waiting: bool,
    bucket: TokenBucket,
    /// Screens: action ids seen on this connection.
    last_act: i64,
}

impl Client {
    fn send(&self, msg: Value) -> bool {
        if self.conn.closed() {
            return false;
        }
        if self.conn.buffered() > SLOW_LIMIT {
            bye(&self.conn, Bye::Slow, None);
            return false;
        }
        self.conn.send(msg.to_string());
        true
    }
}

/// Where an ack goes.
enum AckTo {
    Player(PlayerId),
    Conn(ConnId),
}

struct PendingAck {
    to: AckTo,
    id: i64,
    /// `Some` once answered; `None` at flush: `unknown-action`.
    result: Option<Value>,
    kind: String,
    key: u64,
}

/// Shared with the HTTP server: failed `hello`s per address.
pub(crate) type Joins = Arc<Mutex<WindowCounter>>;

/// The screen view for another thread (a renderer): updated by
/// [`Party::flush`].
#[derive(Clone, Debug)]
pub struct ScreenWatch(Arc<Mutex<(u64, Arc<Value>)>>);

impl ScreenWatch {
    /// The latest screen view (`{roster, screen, join}`) and its version
    /// (increases with every change).
    pub fn get(&self) -> (u64, Arc<Value>) {
        let g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        (g.0, g.1.clone())
    }
}

/// The party state without I/O: owned by [`Party`](crate::Party).
pub(crate) struct Core {
    pub code: String,
    pub join_key: String,
    pub screen_key: String,
    pub watch_key: String,
    pub room: State,
    pub screen: State,
    pub max_players: usize,
    limits: Limits,
    moderate: Option<Moderate>,
    join: Value,
    join_dirty: bool,
    roster_dirty: bool,
    players: Vec<Player>,
    clients: HashMap<ConnId, Client>,
    pending: HashMap<ConnId, Conn>,
    locked: bool,
    closed: bool,
    seq: u64,
    screen_seq: u64,
    action_seq: u64,
    acks: Vec<PendingAck>,
    events: VecDeque<Event>,
    joins: Joins,
    watch: ScreenWatch,
    watch_version: u64,
}

pub(crate) struct CoreOptions {
    pub code: Option<String>,
    pub max_players: usize,
    pub limits: Limits,
    pub moderate: Option<Moderate>,
}

impl Core {
    pub fn new(options: CoreOptions, joins: Joins) -> Core {
        let code = options
            .code
            .map(|c| c.to_ascii_uppercase())
            .unwrap_or_else(room_code);
        let join = Value::object()
            .with("code", code.as_str())
            .with("url", "")
            .with("qr", Value::Null);
        Core {
            code,
            join_key: token(),
            screen_key: token(),
            watch_key: token(),
            room: State::default(),
            screen: State::default(),
            max_players: options.max_players,
            limits: options.limits,
            moderate: options.moderate,
            join,
            join_dirty: true,
            roster_dirty: false,
            players: Vec::new(),
            clients: HashMap::new(),
            pending: HashMap::new(),
            locked: false,
            closed: false,
            seq: 0,
            screen_seq: 0,
            action_seq: 0,
            acks: Vec::new(),
            events: VecDeque::new(),
            joins,
            watch: ScreenWatch(Arc::new(Mutex::new((0, Arc::new(Value::Null))))),
            watch_version: 0,
        }
    }

    pub fn set_join(&mut self, join: Value) {
        self.join = join;
        self.join_dirty = true;
    }

    pub fn watch(&self) -> ScreenWatch {
        self.watch.clone()
    }

    pub fn players(&self) -> &[Player] {
        &self.players
    }

    pub fn player(&self, id: &PlayerId) -> Option<&Player> {
        self.players.iter().find(|p| &p.id == id)
    }

    pub fn player_mut(&mut self, id: &PlayerId) -> Option<&mut Player> {
        self.players.iter_mut().find(|p| &p.id == id)
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    pub fn lock(&mut self) {
        self.locked = true;
    }

    pub fn unlock(&mut self) {
        self.locked = false;
    }

    pub fn closed(&self) -> bool {
        self.closed
    }

    pub fn events(&mut self) -> Vec<Event> {
        self.events.drain(..).collect()
    }

    pub fn moderate(&self, text: &str, kind: &str) -> Moderation {
        match &self.moderate {
            Some(f) => f(text, kind),
            None => Moderation::Accept,
        }
    }

    // -------------------------------------------------------------------
    // Views

    fn roster(&self) -> Value {
        Value::Array(self.players.iter().map(Player::roster_entry).collect())
    }

    /// What screens see: `{roster, screen, join}`.
    pub fn screen_view(&self) -> Object {
        let mut o = Object::new();
        o.insert("roster", self.roster());
        o.insert("screen", Value::Object(self.screen.value.clone()));
        o.insert("join", self.join.clone());
        o
    }

    fn player_view(&self, p: &Player) -> Object {
        let mut o = Object::new();
        o.insert("roster", self.roster());
        o.insert("room", Value::Object(self.room.value.clone()));
        o.insert("me", Value::Object(p.state.value.clone()));
        o
    }

    fn view_for(&self, c: &Client) -> Object {
        match c.player.as_ref().and_then(|id| self.player(id)) {
            Some(p) => self.player_view(p),
            None => self.screen_view(),
        }
    }

    // -------------------------------------------------------------------
    // Sockets

    pub fn handle(&mut self, inbound: Inbound) {
        match inbound {
            Inbound::Opened { conn } => {
                if self.closed {
                    bye(&conn, Bye::RoomClosed, None);
                    return;
                }
                self.pending.insert(conn.id, conn);
            }
            Inbound::HelloTimeout { conn } => {
                if let Some(c) = self.pending.remove(&conn) {
                    self.failed_join(&c.address);
                    bye(&c, Bye::Protocol, Some("no hello"));
                }
            }
            Inbound::Message { conn, at, msg } => {
                if let Some(c) = self.pending.remove(&conn) {
                    self.hello(c, msg, at);
                } else if self.clients.contains_key(&conn) {
                    self.message(conn, msg, at);
                }
            }
            Inbound::Drained { conn } => {
                if self.clients.get(&conn).is_some_and(|c| c.waiting) {
                    self.flush_client(conn);
                }
            }
            Inbound::Closed { conn } => {
                self.pending.remove(&conn);
                if let Some(c) = self.clients.remove(&conn) {
                    self.client_gone(c);
                }
            }
        }
    }

    fn failed_join(&self, address: &str) {
        let mut j = self.joins.lock().unwrap_or_else(|e| e.into_inner());
        j.hit(address, Instant::now());
    }

    fn client_gone(&mut self, c: Client) {
        match c.player {
            Some(id) => {
                let Some(p) = self.player_mut(&id) else {
                    return;
                };
                if p.client == Some(c.conn.id) {
                    p.client = None;
                    let name = p.name.clone();
                    info(&format!("party {}: {name} is away", self.code));
                    self.events.push_back(Event::Away(id));
                    self.roster_dirty = true;
                }
            }
            None if c.role != Role::Player => self.events.push_back(Event::Screen {
                id: c.id,
                role: c.role,
                connected: false,
            }),
            None => {}
        }
    }

    fn hello(&mut self, conn: Conn, msg: Incoming, at: Instant) {
        let hello = match msg {
            Incoming::Text(t) => Value::parse(&t).ok(),
            Incoming::Binary => None,
        };
        let hello = match hello {
            Some(Value::Object(o))
                if o.get("t").and_then(Value::as_str) == Some("hello")
                    && o.get("v").and_then(Value::as_i64) == Some(PROTOCOL)
                    && o.get("v")
                        .and_then(Value::as_number)
                        .is_some_and(|n| n.is_integer()) =>
            {
                o
            }
            _ => {
                self.failed_join(&conn.address);
                return bye(&conn, Bye::Protocol, Some("expected hello, version 1"));
            }
        };
        let room = hello
            .get("room")
            .and_then(Value::as_str)
            .map(str::to_uppercase)
            .unwrap_or_default();
        if room != self.code {
            let shown: String = room.chars().take(8).collect();
            warn(&format!(
                "party server: hello for room {shown:?} from {}",
                conn.address
            ));
            self.failed_join(&conn.address);
            return bye(&conn, Bye::NoRoom, None);
        }
        if self.closed {
            return bye(&conn, Bye::RoomClosed, None);
        }
        let key = hello.get("key").and_then(Value::as_str);
        let role = hello.get("role").and_then(Value::as_str);
        match role {
            Some(r @ ("screen" | "watch")) => {
                let role = if r == "screen" {
                    Role::Screen
                } else {
                    Role::Watch
                };
                let want = if role == Role::Screen {
                    &self.screen_key
                } else {
                    &self.watch_key
                };
                if !key.is_some_and(|k| same_token(k, want)) {
                    warn(&format!(
                        "party {}: {r} with a wrong key from {}",
                        self.code, conn.address
                    ));
                    self.failed_join(&conn.address);
                    return bye(&conn, Bye::BadKey, None);
                }
                let screens = self
                    .clients
                    .values()
                    .filter(|c| c.role != Role::Player)
                    .count();
                if screens >= self.limits.screens {
                    return bye(&conn, Bye::Full, None);
                }
                self.screen_seq += 1;
                let id = format!("s{}", self.screen_seq);
                self.adopt(conn, role, id.clone(), None, at);
                self.events.push_back(Event::Screen {
                    id,
                    role,
                    connected: true,
                });
            }
            Some("player") => self.hello_player(conn, &hello, key, at),
            _ => bye(&conn, Bye::Protocol, None),
        }
    }

    fn hello_player(&mut self, conn: Conn, hello: &Object, key: Option<&str>, at: Instant) {
        // A known player coming back.
        if let Some(resume) = hello.get("resume").and_then(Value::as_str) {
            if let Some(i) = self
                .players
                .iter()
                .position(|p| same_token(resume, &p.resume))
            {
                let id = self.players[i].id.clone();
                let old = self.players[i].client.take();
                if let Some(old) = old.and_then(|o| self.clients.remove(&o)) {
                    bye(&old.conn, Bye::Replaced, None);
                }
                let conn_id = conn.id;
                self.players[i].client = Some(conn_id);
                let name = self.players[i].name.clone();
                self.adopt(
                    conn,
                    Role::Player,
                    id.as_str().to_owned(),
                    Some(id.clone()),
                    at,
                );
                info(&format!("party {}: {name} is back", self.code));
                self.events.push_back(Event::Back(id));
                self.roster_dirty = true;
                return;
            }
        }
        if !key.is_some_and(|k| same_token(k, &self.join_key)) {
            warn(&format!(
                "party {}: a join with a wrong key from {}",
                self.code, conn.address
            ));
            self.failed_join(&conn.address);
            return bye(&conn, Bye::BadKey, None);
        }
        let raw = match hello.get("name") {
            None | Some(Value::Null) => return bye(&conn, Bye::NoPlayer, None),
            Some(v) => v.as_str().unwrap_or(""),
        };
        if self.locked {
            return bye(&conn, Bye::Locked, None);
        }
        if self.players.len() >= self.max_players {
            return bye(&conn, Bye::Full, None);
        }
        let mut name = clean_text(raw, MAX_NAME);
        if name.is_empty() {
            return bye(&conn, Bye::NameInvalid, Some("Pick a name"));
        }
        match self.moderate(&name, "name") {
            Moderation::Refuse => return bye(&conn, Bye::NameInvalid, Some("Pick another name")),
            Moderation::Replace(n) => name = clean_text(&n, MAX_NAME),
            Moderation::Accept => {}
        }
        if name.is_empty() {
            return bye(&conn, Bye::NameInvalid, Some("Pick a name"));
        }
        let key = name_key(&name);
        if self.players.iter().any(|p| name_key(&p.name) == key) {
            return bye(&conn, Bye::NameTaken, Some("Someone has that name"));
        }
        self.seq += 1;
        let id = PlayerId(format!("p{}", self.seq));
        let vip = !self.players.iter().any(|p| p.vip);
        let address = conn.address.clone();
        self.players.push(Player {
            id: id.clone(),
            name: name.clone(),
            vip,
            state: State::default(),
            resume: token(),
            last_act: 0,
            results: VecDeque::new(),
            client: Some(conn.id),
        });
        self.adopt(
            conn,
            Role::Player,
            id.as_str().to_owned(),
            Some(id.clone()),
            at,
        );
        info(&format!("party {}: {name} joined ({address})", self.code));
        self.events.push_back(Event::Joined(id.clone()));
        if vip {
            self.events.push_back(Event::Vip(Some(id)));
        }
        self.roster_dirty = true;
    }

    /// Takes a socket after a good `hello`: sends `welcome` with the
    /// full view.
    fn adopt(&mut self, conn: Conn, role: Role, id: String, player: Option<PlayerId>, at: Instant) {
        let mut c = Client {
            conn,
            role,
            id,
            player,
            sent: Object::new(),
            rev: 0,
            waiting: false,
            bucket: TokenBucket::new(self.limits.rate, self.limits.burst, at),
            last_act: 0,
        };
        let view = self.view_for(&c);
        let mut you = Object::new();
        you.insert("id", c.id.as_str());
        you.insert("role", role.as_str());
        if let Some(p) = c.player.as_ref().and_then(|id| self.player(id)) {
            you.insert("name", p.name.as_str());
            you.insert("vip", p.vip);
            you.insert("resume", p.resume.as_str());
            you.insert("lastAct", p.last_act);
        }
        let mut msg = Object::new();
        msg.insert("t", "welcome");
        msg.insert("v", PROTOCOL);
        msg.insert("room", Value::object().with("code", self.code.as_str()));
        msg.insert("you", Value::Object(you));
        msg.insert("now", now_ms());
        msg.insert("view", Value::Object(view.clone()));
        msg.insert("rev", 0);
        c.sent = view;
        c.send(Value::Object(msg));
        self.clients.insert(c.conn.id, c);
    }

    fn message(&mut self, conn: ConnId, msg: Incoming, at: Instant) {
        let Some(c) = self.clients.get_mut(&conn) else {
            return;
        };
        if !c.bucket.take(at) {
            let c = self.drop_client(conn);
            return bye(&c.conn, Bye::RateLimited, None);
        }
        let text = match msg {
            Incoming::Text(t) => t,
            Incoming::Binary => {
                let c = self.drop_client(conn);
                return bye(&c.conn, Bye::Protocol, Some("binary messages are not used"));
            }
        };
        let msg = match Value::parse(&text) {
            Ok(Value::Object(o)) => o,
            Ok(_) => {
                let c = self.drop_client(conn);
                return bye(&c.conn, Bye::Protocol, Some("not an object"));
            }
            Err(_) => {
                let c = self.drop_client(conn);
                return bye(&c.conn, Bye::Protocol, Some("not JSON"));
            }
        };
        let t = msg.get("t").and_then(Value::as_str).unwrap_or("");
        if t != "act" && text.len() > MAX_CONTROL_MESSAGE {
            let c = self.drop_client(conn);
            return bye(&c.conn, Bye::Protocol, Some("too big"));
        }
        match t {
            "act" => self.act(conn, &msg),
            "ping" => {
                let echo = match msg.get("c") {
                    Some(v @ Value::Number(_)) => v.clone(),
                    _ => Value::from(0),
                };
                let pong = Value::object()
                    .with("t", "pong")
                    .with("c", echo)
                    .with("now", now_ms());
                if let Some(c) = self.clients.get(&conn) {
                    c.send(pong);
                }
            }
            "resync" => {
                let Some(c) = self.clients.get(&conn) else {
                    return;
                };
                let view = self.view_for(c);
                let Some(c) = self.clients.get_mut(&conn) else {
                    return;
                };
                c.rev += 1;
                c.sent = view.clone();
                let msg = Value::object()
                    .with("t", "state")
                    .with("rev", c.rev)
                    .with("full", Value::Object(view));
                c.send(msg);
            }
            "leave" => {
                let Some(c) = self.clients.remove(&conn) else {
                    return;
                };
                match &c.player {
                    Some(id) => {
                        if let Some(p) = self.player_mut(id) {
                            p.client = None;
                        }
                        self.remove(id, "left");
                    }
                    None if c.role != Role::Player => self.events.push_back(Event::Screen {
                        id: c.id.clone(),
                        role: c.role,
                        connected: false,
                    }),
                    None => {}
                }
                c.conn.close(1000, "left");
            }
            // Unknown types are ignored.
            _ => {}
        }
    }

    /// Forgets a client without the `away` bookkeeping of a lost socket
    /// (it is closed on purpose); a player becomes away.
    fn drop_client(&mut self, conn: ConnId) -> Client {
        let c = self
            .clients
            .remove(&conn)
            .expect("drop_client of a known client");
        if let Some(id) = &c.player {
            if let Some(p) = self.player_mut(id) {
                if p.client == Some(conn) {
                    p.client = None;
                    let id = id.clone();
                    self.events.push_back(Event::Away(id));
                    self.roster_dirty = true;
                }
            }
        } else if c.role != Role::Player {
            self.events.push_back(Event::Screen {
                id: c.id.clone(),
                role: c.role,
                connected: false,
            });
        }
        c
    }

    fn act(&mut self, conn: ConnId, msg: &Object) {
        let id = msg
            .get("id")
            .and_then(Value::as_number)
            .filter(|n| n.is_integer())
            .and_then(|n| n.as_i64())
            .filter(|&n| (1..=MAX_SAFE_INTEGER).contains(&n));
        let kind = msg.get("type").and_then(Value::as_str);
        let (Some(id), Some(kind)) = (id, kind) else {
            let c = self.drop_client(conn);
            return bye(&c.conn, Bye::Protocol, Some("bad act"));
        };
        let data = dedupe(msg.get("data").cloned().unwrap_or(Value::Null));
        let Some(c) = self.clients.get_mut(&conn) else {
            return;
        };
        if c.role == Role::Watch {
            let ack = ack_error(id, "read-only", "Watchers cannot act");
            self.acks.push(PendingAck {
                to: AckTo::Conn(conn),
                id,
                result: Some(ack),
                kind: kind.to_owned(),
                key: 0,
            });
            return;
        }
        let Some(pid) = c.player.clone() else {
            // A screen: deduplicated per connection.
            if id <= c.last_act {
                return;
            }
            c.last_act = id;
            let from = Source::Screen(c.id.clone());
            self.deliver(AckTo::Conn(conn), id, kind, data, from);
            return;
        };
        let Some(p) = self.player_mut(&pid) else {
            return;
        };
        if id <= p.last_act {
            // A replay after a reconnect: the cached result, once more.
            let cached = p
                .results
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, a)| a.clone());
            let pending = self
                .acks
                .iter()
                .any(|a| matches!(&a.to, AckTo::Player(x) if *x == pid) && a.id == id);
            let result = match cached {
                Some(a) => Some(a),
                // Answered at the flush with the original.
                None if pending => None,
                None => Some(
                    Value::object()
                        .with("t", "ack")
                        .with("id", id)
                        .with("ok", true)
                        .with("dup", true),
                ),
            };
            self.acks.push(PendingAck {
                to: AckTo::Player(pid),
                id,
                result,
                kind: String::new(),
                key: u64::MAX,
            });
            return;
        }
        p.last_act = id;
        self.deliver(
            AckTo::Player(pid.clone()),
            id,
            kind,
            data,
            Source::Player(pid),
        );
    }

    fn deliver(&mut self, to: AckTo, id: i64, kind: &str, data: Value, from: Source) {
        self.action_seq += 1;
        let key = self.action_seq;
        self.acks.push(PendingAck {
            to,
            id,
            result: None,
            kind: kind.to_owned(),
            key,
        });
        self.events.push_back(Event::Action(Action {
            key,
            id,
            kind: kind.to_owned(),
            data,
            from,
        }));
    }

    pub fn reply(&mut self, action: Action, result: Result<Value, PartyError>) {
        let ack = match result {
            Ok(v) => {
                let mut o = Object::new();
                o.insert("t", "ack");
                o.insert("id", action.id);
                o.insert("ok", true);
                if !v.is_null() {
                    o.insert("result", v);
                }
                Value::Object(o)
            }
            Err(e) => ack_error(action.id, &e.code, &e.message),
        };
        if let Some(p) = self.acks.iter_mut().find(|a| a.key == action.key) {
            p.result = Some(ack);
        }
    }

    // -------------------------------------------------------------------
    // The host

    pub fn send(&self, to: &PlayerId, kind: &str, data: Value) {
        let msg = Value::object()
            .with("t", "msg")
            .with("type", kind)
            .with("data", data);
        if let Some(c) = self
            .player(to)
            .and_then(|p| p.client)
            .and_then(|c| self.clients.get(&c))
        {
            c.send(msg);
        }
    }

    pub fn broadcast(&self, kind: &str, data: Value, roles: &[Role]) {
        let msg = Value::object()
            .with("t", "msg")
            .with("type", kind)
            .with("data", data);
        let mut clients: Vec<&Client> = self
            .clients
            .values()
            .filter(|c| roles.contains(&c.role))
            .collect();
        clients.sort_by_key(|c| c.conn.id);
        for c in clients {
            c.send(msg.clone());
        }
    }

    pub fn kick(&mut self, id: &PlayerId, reason: &str) {
        let Some(p) = self.player_mut(id) else {
            return;
        };
        if let Some(c) = p.client.take().and_then(|c| self.clients.remove(&c)) {
            bye(&c.conn, Bye::Kicked, Some(reason));
        }
        self.remove(id, "kicked");
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        let mut clients: Vec<Client> = self.clients.drain().map(|(_, c)| c).collect();
        clients.sort_by_key(|c| c.conn.id);
        for c in clients {
            bye(&c.conn, Bye::RoomClosed, None);
        }
        for p in &mut self.players {
            p.client = None;
        }
        for (_, c) in self.pending.drain() {
            bye(&c, Bye::RoomClosed, None);
        }
    }

    fn remove(&mut self, id: &PlayerId, reason: &str) {
        let Some(i) = self.players.iter().position(|p| &p.id == id) else {
            return;
        };
        let p = self.players.remove(i);
        if let Some(c) = p.client {
            if let Some(c) = self.clients.get_mut(&c) {
                c.player = None;
            }
        }
        info(&format!("party {}: {} left ({reason})", self.code, p.name));
        self.events.push_back(Event::Left {
            player: p.id.clone(),
            name: p.name.clone(),
            reason: reason.to_owned(),
        });
        if p.vip {
            let next =
                self.players
                    .iter()
                    .position(Player::connected)
                    .or(if self.players.is_empty() {
                        None
                    } else {
                        Some(0)
                    });
            if let Some(n) = next {
                self.players[n].vip = true;
            }
            let id = next.map(|n| self.players[n].id.clone());
            self.events.push_back(Event::Vip(id));
        }
        self.roster_dirty = true;
    }

    // -------------------------------------------------------------------
    // Flushing

    /// Sends what changed: one `state` diff per client, then the answers
    /// to the actions (unanswered ones: `unknown-action`).
    pub fn flush(&mut self) {
        let any_player = self.players.iter().any(|p| p.state.dirty);
        if self.room.dirty
            || self.screen.dirty
            || self.roster_dirty
            || self.join_dirty
            || any_player
        {
            let mut ids: Vec<ConnId> = self.clients.keys().copied().collect();
            ids.sort_unstable();
            for id in ids {
                self.flush_client(id);
            }
            if self.screen.dirty || self.roster_dirty || self.join_dirty {
                self.watch_version += 1;
                let view = Arc::new(Value::Object(self.screen_view()));
                *self.watch.0.lock().unwrap_or_else(|e| e.into_inner()) =
                    (self.watch_version, view);
            }
            self.room.dirty = false;
            self.screen.dirty = false;
            self.roster_dirty = false;
            self.join_dirty = false;
            for p in &mut self.players {
                p.state.dirty = false;
            }
        }
        self.send_acks();
    }

    fn flush_client(&mut self, id: ConnId) {
        let Some(c) = self.clients.get(&id) else {
            return;
        };
        if c.conn.closed() || (c.role == Role::Player && c.player.is_none()) {
            return;
        }
        if c.conn.buffered() > STATE_HIGH_WATER {
            // Latest wins: one diff against what it has once it drains.
            if let Some(c) = self.clients.get_mut(&id) {
                c.waiting = true;
            }
            return;
        }
        let view = self.view_for(c);
        let Some(c) = self.clients.get_mut(&id) else {
            return;
        };
        c.waiting = false;
        let Some(d) = diff(&c.sent, &view) else {
            return;
        };
        c.sent = view;
        c.rev += 1;
        let mut msg = Object::new();
        msg.insert("t", "state");
        msg.insert("rev", c.rev);
        d.write_into(&mut msg);
        c.send(Value::Object(msg));
    }

    fn send_acks(&mut self) {
        let acks = std::mem::take(&mut self.acks);
        let mut answered: HashMap<(String, i64), Value> = HashMap::new();
        for a in acks {
            let ack = match a.result {
                Some(r) => r,
                None if a.key == u64::MAX => {
                    // A replay of an action answered in this flush.
                    let pid = match &a.to {
                        AckTo::Player(p) => p.as_str().to_owned(),
                        AckTo::Conn(_) => continue,
                    };
                    match answered.get(&(pid, a.id)) {
                        Some(v) => v.clone(),
                        None => continue,
                    }
                }
                None => ack_error(a.id, "unknown-action", &format!("No {} now", a.kind)),
            };
            match &a.to {
                AckTo::Player(pid) => {
                    if a.key != u64::MAX {
                        answered.insert((pid.as_str().to_owned(), a.id), ack.clone());
                        if let Some(p) = self.player_mut(pid) {
                            p.results.push_back((a.id, ack.clone()));
                            while p.results.len() > ACK_CACHE {
                                p.results.pop_front();
                            }
                        }
                    }
                    // The player's current socket: it may have reconnected.
                    if let Some(c) = self
                        .player(pid)
                        .and_then(|p| p.client)
                        .and_then(|c| self.clients.get(&c))
                    {
                        c.send(ack);
                    }
                }
                AckTo::Conn(id) => {
                    if let Some(c) = self.clients.get(id) {
                        c.send(ack);
                    }
                }
            }
        }
    }
}

fn info(msg: &str) {
    tvpipe::log::write(
        tvpipe::log::Level::Info,
        "tvpipe_party",
        format_args!("{msg}"),
    );
}

fn warn(msg: &str) {
    tvpipe::log::write(
        tvpipe::log::Level::Warn,
        "tvpipe_party",
        format_args!("{msg}"),
    );
}

fn ack_error(id: i64, code: &str, message: &str) -> Value {
    Value::object()
        .with("t", "ack")
        .with("id", id)
        .with("ok", false)
        .with(
            "error",
            Value::object().with("code", code).with("message", message),
        )
}

/// The receiving end for connection threads.
pub(crate) type Inbox = mpsc::Receiver<Inbound>;

/// Drains `inbox` into `core` (without blocking), or waits up to `wait`
/// for the first message.
pub(crate) fn pump(core: &mut Core, inbox: &Inbox, wait: Option<Duration>) {
    if let Some(wait) = wait {
        match inbox.recv_timeout(wait) {
            Ok(m) => core.handle(m),
            Err(_) => return,
        }
    }
    while let Ok(m) = inbox.try_recv() {
        core.handle(m);
    }
}
