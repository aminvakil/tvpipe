//! One party: its players and screens, their views, actions and messages
//! (docs/party-design.md §5). The game owns the [`Party`](crate::Party) and drives it
//! from its own loop: [`Party::poll`](crate::Party::poll) (or [`Party::wait`](crate::Party::wait)) returns what
//! happened, [`Party::flush`](crate::Party::flush) sends what changed.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::conn::{Conn, ConnId, Inbound, Incoming, STATE_HIGH_WATER};
use crate::limits::{TokenBucket, WindowCounter};
use crate::names::{clean_text, name_key, MAX_NAME};
use crate::sync::{diff_at, json_equal};
use crate::value::{dedupe, Object, Value};

/// The protocol version (`hello.v`).
pub const PROTOCOL: i64 = 1;

/// Bytes queued to one socket before it is closed as too slow.
const SLOW_LIMIT: usize = 256 * 1024;
/// Results of earlier actions kept per player, for replays.
const ACK_CACHE: usize = 32;
/// Largest message other than `act`.
const MAX_CONTROL_MESSAGE: usize = 16 * 1024;
/// Largest `in` message (continuous input: a stick, a tilt).
const MAX_INPUT: usize = 4 * 1024;
/// `in` messages per second on the unreliable channel, and at once.
#[cfg(feature = "webrtc")]
const INPUT_RATE: (f64, f64) = (120.0, 60.0);
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
    InProgress,
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
            Bye::InProgress => "in-progress",
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
            Bye::InProgress => 4030,
        }
    }
}

/// The join info with `lateJoin` and `inProgress`.
fn join_value(join: &Value, late_join: LateJoin, in_progress: bool) -> Value {
    let mut o = match join {
        Value::Object(o) => o.clone(),
        _ => Object::new(),
    };
    o.insert("lateJoin", late_join.as_str());
    o.insert("inProgress", in_progress);
    Value::Object(o)
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
/// before a [`Party::flush`](crate::Party::flush) go out as one diff.
#[derive(Debug, Clone, Default)]
pub struct State {
    value: Object,
    dirty: bool,
    /// Counts the changes.
    version: u64,
    /// A copy as of a version, made once per version for the flush.
    snap: Option<(u64, Arc<Value>)>,
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
                self.version += 1;
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
                self.version += 1;
            }
            v => panic!("party state must be an object, not {v}"),
        }
    }

    /// Removes a top-level field.
    pub fn remove(&mut self, key: &str) {
        if self.value.remove(key).is_some() {
            self.dirty = true;
            self.version += 1;
        }
    }

    /// Changes the state in place.
    pub fn update(&mut self, f: impl FnOnce(&mut Object)) {
        f(&mut self.value);
        self.dirty = true;
        self.version += 1;
    }

    /// The state as of this version (a copy made once per version).
    fn snapshot(&mut self) -> Arc<Value> {
        match &self.snap {
            Some((v, value)) if *v == self.version => value.clone(),
            _ => {
                let value = Arc::new(Value::Object(self.value.clone()));
                self.snap = Some((self.version, value.clone()));
                value
            }
        }
    }
}

/// A part of a client's view (`roster`, `room`/`screen`, `me`/`join`) as
/// of a version.
#[derive(Debug, Clone)]
struct Section {
    key: &'static str,
    /// Names the source: sections of the same id and version are equal.
    id: String,
    version: u64,
    value: Arc<Value>,
}

/// A section's diff between two versions, serialized once per flush for
/// every client that needs it.
#[derive(Debug, Default)]
struct SectionDiff {
    /// `"key":{…}` for the `set` object, if anything is set.
    set: Option<String>,
    /// The `del` paths, each serialized.
    del: Vec<String>,
}

/// What the views cost: counters since the party started
/// ([`Party::stats`](crate::Party::stats)).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stats {
    /// Flushes that had something to send.
    pub flushes: u64,
    /// `state` messages sent.
    pub states: u64,
    /// Bytes of those messages.
    pub state_bytes: u64,
    /// Time spent building and sending them.
    pub flush_time: Duration,
}

