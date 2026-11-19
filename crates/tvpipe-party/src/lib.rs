//! Party games on tvpipe, hosted in Rust (docs/multiscreen-design.md).
//!
//! Phones join with a QR code on the TV; the host keeps the room, its
//! players and what each of them sees in sync over WebSockets, speaking
//! the protocol of the `@tvpipe/party` npm package (the same browser
//! client, the same test vectors).
//!
//! The game owns the [`Party`] and drives it from its own loop: no async
//! runtime, no callbacks on foreign threads.
//!
//! ```no_run
//! use std::time::Duration;
//! use tvpipe_party::{json, Event, Join, Party, PartyError, PartyOptions};
//!
//! static PHONE: tvpipe_party::Assets = tvpipe_party::Assets::new("", &[]);
//!
//! let mut party = Party::start(PartyOptions::new().phone(&PHONE).join(Join::Local))?;
//! println!("join: {}", party.join_url());
//! party.room().set(json!({ "phase": "lobby" }));
//! loop {
//!     for event in party.wait(Duration::from_millis(50)) {
//!         if let Event::Action(a) = event {
//!             let reply = match a.kind() {
//!                 "hello" => Ok(json!("hi")),
//!                 _ => Err(PartyError::new("unknown-action", "Not now")),
//!             };
//!             party.reply(a, reply);
//!         }
//!     }
//!     party.flush();
//! }
//! # Ok::<(), tvpipe::Error>(())
//! ```

mod assets;
mod collect;
mod conn;
mod limits;
mod names;
mod party;
mod server;
mod sync;
#[cfg(test)]
mod testutil;
#[cfg(feature = "deser")]
mod typed;
mod value;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use tvpipe::__web::{httpd, iface};
use tvpipe::{QrCode, Tunnel, TunnelProvider};

pub use assets::{Asset, Assets};
pub use collect::Collect;
pub use names::{clean_text, fold, name_key, MAX_NAME};
pub use party::{
    Action, Event, Limits, Moderate, Moderation, PartyError, Player, PlayerId, Role, ScreenWatch,
    Source, State, PROTOCOL,
};
pub use server::DEFAULT_CSP;
#[cfg(feature = "deser")]
pub use typed::{from_value, to_value};
pub use value::{Number, Object, Value};

use limits::WindowCounter;
use party::{Core, CoreOptions, Inbox};
use server::{Listener, Shared};

/// The browser client (`/party/client.js`), shared with `@tvpipe/party`.
pub const CLIENT_JS: &str = include_str!("../web/party-client.js");

/// How phones reach the party.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Join {
    /// A public link (Cloudflare, else OpenTunnel), or the LAN link when
    /// no tunnel comes up in time (offline): the default.
    #[default]
    Auto,
    /// A public link through this provider.
    Tunnel(TunnelProvider),
    /// The local network only: `http://IP:PORT` on this address (`None`:
    /// the default route's). Plain HTTP: not a secure context, and anyone
    /// who can read the Wi-Fi can join (docs/multiscreen-design.md §3.2).
    Lan(Option<IpAddr>),
    /// This computer only (`http://127.0.0.1:PORT`): development, tests.
    Local,
}

/// How phones reached the party in the end (the TV says so).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Link {
    /// A public HTTPS link through a tunnel.
    Tunnel,
    /// `http://IP:PORT` on the local network: phones must be on the same
    /// Wi-Fi.
    Lan,
    /// This computer only.
    Local,
}

impl Link {
    /// `tunnel`, `lan` or `local` (the `mode` of the screens' `join`).
    pub fn as_str(self) -> &'static str {
        match self {
            Link::Tunnel => "tunnel",
            Link::Lan => "lan",
            Link::Local => "local",
        }
    }
}

/// The LAN port tried first.
pub const LAN_PORT: u16 = 7420;

/// How a [`Party`] starts.
pub struct PartyOptions {
    phone: Option<&'static Assets>,
    screen: Option<&'static Assets>,
    join: Join,
    max_players: usize,
    limits: Limits,
    code: Option<String>,
    port: u16,
    lan_port: u16,
    tunnel_timeout: Duration,
    secure: bool,
    csp: Option<String>,
    max_sockets: usize,
    max_message: usize,
    joins_per_minute: usize,
    moderate: Option<Moderate>,
}

impl Default for PartyOptions {
    fn default() -> PartyOptions {
        PartyOptions::new()
    }
}

