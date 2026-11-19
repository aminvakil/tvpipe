# Multi-screen games in Rust (design)

Status: proposal, nothing built. Builds on `docs/party-design.md` (the
Node-first party library as built: `@tvpipe/party`, drawbluff) and
`docs/rust-sdk-design.md` (the in-process `tvpipe` library, 0.2.0).

Goal (owner): a party / multi-screen **game written in Rust ships as one
binary** with its assets embedded. It streams its main screen to AirPlay
and Chromecast with the `tvpipe` library, shows a join QR code on the TV,
serves the game's controller web UI (DOM) to phones, through a tunnel or
on the LAN only, and talks to the phones with low latency. Node stays
supported and speaks the same protocol, but is the second citizen.

Priorities: (1) DOM controllers: a Rust host crate speaking the
`@tvpipe/party` wire protocol, then WebRTC data channels with the
WebSocket as fallback; (2) per-player video on phones: design only (§7).
WASM controllers (game code running on the phone): later, out of scope.

Contents: 1. crates and the game author's view, 2. transport (WebSocket,
WebRTC: `webrtc`/`rtc` vs `str0m` vs Chrome as the peer, measured), 3.
join modes (tunnel, LAN), 4. the reference game: drawbluff in Rust, 5.
the TV screen rendered in Rust, 6. shipping and security, 7. per-player
video (design only), 8. plan, 9. questions for the owner.

## 1. Crates and the game author's view

### 1.1 Layout

| Package | Path | What | Depends on |
|---|---|---|---|
| `tvpipe` (exists) | `crates/tvpipe/` | streaming; gains public `tvpipe::qr` and `tvpipe::Tunnel` (planned in party-design §3), a LAN bind for its HTTP server, and doc-hidden `__web` re-exports (below) | `ring`, `rustls` |
| **`tvpipe-party`** (new) | `crates/tvpipe-party/` | the party host: HTTP + WebSocket server, rooms, players, views and diffs, actions with acks, limits, names, the join link and QR, the embedded browser client, asset embedding; feature `webrtc` (§2) | `tvpipe` (features `tunnel`), `ring`; `rtc` only with `webrtc` |
| **`tvpipe-canvas`** (new) | `crates/tvpipe-canvas/` | a small software 2D renderer for RGBA frames: fills, rounded rectangles, anti-aliased strokes, TrueType text, images, the QR code (§5) | nothing (std) |
| `drawbluff` (Rust, new) | `games/drawbluff/` (`Cargo.toml` next to `package.json`, `publish = false`) | the reference game: rules, the host, the TV renderer; the phone UI in `web/phone/` is shared with the Node version | `tvpipe`, `tvpipe-party`, `tvpipe-canvas` |
| `@tvpipe/party` (exists) | `npm/party/` | the Node host, same protocol; its `client/party-client.js` becomes a copy of the Rust crate's (§1.5) | — |

Why a separate canvas crate: games that are not party games (a Rust
game streaming its frames, `rust-sdk-design` §6.1) want text and shapes
too, and the party crate stays free of rendering. Both are workspace
members with the shared version, released together.

**How `tvpipe-party` reaches tvpipe's internals.** The party server needs
tvpipe's HTTP/1.1 framing, the WebSocket implementation (RFC 6455, 1 182
lines, fuzzed), the Host/Origin rules and the interface list. Three ways:

1. make them public API (`tvpipe::web`): a semver promise on a web server
   that is not tvpipe's purpose;
2. copy them into `tvpipe-party`: two WebSocket implementations to keep
   hardened;
3. **(recommended)** a `#[doc(hidden)] pub mod __web` in `tvpipe`,
   outside the semver promise like the existing `__cli`, and
   `tvpipe-party` pins `tvpipe = "=0.x.y"` (lockstep versions make this
   free). Promote to public later if a third party asks.

Changes inside tvpipe for this: `net::httpd` binds a LAN address on
request (today loopback only, by design: an explicit
`Bind::Lan(addr)` with its own access rule, §3.2), its connection cap
(32) becomes a parameter (a party needs 64 + headroom), and
`net::tunnel::Provider` gains `Auto` (Cloudflare, OpenTunnel on 429;
the Node library does this today).