/// Who may join while a game is in progress
/// ([`Party::set_in_progress`](crate::Party::set_in_progress)).
///
/// The two that let newcomers in differ only in what the game does with
/// them: the platform treats them alike (waiting players,
/// [`Player::waiting`]) and shows the setting to screens
/// (`view.join.lateJoin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LateJoin {
    /// New names are refused (`bye in-progress`, 4030); resumes still work.
    Closed,
    /// New names join as waiting players; the game deals them in later
    /// (drawbluff: at the next round).
    #[default]
    Wait,
    /// Like `Wait` for the platform; the game deals them in as soon as it
    /// can (drawbluff: at the next drawing).
    Open,
}

impl LateJoin {
    /// `closed`, `wait`, `open`.
    pub fn as_str(self) -> &'static str {
        match self {
            LateJoin::Closed => "closed",
            LateJoin::Wait => "wait",
            LateJoin::Open => "open",
        }
    }

    /// From `closed`, `wait`, `open`.
    pub fn parse(s: &str) -> Option<LateJoin> {
        match s {
            "closed" => Some(LateJoin::Closed),
            "wait" => Some(LateJoin::Wait),
            "open" => Some(LateJoin::Open),
            _ => None,
        }
    }
}

/// A player: a person with a phone, across reconnects.
#[derive(Debug)]
pub struct Player {
    id: PlayerId,
    name: String,
    vip: bool,
    waiting: bool,
    /// What only this player sees (`view.me`).
    pub state: State,
    resume: String,
    last_act: i64,
    results: VecDeque<(i64, Value)>,
    client: Option<ConnId>,
    /// The latest `in` since the last poll.
    input: Option<Value>,
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

    /// Joined while a game was in progress and not dealt in yet: the game
    /// decides when ([`Party::activate`](crate::Party::activate)). In the
    /// roster as `waiting: true`.
    pub fn waiting(&self) -> bool {
        self.waiting
    }

    fn roster_entry(&self) -> Value {
        let mut o = Object::new();
        o.insert("id", self.id.as_str());
        o.insert("name", self.name.as_str());
        o.insert("vip", self.vip);
        o.insert("connected", self.connected());
        if self.waiting {
            o.insert("waiting", true);
        }
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

/// An action from a phone or a screen. Answer it with [`Party::reply`](crate::Party::reply)
/// before the next [`Party::flush`](crate::Party::flush); one left unanswered is refused as
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

/// What happened, from [`Party::poll`](crate::Party::poll).
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
    /// A waiting player was dealt in ([`Party::activate`](crate::Party::activate)).
    Active(PlayerId),
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
    /// A player's continuous input (`in`: a stick, a tilt), the latest
    /// since the last poll; earlier ones of the same poll are dropped.
    /// Comes over the WebRTC fast path when there is one (unreliable,
    /// unordered), else over the WebSocket.
    Input {
        /// Whose.
        player: PlayerId,
        /// What the phone sent.
        data: Value,
    },
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

/// Checks names (and whatever the game passes to [`Party::moderate`](crate::Party::moderate)):
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
    /// The view this client has, by section.
    sent: Vec<Section>,
    rev: u64,
    waiting: bool,
    bucket: TokenBucket,
    /// Screens: action ids seen on this connection.
    last_act: i64,
    /// The last `in` sequence number on this connection.
    in_seq: i64,
    #[cfg(feature = "webrtc")]
    fast: ClientFast,
}

/// A client's WebRTC fast path.
#[cfg(feature = "webrtc")]
struct ClientFast {
    /// The driver, if the party has one.
    link: Option<crate::fast::driver::Link>,
    /// The WebSocket's id, the driver's name for the peer.
    peer: ConnId,
    /// The client said `caps: ["rtc"]`.
    capable: bool,
    /// An offer went to the driver; the channels are not open yet.
    pending: bool,
    /// The channels are open: messages to the client go on `r`.
    open: bool,
    /// The stack failed on this client's input: no more offers.
    refused: bool,
    /// `in` on the unreliable channel.
    bucket: TokenBucket,
}

impl Client {
    fn send(&self, msg: Value) -> bool {
        self.send_text(msg.to_string())
    }