impl PartyOptions {
    /// The defaults: a public link (`Join::Auto`), at most 8 players.
    pub fn new() -> PartyOptions {
        PartyOptions {
            phone: None,
            screen: None,
            join: Join::Auto,
            max_players: 8,
            limits: Limits::default(),
            code: None,
            port: 0,
            lan_port: LAN_PORT,
            tunnel_timeout: Duration::from_secs(10),
            secure: false,
            csp: Some(DEFAULT_CSP.to_owned()),
            max_sockets: 64,
            max_message: 256 * 1024,
            joins_per_minute: 20,
            moderate: None,
        }
    }

    /// The phone pages, served at `/`.
    pub fn phone(mut self, assets: &'static Assets) -> Self {
        self.phone = Some(assets);
        self
    }

    /// The screen pages (a web screen), served at `/screen/`.
    pub fn screen(mut self, assets: &'static Assets) -> Self {
        self.screen = Some(assets);
        self
    }

    /// How phones reach the party.
    pub fn join(mut self, join: Join) -> Self {
        self.join = join;
        self
    }

    /// Players at most (default 8).
    pub fn max_players(mut self, n: usize) -> Self {
        self.max_players = n;
        self
    }

    /// Rate limits and the number of screens.
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The room code (default: four random consonants).
    pub fn code(mut self, code: &str) -> Self {
        self.code = Some(code.to_owned());
        self
    }

    /// The port on 127.0.0.1 (default 0: any free one).
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// The LAN port (default [`LAN_PORT`], the next free one if taken; 0:
    /// any).
    pub fn lan_port(mut self, port: u16) -> Self {
        self.lan_port = port;
        self
    }

    /// How long [`Join::Auto`] waits for a tunnel before it falls back to
    /// the LAN link (default 10 s).
    pub fn tunnel_timeout(mut self, timeout: Duration) -> Self {
        self.tunnel_timeout = timeout;
        self
    }

    /// The phone pages need a secure context (camera, motion sensors,
    /// wake lock, `crypto.subtle`): plain-HTTP LAN links are refused, so
    /// [`Join::Lan`] fails to start and [`Join::Auto`] does not fall back.
    pub fn require_secure_context(mut self) -> Self {
        self.secure = true;
        self
    }

    /// The Content-Security-Policy of HTML pages (`None`: none).
    pub fn csp(mut self, csp: Option<&str>) -> Self {
        self.csp = csp.map(str::to_owned);
        self
    }

    /// WebSockets at once (default 64).
    pub fn max_sockets(mut self, n: usize) -> Self {
        self.max_sockets = n;
        self
    }

    /// The largest message from a client (default 256 KiB).
    pub fn max_message(mut self, bytes: usize) -> Self {
        self.max_message = bytes;
        self
    }

    /// Failed `hello`s per address and minute before it is refused
    /// (default 20).
    pub fn joins_per_minute(mut self, n: usize) -> Self {
        self.joins_per_minute = n;
        self
    }

    /// Checks names (and what the game passes to [`Party::moderate`]).
    pub fn moderate(mut self, f: impl Fn(&str, &str) -> Moderation + Send + 'static) -> Self {
        self.moderate = Some(Box::new(f));
        self
    }
}

/// A party: the HTTP and WebSocket server, its link and QR code, the room
/// and its players. Dropping it closes everything.
pub struct Party {
    core: Core,
    inbox: Inbox,
    // The HTTP handler's state (public hosts, limits).
    _shared: Arc<Shared>,
    server: httpd::Server,
    lan_server: Option<httpd::Server>,
    tunnel: Option<Tunnel>,
    local: String,
    /// The tunnel's or the LAN's origin.
    public: Option<String>,
    link: Link,
    qr: Option<QrCode>,
}

impl std::fmt::Debug for Party {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Party")
            .field("code", &self.core.code)
            .field("local", &self.local)
            .field("public", &self.public)
            .finish_non_exhaustive()
    }
}