### 1.2 Threads, not async

`tvpipe-party` follows tvpipe's rule: **no async runtime**, `std::thread`
and blocking I/O. Reasons: a game owns its loop (a frame tick, or an
engine's schedule) and should not have to host tokio; the party's load is
tiny (≤ 64 sockets, a few messages per second each), which a thread per
connection handles easily; and the WebRTC stack we recommend is sans-IO
(§2.3), so nothing forces a runtime. The rule should hold for the party
crate as for tvpipe.

Inside: one accept thread (tvpipe's `httpd`), a thread per connection
(read loop; writes go through the cloneable WebSocket `Sender`), and the
**party core** owned by whoever calls it: connection threads push events
into a channel, the game drains it. There is no party thread calling
game code: the game's callbacks never run on foreign threads, so game
state needs no locks. A small timer in the core (deadlines, pings, the
hello timeout) runs when the game polls, plus a housekeeping thread for
liveness pings that does not touch game state.

### 1.3 The API, as a game loop

Node's API is callback-shaped (`party.handle`, `await party.collect`).
Rust games are tick-shaped; the Rust API is a poll loop with the same
concepts and the same wire behaviour:

```rust
use std::time::Duration;
use tvpipe::{Options, PixelFormat::Rgba};
use tvpipe_party::{assets, json, Action, Event, Join, Party, PartyError, PartyOptions};

static PHONE: tvpipe_party::Assets = assets!("web/phone", [
    "index.html", "phone.js", "style.css", "fonts/plex-sans.woff2",
]);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut party = Party::start(
        PartyOptions::new()
            .phone(&PHONE)                       // served at /, the client at /party/client.js
            .join(Join::Tunnel(Default::default())) // or Join::Lan, Join::Both
            .players(3..=8),
    )?;
    println!("join: {}", party.join_url());     // also party.join_qr() for the TV

    let tv = tvpipe::stream(Options::new().to("Living Room").video(1280, 720, 30, Rgba))?;
    let mut canvas = tvpipe_canvas::Canvas::new(1280, 720);
    let mut game = Game::default();

    party.room().set(json!({ "phase": "lobby" }));
    loop {
        // Everything that happened since the last tick; never blocks.
        for event in party.poll() {
            match event {
                Event::Joined(p) | Event::Back(p) => game.roster_changed(&party),
                Event::Away(_) | Event::Left(_) => game.roster_changed(&party),
                Event::Action(a) => game.act(&mut party, a),   // a.reply(Ok(json)) or a.reply(Err(PartyError::new(..)))
                _ => {}
            }
        }
        game.tick(&mut party, party.now());     // phases, deadlines
        party.flush();                          // one `state` diff per client: the coalescing point
        if tv.wants_frame() {
            draw(&mut canvas, &game, party.join_qr());
            tv.write_video(canvas.rgba(), tv.elapsed())?;
        }
        std::thread::sleep(Duration::from_millis(33)); // or party.wait(next_deadline)
    }
}

fn act(game: &mut Game, party: &mut Party, a: Action) {
    match a.kind() {
        "start" if !a.player().vip() => a.reply(Err(PartyError::new("not-vip", "Only the VIP starts"))),
        "start" => { party.lock(); game.start(party); a.reply(Ok(json!(null))) }
        "answer" => game.answers.offer(a),      // Collect: one action per player, until a deadline
        _ => a.reply(Err(PartyError::unknown_action())),
    }
}
```

- `party.poll()` drains events; `party.wait(until)` blocks until an event
  or the instant, for games without a frame loop (drawbluff's host can
  be a plain state machine on its own thread).
- Views: `party.room()`, `party.screen()`, `player.state()` take
  `set(json)` (replace) and `merge(json)`; `flush()` sends diffs (the
  Node library coalesces per microtask; the tick is Rust's equivalent).
  An action not replied to by the end of `flush()` is answered
  `no-reply` (a bug in the game, logged), so phones never hang.
- `Collect<T>` is the helper behind `party.collect`: `Collect::new(kind,
  players, deadline)`, `offer(action)` (validates, replies, keeps the
  first), `done(now)` (everyone answered, or the deadline plus 500 ms
  grace passed), `take()`. Same semantics as Node's (away players are
  waited for until the deadline unless `skip_away`).
- JSON values: a small `tvpipe_party::Value` (hand-rolled, on tvpipe's
  `codec::json` through `__web`) with `From` impls and a `json!`
  `macro_rules!` like serde_json's. A game converts its own types by
  hand (`impl From<&Phase> for Value`); an optional `serde` feature is
  an owner question (§9), not part of the plan.
- The screen view for the video renderer: `party.screen().view()`
  returns the current `Value` (cheap clone, `Arc` inside), so a render
  thread can hold a snapshot.

### 1.4 Embedded assets without extra crates

`assets!(dir, [files…])` is a `macro_rules!` expanding to a static table
of `(path, include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/", dir,
"/", file)), mime)`: the list is explicit, the bytes are in the binary,
and nothing runs at build time. A forgotten file is a 404, so the crate
offers `Assets::check_dir(path)` for a test (`assert!(PHONE.check_dir
("web/phone").is_ok())`, listing files on disk that are not embedded and
vice versa). Alternatives considered: a `build.rs` walking the directory
(std only, no list, but every game copies a build script and rebuilds on
any change) and a proc macro (a new crate type, `syn`-free but still
more machinery); the explicit list is the smallest thing that works.

Development: `TVPIPE_PARTY_ASSETS=web/phone` (or `.phone_dir(path)`)
serves from disk instead, so the phone UI reloads without recompiling.
MIME types come from the extension (tvpipe's table), ETags from a hash
computed once at start.

### 1.5 One protocol, one browser client, golden vectors

- The browser client (`party-client.js`, 615 lines) has **one canonical
  copy** in `crates/tvpipe-party/web/` (a crate cannot `include_str!`
  outside its package when published). `npm/party/client/` keeps a
  copy; a test in each package compares the bytes (and `npm/party`'s
  `prepack` copies it), so they cannot drift silently.
- **Golden vectors** in `crates/tvpipe-party/tests/vectors/` (tvpipe's
  `key = value` format), replayed by the Rust tests and by
  `npm/party/test/` reading `../../crates/tvpipe-party/tests/vectors/`:
  - `diff.txt`: view before, view after → the `state` message, and the
    client's merge result (both servers must produce identical diffs;
    key order is part of it);
  - `names.txt`: raw name → cleaned name or refusal, and the uniqueness
    key (§1.6);
  - `sessions/*.txt`: transcripts of client ⇄ server messages for join,
    resume, VIP hand-over, lock, kick, replayed acts, `resync`,
    rate-limit and protocol errors (keys and times scrubbed). Each server
    is driven by a scripted fake client and must produce the transcript.
  - Close codes and `bye` reasons as one table both test suites check.
- The Node and Rust servers keep their own code. Generating one from the
  other is not worth it at ~1 500 lines; the vectors keep them honest.

### 1.6 Names without Unicode tables

Node normalises names with `normalize("NFC")`, `Intl.Segmenter`
(graphemes) and `normalize("NFKD")` + mark removal for the uniqueness
key. Rust's std has none of these. Proposal:

- The client sends `name.normalize("NFC")`; the server does not trust it
  but needs no normaliser to stay safe: it strips the same control,
  bidi, zero-width and private-use ranges (a fixed list), collapses
  whitespace, and bounds the length.
- Graphemes: a generated table of the ranges that extend a grapheme
  (combining marks, ZWJ sequences, variation selectors, emoji modifiers;
  a few hundred ranges, a few KB) gives "user-perceived characters" for
  the 16-character bound. Exact UAX #29 is not needed for a length cap.
- Uniqueness key: lowercase (std, Unicode-aware) plus a decomposition
  table for Latin letters with diacritics (Latin-1 Supplement, Latin
  Extended-A/B: a few hundred entries), so "Zoë" and "zoe" collide as in
  Node. Other scripts compare by lowercase only.
- `names.txt` holds the cases where both agree; the few deliberate
  differences (a non-Latin compatibility character) are listed in the
  vector file as such.