    /// A message already serialized.
    fn send_text(&self, text: String) -> bool {
        if self.conn.closed() {
            return false;
        }
        #[cfg(feature = "webrtc")]
        if self.fast.open {
            if let Some(link) = &self.fast.link {
                if link.send(crate::fast::driver::Cmd::Send {
                    peer: self.conn.id,
                    channel: crate::fast::Channel::Reliable,
                    text: text.clone(),
                }) {
                    return true;
                }
            }
        }
        if self.conn.buffered() > SLOW_LIMIT {
            bye(&self.conn, Bye::Slow, None);
            return false;
        }
        self.conn.send(text);
        true
    }
}

#[cfg(feature = "webrtc")]
impl Drop for ClientFast {
    /// The fast path lives as long as the WebSocket that signaled it.
    fn drop(&mut self) {
        if self.pending || self.open {
            if let Some(link) = &self.link {
                link.send(crate::fast::driver::Cmd::Close { peer: self.peer });
            }
        }
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
/// [`Party::flush`](crate::Party::flush).
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
    join_snap: (u64, Arc<Value>),
    roster_dirty: bool,
    roster_snap: (u64, Arc<Value>),
    stats: Stats,
    players: Vec<Player>,
    clients: HashMap<ConnId, Client>,
    pending: HashMap<ConnId, Conn>,
    locked: bool,
    late_join: LateJoin,
    in_progress: bool,
    closed: bool,
    seq: u64,
    screen_seq: u64,
    action_seq: u64,
    acks: Vec<PendingAck>,
    events: VecDeque<Event>,
    joins: Joins,
    watch: ScreenWatch,
    watch_version: u64,
    /// The fast path's driver, if it runs.
    #[cfg(feature = "webrtc")]
    pub fast: Option<crate::fast::driver::Link>,
}

pub(crate) struct CoreOptions {
    pub code: Option<String>,
    pub max_players: usize,
    pub limits: Limits,
    pub moderate: Option<Moderate>,
    pub late_join: LateJoin,
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
            join_snap: (1, Arc::new(join_value(&join, options.late_join, false))),
            join,
            join_dirty: true,
            roster_dirty: false,
            roster_snap: (0, Arc::new(Value::Array(Vec::new()))),
            stats: Stats::default(),
            players: Vec::new(),
            clients: HashMap::new(),
            pending: HashMap::new(),
            locked: false,
            late_join: options.late_join,
            in_progress: false,
            closed: false,
            seq: 0,
            screen_seq: 0,
            action_seq: 0,
            acks: Vec::new(),
            events: VecDeque::new(),
            joins,
            watch: ScreenWatch(Arc::new(Mutex::new((0, Arc::new(Value::Null))))),
            watch_version: 0,
            #[cfg(feature = "webrtc")]
            fast: None,
        }
    }

    pub fn set_join(&mut self, join: Value) {
        self.join = join;
        self.join_changed();
    }

    /// The join section: the join info, the late-join setting, whether a
    /// game is on.
    fn join_changed(&mut self) {
        let value = join_value(&self.join, self.late_join, self.in_progress);
        self.join_snap = (self.join_snap.0 + 1, Arc::new(value));
        self.join_dirty = true;
    }

    pub fn late_join(&self) -> LateJoin {
        self.late_join
    }

    pub fn set_late_join(&mut self, mode: LateJoin) {
        if mode != self.late_join {
            self.late_join = mode;
            self.join_changed();
        }
    }

    pub fn in_progress(&self) -> bool {
        self.in_progress
    }

    pub fn set_in_progress(&mut self, on: bool) {
        if on != self.in_progress {
            self.in_progress = on;
            self.join_changed();
        }
    }

    /// Deals a waiting player in; false if they were not waiting.
    pub fn activate(&mut self, id: &PlayerId) -> bool {
        let Some(p) = self.players.iter_mut().find(|p| &p.id == id) else {
            return false;
        };
        if !p.waiting {
            return false;
        }
        p.waiting = false;
        self.events.push_back(Event::Active(id.clone()));
        self.roster_dirty = true;
        true
    }