impl Party {
    /// Starts the server (and the tunnel, which may take several
    /// seconds) and makes the room.
    pub fn start(options: PartyOptions) -> tvpipe::Result<Party> {
        let (tx, inbox) = mpsc::channel();
        let joins = Arc::new(Mutex::new(WindowCounter::new(
            options.joins_per_minute,
            Duration::from_secs(60),
        )));
        let shared = Arc::new(Shared {
            port: OnceLock::new(),
            lan: OnceLock::new(),
            hosts: RwLock::new(Vec::new()),
            visitor: RwLock::new(None),
            joins: joins.clone(),
            sockets: Arc::new(AtomicUsize::new(0)),
            next_conn: AtomicU64::new(0),
            inbox: Mutex::new(tx),
            phone: options.phone,
            screen: options.screen,
            csp: options.csp,
            max_sockets: options.max_sockets,
            max_message: options.max_message,
        });
        let config = httpd::Config {
            // Sockets, plus room for page loads.
            max_connections: options.max_sockets + 32,
            ..httpd::Config::default()
        };
        if options.secure && matches!(options.join, Join::Lan(_)) {
            return Err(tvpipe::Error::usage(
                "party: the phone pages need a secure context (HTTPS); a LAN link is plain HTTP",
            ));
        }
        let server = httpd::Server::bind_with(
            SocketAddr::from((Ipv4Addr::LOCALHOST, options.port)),
            server::handler(shared.clone(), Listener::Loopback),
            config,
        )?;
        let port = server.local_addr().port();
        let _ = shared.port.set(port);
        let core = Core::new(
            CoreOptions {
                code: options.code,
                max_players: options.max_players,
                limits: options.limits,
                moderate: options.moderate,
            },
            joins,
        );
        let (tunnel, lan) = match options.join {
            Join::Local => (None, None),
            Join::Tunnel(p) => (Some(Tunnel::start(port, p)?), None),
            Join::Lan(ip) => (None, Some(ip)),
            Join::Auto => match auto_tunnel(port, options.tunnel_timeout) {
                Ok(t) => (Some(t), None),
                Err(e) if options.secure => return Err(e),
                Err(e) => {
                    tvpipe::log::write(
                        tvpipe::log::Level::Warn,
                        "tvpipe_party",
                        format_args!("party: no tunnel ({}); phones join on the LAN", e.message()),
                    );
                    (None, Some(None))
                }
            },
        };
        let lan_server = match lan {
            Some(ip) => {
                let ip = match ip {
                    Some(ip) => ip,
                    None => IpAddr::V4(iface::default_ipv4()?),
                };
                let s = bind_lan(ip, options.lan_port, &shared, config)?;
                let _ = shared.lan.set(s.local_addr());
                Some(s)
            }
            None => None,
        };
        let (public, link) = match (&tunnel, &lan_server) {
            (Some(t), _) => {
                shared
                    .hosts
                    .write()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(t.host().to_ascii_lowercase());
                (Some(format!("https://{}", t.host())), Link::Tunnel)
            }
            (None, Some(s)) => (Some(format!("http://{}", s.local_addr())), Link::Lan),
            (None, None) => (None, Link::Local),
        };
        let mut party = Party {
            core,
            inbox,
            _shared: shared,
            server,
            lan_server,
            tunnel,
            local: format!("http://127.0.0.1:{port}"),
            public,
            link,
            qr: None,
        };
        let url = party.join_url();
        party.qr = QrCode::encode(&url);
        let qr = party.qr.as_ref().map_or(Value::Null, |q| {
            Value::object()
                .with("size", q.size())
                .with("rows", Value::array(q.rows()))
        });
        let join = Value::object()
            .with("code", party.core.code.as_str())
            .with("url", url)
            .with("qr", qr)
            .with("mode", link.as_str());
        party.core.set_join(join);
        party.core.flush();
        Ok(party)
    }

    /// The room code.
    pub fn code(&self) -> &str {
        &self.core.code
    }

    /// The key phones join with (in the join link's fragment): for
    /// clients the game runs itself, such as bots.
    pub fn join_key(&self) -> &str {
        &self.core.join_key
    }

    /// The most players ([`PartyOptions::max_players`]).
    pub fn max_players(&self) -> usize {
        self.core.max_players
    }

    /// The port on 127.0.0.1.
    pub fn port(&self) -> u16 {
        self.server.local_addr().port()
    }

    /// The join link for the QR code and the TV:
    /// `https://HOST/?r=CODE#k=KEY` through a tunnel,
    /// `http://IP:PORT/?r=CODE#k=KEY` on the LAN, or the local one.
    pub fn join_url(&self) -> String {
        let base = self.public.as_deref().unwrap_or(&self.local);
        format!("{base}/?r={}#k={}", self.core.code, self.core.join_key)
    }

    /// The join link on this computer (`http://127.0.0.1:PORT/…`).
    pub fn local_join_url(&self) -> String {
        format!(
            "{}/?r={}#k={}",
            self.local, self.core.code, self.core.join_key
        )
    }

    /// The web screen on this computer, with the screen key (cast it with
    /// `tvpipe::share`; it never leaves the machine).
    pub fn screen_url(&self) -> String {
        format!(
            "{}/screen/#r={}&k={}&role=screen",
            self.local, self.core.code, self.core.screen_key
        )
    }