    /// What the views cost so far.
    pub fn stats(&self) -> Stats {
        self.stats
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

    /// What happened: the latest inputs first (an action sent after an
    /// `in` sees it), then the rest in order.
    pub fn events(&mut self) -> Vec<Event> {
        let mut out: Vec<Event> = Vec::with_capacity(self.events.len());
        for p in &mut self.players {
            if let Some(data) = p.input.take() {
                out.push(Event::Input {
                    player: p.id.clone(),
                    data,
                });
            }
        }
        out.extend(self.events.drain(..));
        out
    }

    /// Whether this player's phone has an open fast path.
    #[cfg(feature = "webrtc")]
    pub fn fast_path(&self, id: &PlayerId) -> bool {
        self.player(id)
            .and_then(|p| p.client)
            .and_then(|c| self.clients.get(&c))
            .is_some_and(|c| c.fast.open)
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
        o.insert("join", (*self.join_snap.1).clone());
        o
    }

    /// The roster's snapshot: a new version only when it changed.
    fn refresh_roster(&mut self) {
        let roster = self.roster();
        if !json_equal(&roster, &self.roster_snap.1) {
            self.roster_snap = (self.roster_snap.0 + 1, Arc::new(roster));
        }
    }

    /// A client's view as sections (a player's, or a screen's without
    /// one); call [`Core::refresh_roster`] first.
    fn sections(&mut self, player: Option<&PlayerId>) -> Vec<Section> {
        let roster = Section {
            key: "roster",
            id: "roster".to_owned(),
            version: self.roster_snap.0,
            value: self.roster_snap.1.clone(),
        };
        if let Some(i) = player.and_then(|id| self.players.iter().position(|p| &p.id == id)) {
            let room = Section {
                key: "room",
                id: "room".to_owned(),
                version: self.room.version,
                value: self.room.snapshot(),
            };
            let p = &mut self.players[i];
            let me = Section {
                key: "me",
                id: format!("me:{}", p.id.as_str()),
                version: p.state.version,
                value: p.state.snapshot(),
            };
            return vec![roster, room, me];
        }
        let screen = Section {
            key: "screen",
            id: "screen".to_owned(),
            version: self.screen.version,
            value: self.screen.snapshot(),
        };
        let join = Section {
            key: "join",
            id: "join".to_owned(),
            version: self.join_snap.0,
            value: self.join_snap.1.clone(),
        };
        vec![roster, screen, join]
    }

    // -------------------------------------------------------------------
    // Sockets

    pub fn handle(&mut self, inbound: Inbound) {
        match inbound {
            Inbound::Wake => {}
            #[cfg(feature = "webrtc")]
            Inbound::Fast(up) => self.fast_up(up),
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
                self.adopt(conn, role, id.clone(), None, &hello, at);
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
                    hello,
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
        if self.in_progress && self.late_join == LateJoin::Closed {
            return bye(&conn, Bye::InProgress, Some("A game is in progress"));
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
            waiting: self.in_progress,
            state: State::default(),
            resume: token(),
            last_act: 0,
            results: VecDeque::new(),
            client: Some(conn.id),
            input: None,
        });
        self.adopt(
            conn,
            Role::Player,
            id.as_str().to_owned(),
            Some(id.clone()),
            hello,
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
    fn adopt(
        &mut self,
        conn: Conn,
        role: Role,
        id: String,
        player: Option<PlayerId>,
        hello: &Object,
        at: Instant,
    ) {
        #[cfg(not(feature = "webrtc"))]
        let _ = hello;
        #[cfg(feature = "webrtc")]
        let peer = conn.id;
        let mut c = Client {
            conn,
            role,
            id,
            player,
            sent: Vec::new(),
            rev: 0,
            waiting: false,
            bucket: TokenBucket::new(self.limits.rate, self.limits.burst, at),
            last_act: 0,
            in_seq: 0,
            #[cfg(feature = "webrtc")]
            fast: ClientFast {
                link: self.fast.clone(),
                peer,
                capable: role == Role::Player
                    && hello
                        .get("caps")
                        .and_then(Value::as_array)
                        .is_some_and(|caps| caps.iter().any(|c| c.as_str() == Some("rtc"))),
                pending: false,
                open: false,
                refused: false,
                bucket: TokenBucket::new(INPUT_RATE.0, INPUT_RATE.1, at),
            },
        };
        self.refresh_roster();
        let sections = self.sections(c.player.as_ref());
        let view = view_of(&sections);
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
        msg.insert("view", Value::Object(view));
        msg.insert("rev", 0);
        #[cfg(feature = "webrtc")]
        if c.fast.capable && c.fast.link.is_some() {
            msg.insert("rtc", true);
        }
        c.sent = sections;
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
        if t == "in" && text.len() > MAX_INPUT {
            return;
        }
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
                let player = c.player.clone();
                self.refresh_roster();
                let sections = self.sections(player.as_ref());
                let view = view_of(&sections);
                let Some(c) = self.clients.get_mut(&conn) else {
                    return;
                };
                c.rev += 1;
                c.sent = sections;
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
            "in" => self.input(conn, &msg),
            #[cfg(feature = "webrtc")]
            "rtc" => self.rtc(conn, &msg),
            // Unknown types are ignored.
            _ => {}
        }
    }

    /// `{t: "in", s, data}`: the latest input wins, by `s` (per
    /// connection); never answered. Players only.
    fn input(&mut self, conn: ConnId, msg: &Object) {
        let Some(c) = self.clients.get_mut(&conn) else {
            return;
        };
        let s = msg
            .get("s")
            .and_then(Value::as_number)
            .filter(|n| n.is_integer())
            .and_then(|n| n.as_i64())
            .filter(|&n| (1..=MAX_SAFE_INTEGER).contains(&n));
        let (Some(s), Some(pid)) = (s, c.player.clone()) else {
            return;
        };
        if s <= c.in_seq {
            return;
        }
        c.in_seq = s;
        let data = dedupe(msg.get("data").cloned().unwrap_or(Value::Null));
        if let Some(p) = self.player_mut(&pid) {
            p.input = Some(data);
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
            let started = Instant::now();
            self.refresh_roster();
            let mut diffs = HashMap::new();
            for id in ids {
                self.flush_client_with(id, &mut diffs);
            }
            self.stats.flushes += 1;
            self.stats.flush_time += started.elapsed();
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
        let started = Instant::now();
        self.refresh_roster();
        self.flush_client_with(id, &mut HashMap::new());
        self.stats.flush_time += started.elapsed();
    }

    /// Sends a client the diff of its view: only the sections whose
    /// version moved, each section's diff computed and serialized once
    /// per flush for every client that had the same version (`diffs`).
    /// The text is what serializing `{t, rev, set, del}` of the whole
    /// view's diff gives.
    fn flush_client_with(&mut self, id: ConnId, diffs: &mut HashMap<(String, u64), SectionDiff>) {
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
        let player = c.player.clone();
        let sections = self.sections(player.as_ref());
        let Some(c) = self.clients.get_mut(&id) else {
            return;
        };
        c.waiting = false;
        let mut set: Vec<String> = Vec::new();
        let mut del: Vec<String> = Vec::new();
        for (i, s) in sections.into_iter().enumerate() {
            let Some(old) = c.sent.get(i) else {
                continue;
            };
            if old.id == s.id && old.version == s.version {
                continue;
            }
            let key = (old.id.clone(), old.version);
            let d = diffs
                .entry(key)
                .or_insert_with(|| section_diff(s.key, &old.value, &s.value));
            set.extend(d.set.iter().cloned());
            del.extend(d.del.iter().cloned());
            c.sent[i] = s;
        }
        if set.is_empty() && del.is_empty() {
            return;
        }
        c.rev += 1;
        let mut text = format!("{{\"t\":\"state\",\"rev\":{}", c.rev);
        if !set.is_empty() {
            text.push_str(",\"set\":{");
            text.push_str(&set.join(","));
            text.push('}');
        }
        if !del.is_empty() {
            text.push_str(",\"del\":[");
            text.push_str(&del.join(","));
            text.push(']');
        }
        text.push('}');
        let bytes = text.len() as u64;
        c.send_text(text);
        self.stats.states += 1;
        self.stats.state_bytes += bytes;
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

/// The fast path (docs/multiscreen-design.md §2.4): signaling over the
/// WebSocket, the driver's reports.
#[cfg(feature = "webrtc")]
impl Core {
    /// `{t: "rtc", sdp}`: an offer (answered by [`Up::Answer`]);
    /// `{t: "rtc", state: "failed"}`: the client gave up (logged).
    fn rtc(&mut self, conn: ConnId, msg: &Object) {
        use crate::fast::driver::Cmd;
        let code = self.code.clone();
        let Some(c) = self.clients.get_mut(&conn) else {
            return;
        };
        if let Some(sdp) = msg.get("sdp").and_then(Value::as_str) {
            let link = match &c.fast.link {
                Some(l) if c.fast.capable && !c.fast.refused => l.clone(),
                _ => return c.conn.send(rtc_state("failed")),
            };
            // A new offer replaces the old connection: nothing more on it.
            c.fast.open = false;
            c.fast.pending = true;
            let offer = Cmd::Offer {
                peer: conn,
                sdp: sdp.to_owned(),
            };
            if !link.send(offer) {
                c.fast.pending = false;
                c.conn.send(rtc_state("failed"));
            }
        } else if let Some(state) = msg.get("state").and_then(Value::as_str) {
            let state: String = state.chars().take(32).collect();
            info(&format!("party {code}: {}'s fast path: {state:?}", c.id));
        }
    }

    fn fast_up(&mut self, up: crate::fast::driver::Up) {
        use crate::fast::driver::{Cmd, Up};
        use crate::fast::Channel;
        let peer = match &up {
            Up::Answer { peer, .. } | Up::Open(peer) | Up::Down { peer, .. } => *peer,
            Up::Message { peer, .. } => *peer,
        };
        let code = self.code.clone();
        let Some(c) = self.clients.get_mut(&peer) else {
            // The socket went first; the driver may still have the peer.
            if let Some(link) = &self.fast {
                link.send(Cmd::Close { peer });
            }
            return;
        };
        match up {
            Up::Answer { sdp: Ok(sdp), .. } if c.fast.pending => {
                // Over the WebSocket: `r` is not open (or being replaced).
                c.conn.send(
                    Value::object()
                        .with("t", "rtc")
                        .with("sdp", sdp)
                        .to_string(),
                );
            }
            Up::Answer { sdp: Ok(_), .. } => {}
            Up::Answer { sdp: Err(e), .. } => {
                c.fast.pending = false;
                info(&format!("party {code}: no fast path for {}: {e}", c.id));
                c.conn.send(rtc_state("failed"));
            }
            Up::Open(_) => {
                c.fast.pending = false;
                c.fast.open = true;
                info(&format!("party {code}: fast path open for {}", c.id));
            }
            Up::Down { why, .. } => {
                let was = c.fast.open || c.fast.pending;
                c.fast.pending = false;
                c.fast.open = false;
                if why == crate::fast::rtc::CRASHED {
                    c.fast.refused = true;
                    warn(&format!(
                        "party {code}: the fast path failed on {}'s data; WebSocket only from now",
                        c.id
                    ));
                } else {
                    info(&format!("party {code}: fast path down for {}: {why}", c.id));
                }
                if was {
                    c.conn.send(rtc_state("closed"));
                }
            }
            Up::Message {
                channel: Channel::Reliable,
                text,
                at,
                ..
            } => {
                if c.fast.open {
                    self.message(peer, Incoming::Text(text), at);
                }
            }
            Up::Message {
                channel: Channel::Unreliable,
                text,
                at,
                ..
            } => {
                if !c.fast.open || text.len() > MAX_INPUT || !c.fast.bucket.take(at) {
                    return;
                }
                if let Ok(Value::Object(msg)) = Value::parse(&text) {
                    if msg.get("t").and_then(Value::as_str) == Some("in") {
                        self.input(peer, &msg);
                    }
                }
            }
        }
    }
}

/// `{t: "rtc", state}` as text.
#[cfg(feature = "webrtc")]
fn rtc_state(state: &str) -> String {
    Value::object()
        .with("t", "rtc")
        .with("state", state)
        .to_string()
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

/// A view from its sections (for `welcome` and `resync`).
fn view_of(sections: &[Section]) -> Object {
    let mut o = Object::new();
    for s in sections {
        o.insert(s.key, (*s.value).clone());
    }
    o
}

/// One section's diff, serialized: objects diff key by key, anything else
/// is set when it changed (the rules of `sync::diff` at the top level).
fn section_diff(key: &'static str, old: &Value, new: &Value) -> SectionDiff {
    let name = Value::from(key).to_string();
    match (old, new) {
        (Value::Object(a), Value::Object(b)) => {
            let d = diff_at(a, b, key);
            SectionDiff {
                set: (!d.set.is_empty()).then(|| format!("{name}:{}", Value::Object(d.set))),
                del: d
                    .del
                    .into_iter()
                    .map(|p| Value::Array(p.into_iter().map(Value::from).collect()).to_string())
                    .collect(),
            }
        }
        _ if json_equal(old, new) => SectionDiff::default(),
        _ => SectionDiff {
            set: Some(format!("{name}:{new}")),
            del: Vec::new(),
        },
    }
}