    /// The read-only screen for other devices, through the tunnel or on
    /// the LAN (`None` for a local party).
    pub fn watch_url(&self) -> Option<String> {
        self.public.as_ref().map(|p| {
            format!(
                "{p}/screen/#r={}&k={}&role=watch",
                self.core.code, self.core.watch_key
            )
        })
    }

    /// The QR code of [`join_url`](Party::join_url).
    pub fn join_qr(&self) -> Option<&QrCode> {
        self.qr.as_ref()
    }

    /// How phones reach the party.
    pub fn link(&self) -> Link {
        self.link
    }

    /// The LAN listener's address (LAN mode).
    pub fn lan_addr(&self) -> Option<SocketAddr> {
        self.lan_server.as_ref().map(httpd::Server::local_addr)
    }

    /// The tunnel, if there is one.
    pub fn tunnel(&self) -> Option<&Tunnel> {
        self.tunnel.as_ref()
    }

    /// The server's clock (ms since the Unix epoch): deadlines in views
    /// use it, phones count down to the same instant.
    pub fn now(&self) -> u64 {
        party::now_ms()
    }

    /// What happened since the last call; never blocks.
    pub fn poll(&mut self) -> Vec<Event> {
        party::pump(&mut self.core, &self.inbox, None);
        self.core.events()
    }

    /// Like [`poll`](Party::poll), but waits up to `timeout` for something
    /// to happen first.
    pub fn wait(&mut self, timeout: Duration) -> Vec<Event> {
        party::pump(&mut self.core, &self.inbox, Some(timeout));
        self.core.events()
    }

    /// Ends a [`wait`](Party::wait) from another thread, e.g. on Ctrl-C:
    ///
    /// ```no_run
    /// # fn f(party: &mut tvpipe_party::Party) {
    /// // With tvpipe's `signals` feature:
    /// let waker = party.waker();
    /// tvpipe::signals::on_signal(move || waker.wake());
    /// while !tvpipe::signals::requested() {
    ///     for event in party.wait(std::time::Duration::from_secs(1)) {
    ///         // ...
    ///     }
    /// }
    /// party.close();
    /// # }
    /// ```
    pub fn waker(&self) -> Waker {
        Waker(
            self._shared
                .inbox
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        )
    }

    /// Like [`wait`](Party::wait), until an instant.
    pub fn wait_until(&mut self, deadline: Instant) -> Vec<Event> {
        self.wait(deadline.saturating_duration_since(Instant::now()))
    }

    /// Sends what changed since the last flush: one `state` diff per
    /// client, then the answers to actions (unanswered ones are refused
    /// as `unknown-action`).
    pub fn flush(&mut self) {
        self.core.flush();
    }

    /// Answers an action: a result for the phone (`null` for none) or an
    /// error. Sent by the next [`flush`](Party::flush), after the state.
    pub fn reply(&mut self, action: Action, result: Result<Value, PartyError>) {
        self.core.reply(action, result);
    }

    /// What every player sees (`view.room`).
    pub fn room(&mut self) -> &mut State {
        &mut self.core.room
    }

    /// What screens see (`view.screen`).
    pub fn screen(&mut self) -> &mut State {
        &mut self.core.screen
    }

    /// What only this player sees (`view.me`).
    pub fn player_state(&mut self, id: &PlayerId) -> Option<&mut State> {
        self.core.player_mut(id).map(|p| &mut p.state)
    }

    /// The players in join order (away ones included).
    pub fn players(&self) -> &[Player] {
        self.core.players()
    }

    /// A player.
    pub fn player(&self, id: &PlayerId) -> Option<&Player> {
        self.core.player(id)
    }

    /// The VIP.
    pub fn vip(&self) -> Option<&Player> {
        self.core.players().iter().find(|p| p.vip())
    }

    /// No new players from now on (resumes still work).
    pub fn lock(&mut self) {
        self.core.lock();
    }

    /// New players may join again.
    pub fn unlock(&mut self) {
        self.core.unlock();
    }

    /// Whether new players are refused.
    pub fn locked(&self) -> bool {
        self.core.locked()
    }

    /// Removes a player for good, telling their phone why.
    pub fn kick(&mut self, id: &PlayerId, reason: &str) {
        self.core.kick(id, reason);
    }

    /// Ends the party: every client gets `bye room-closed`.
    pub fn close(&mut self) {
        self.core.close();
    }

    /// Closes the party and waits up to `timeout` for every socket to
    /// end, so the clients get their `bye room-closed` before the process
    /// exits (after [`close`](Party::close) alone, exiting at once may cut
    /// it off). True if every socket ended in time.
    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        self.core.close();
        let deadline = Instant::now() + timeout;
        loop {
            // Drain what the connection threads report meanwhile.
            party::pump(&mut self.core, &self.inbox, None);
            if self._shared.sockets.load(Ordering::SeqCst) == 0 {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Whether the party was closed.
    pub fn closed(&self) -> bool {
        self.core.closed()
    }

    /// A transient message to one player (lost when the phone is away).
    pub fn send(&mut self, to: &PlayerId, kind: &str, data: impl Into<Value>) {
        self.core.send(to, kind, data.into());
    }

    /// A transient message to every connected player.
    pub fn broadcast(&mut self, kind: &str, data: impl Into<Value>) {
        self.core.broadcast(kind, data.into(), &[Role::Player]);
    }

    /// A transient message to every connected client of these roles.
    pub fn broadcast_to(&mut self, roles: &[Role], kind: &str, data: impl Into<Value>) {
        self.core.broadcast(kind, data.into(), roles);
    }

    /// Runs the moderation hook on a text the game shows.
    pub fn moderate(&self, text: &str, kind: &str) -> Moderation {
        self.core.moderate(text, kind)
    }

    /// The screen view (`{roster, screen, join}`) as of the last flush,
    /// for a renderer on another thread.
    pub fn screen_watch(&self) -> ScreenWatch {
        self.core.watch()
    }

    /// The screen view now.
    pub fn screen_view(&self) -> Value {
        Value::Object(self.core.screen_view())
    }

    /// Replaces the join info screens get (tests).
    #[doc(hidden)]
    pub fn __set_join(&mut self, join: Value) {
        self.core.set_join(join);
    }

    /// The party's keys (tests): join, screen, watch.
    #[doc(hidden)]
    pub fn __keys(&self) -> (&str, &str, &str) {
        (
            &self.core.join_key,
            &self.core.screen_key,
            &self.core.watch_key,
        )
    }
}

/// A tunnel ([`Tunnel::start_auto`]) within `timeout`; a slow attempt
/// is told to stop and left to finish on its own thread.
fn auto_tunnel(port: u16, timeout: Duration) -> tvpipe::Result<Tunnel> {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let s = stop.clone();
    std::thread::Builder::new()
        .name("party-tunnel".into())
        .spawn(move || {
            let _ = tx.send(Tunnel::start_auto(port, &|| s.load(Ordering::SeqCst)));
        })
        .map_err(|e| tvpipe::Error::io("party: tunnel thread", e))?;
    match rx.recv_timeout(timeout) {
        Ok(r) => r,
        Err(_) => {
            stop.store(true, Ordering::SeqCst);
            Err(tvpipe::Error::timeout(format!(
                "no tunnel within {} s",
                timeout.as_secs()
            )))
        }
    }
}

/// The LAN listener: `port`, else the next free of the following 20
/// (0: any).
fn bind_lan(
    ip: IpAddr,
    port: u16,
    shared: &Arc<Shared>,
    config: httpd::Config,
) -> tvpipe::Result<httpd::Server> {
    let config = httpd::Config {
        lan: true,
        ..config
    };
    let ports: Vec<u16> = if port == 0 {
        vec![0]
    } else {
        (port..=port.saturating_add(20)).collect()
    };
    let mut last = None;
    for p in ports {
        let handler = server::handler(shared.clone(), Listener::Lan);
        match httpd::Server::bind_with(SocketAddr::new(ip, p), handler, config) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| tvpipe::Error::usage("party: no LAN port")))
}

/// Ends a [`Party::wait`] from another thread ([`Party::waker`]).
#[derive(Clone, Debug)]
pub struct Waker(mpsc::Sender<conn::Inbound>);

impl Waker {
    /// The waiting (or next) [`Party::wait`] returns now, with whatever
    /// happened (possibly nothing). Does nothing once the party is gone.
    pub fn wake(&self) {
        let _ = self.0.send(conn::Inbound::Wake);
    }
}

impl Drop for Party {
    fn drop(&mut self) {
        self.core.close();
        self.server.shutdown();
        if let Some(s) = &mut self.lan_server {
            s.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn client_matches_the_npm_copy() {
        let npm = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../npm/party/client/party-client.js"
        );
        // Outside the repository (the published crate) there is no npm copy.
        if let Ok(other) = std::fs::read_to_string(npm) {
            assert!(
                other == super::CLIENT_JS,
                "npm/party/client/party-client.js differs from web/party-client.js (the canonical copy)"
            );
        }
    }
}
