# Multi-screen games in Rust (design)

Status: decided (§10), being built in the order of §8.
Builds on `docs/party-design.md` (the
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
video (design only), 8. plan, 9. questions for the owner, 10. decisions.

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
the public `Tunnel` gains `start_auto` (Cloudflare, else OpenTunnel;
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
    client's merge result (both servers must produce the same diffs;
    objects compare without key order);
  - `names.txt`: raw name → cleaned name or refusal, and the uniqueness
    key (§1.6);
  - `sessions.txt`: transcripts of client ⇄ server messages for join,
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

## 2. Transport

### 2.1 What latency is there to win

| Path (phone → host → phone) | RTT | Source |
|---|---|---|
| WebSocket through a Cloudflare quick tunnel | median 16.5–17.1 ms, p95 19.7 ms, max 26 ms | party-design §13 (client on the host's Mac, so the internet is crossed twice, as for a phone on the same Wi-Fi) |
| WebSocket through OpenTunnel | median 43 ms, p95 47 ms | same |
| WebSocket over the LAN (LAN mode, §3.2) | Wi-Fi: typically 2–10 ms, spikes of 50–200 ms when a phone's Wi-Fi power saving kicks in | to measure in plan step 4 (§8) |
| WebRTC data channel over the LAN | same Wi-Fi floor; no head-of-line blocking on unordered channels | to measure in plan step 7 |

So for turn-based games (drawbluff) the tunnel is already fine: the TV's
playout delay (100–300 ms) dominates. The fast path matters for
real-time controllers (a joystick, a racing game with phones as wheels):
there the tunnel adds 10–40 ms and, worse, one lost TCP segment stalls
every later input (head-of-line blocking) for a retransmission timeout.
A data channel with `ordered: false, maxRetransmits: 0` delivers the
latest stick position or nothing.

### 2.2 Recommendation in short

1. **WebSocket first**, exactly party-design §5 (protocol v1), through
   the tunnel or on the LAN. It always works, it is the only path in
   Node, and it carries signaling for the fast path.
2. **WebRTC data channels as an optional fast path** in `tvpipe-party`
   (feature `webrtc`), on the **`rtc` crate** (the sans-IO core under
   `webrtc` 0.21, same project and version) with blocking `UdpSocket`s;
   LAN only by default (host candidates, no STUN, no TURN). Anything it
   fails at falls back to the WebSocket silently.
3. Not `str0m` (no ring backend, twice the size, measured below), not
   async `webrtc` (forces a runtime for no gain over its own core), not
   Chrome as the peer (an installed browser and 200 MB of RAM for a
   data channel), unless measurements in plan step 7 say otherwise.

### 2.3 Measured: WebRTC stacks for a data channel answerer

Scratch crates outside the repository (`/tmp/mscratch`), 2026-11-19,
macOS 26 arm64, Rust 1.99, each a minimal **ICE-lite answerer with an
echoing data channel** on a `std::net::UdpSocket` (async `webrtc` on
tokio, its runtime), release, `lto = true`, `codegen-units = 1`,
stripped. "On top of tvpipe" adds the stack to `crates/tvpipe/examples/
stream.rs` built the same way (2.74 MB alone; a hello world is 0.30 MB).

| | `rtc` 0.21.1 (sans-IO) | `webrtc` 0.21.1 (async) | `str0m` 0.25.0 (sans-IO) |
|---|---|---|---|
| Model | sans-IO: `handle_read`, `poll_write`, `poll_event`, `poll_timeout` | async over `rtc`; `runtime-tokio` (default) or `runtime-smol` | sans-IO: `handle_input`, `poll_output` |
| Crypto backend | **`ring`** (default) or `aws-lc-rs`, or a provider of our own (`RTCCryptoProvider` trait) | forwards to `rtc` | `aws-lc-rs` (default), `rust-crypto`, `openssl`, `apple-crypto` (needs a Swift toolchain via `swift-rs`; did not resolve here), `wincrypto`; **no ring** |
| C code / toolchain | none beyond `ring`'s (already ours) | same | `aws-lc-sys` (C: a C compiler on every platform, build-time questions on Windows) **even with `rust-crypto`**: `str0m-rust-crypto` enables `dimpl/rcgen`, which enables `aws-lc-rs` |
| Crates in the tree (normal deps, unique) | 148 (incl. `ring`, `rustls`, `serde`, `serde_json`, `url` + `idna` with ~20 ICU support crates, `rkyv`, `nix`, `uuid`, `x509-parser`, `rcgen`, `socket2`) | 177 (+ `tokio`, `mio`, `futures`, `async-*`, `parking_lot`, `quinn-udp`) | 107 (incl. `aws-lc-rs`/`aws-lc-sys`, RustCrypto, `serde`, `tracing`, `rcgen`) |
| Binary, alone | 2.94 MB | 3.87 MB | 4.62 MB |
| Binary, on top of tvpipe | **4.83 MB (+2.09 MB)**; shares `ring`/`rustls` | not built (≈ alone + tvpipe − shared) | 6.99 MB (+4.25 MB) |
| Clean release build | 33 s | 43 s | 75 s (aws-lc) |
| License | MIT/Apache-2.0 | MIT/Apache-2.0 | MIT |
| MSRV | 1.89 | 1.89 (edition 2024) | 1.89 |
| ICE-lite | yes (`SettingEngine::with_lite`) | yes | yes |
| Maturity | the sans-IO rewrite of webrtc-rs (Pion lineage), young in this form; changelog shows active work (pluggable crypto providers, conformance suite) | as `rtc` plus the async layer | sans-IO from the start, several years old, used in production; frequent breaking minor releases (0.25 lists five) |

Readings:

- The owner's instinct about `webrtc` 0.21 holds, with a twist: since
  the rewrite, `webrtc` is a thin async shell over `rtc`, and `rtc` is
  sans-IO. We take the core and drive it from our threads: no tokio, no
  exception to the async rule, and the async API is not needed.
- `rtc`'s default crypto is `ring`, tvpipe's own, and it reuses
  `rustls` (rcgen for the self-signed DTLS certificate). Its weight is
  elsewhere: 140-odd extra crates of which most are glue (`url`/ICU via
  ICE URLs, `serde_json` for SDP init objects, `rkyv` in DTLS, `nix` for
  socket options). That is a real change to the "ring + rustls, nothing
  else" policy and needs the owner's yes (§9); the feature gate keeps it
  out of games that do not ask for it, and out of `tvpipe` entirely.
- `str0m` is the more battle-tested sans-IO stack, but on our
  constraints it loses: no `ring` provider (a provider of our own would
  mean implementing its DTLS crypto traits), an unavoidable C library in
  0.25 even on the "pure Rust" backend, and twice the binary cost.
- Neither was checked for panics on hostile input. Before the fast path
  ships, our fuzz smoke tests (`AGENTS.md`: never panic on network data)
  get a target that feeds random datagrams into an `rtc` peer
  connection; a game built with `panic = "abort"` must not die from a
  malformed STUN packet. The stack sits behind a small internal trait
  (`FastPath`: datagram in, datagrams and channel messages out, timeouts),
  so swapping `rtc` for `str0m` or for a hand-rolled stack is one module.

**Chrome as the peer.** tvpipe already drives a headless Chromium over
CDP (`share URL`). A hidden host page could hold the phones'
`RTCPeerConnection`s and relay to the Rust host over a loopback
WebSocket. For: libwebrtc itself, zero new crates, and later per-player
video with hardware encoding for free (`canvas.captureStream()` → video
tracks, §7). Against: needs Chrome/Edge/Chromium installed (the
"one binary" promise breaks), 150–300 MB of memory and 1–2 s to start,
another process to supervise, the firewall prompt names the browser, and
mDNS candidate obfuscation flags to manage. Keep it as the fallback if
`rtc` disappoints, and as the likely route for §7.

**Hand-rolled** (ICE-lite STUN responder, DTLS 1.2 with one cipher suite
on `ring`, SCTP for data channels, DCEP): within the project's habits
(tvpipe hand-rolls TLS-adjacent pairing, RTSP, mDNS), but SCTP alone is
several thousand lines of retransmission and congestion logic to get
right. Not now; revisit if the dependency weight of `rtc` is refused.

### 2.4 How the fast path works

- **Offerer is the phone.** After `welcome`, a client that supports it
  (and was told `rtc: true` in `welcome`) creates two channels, `r`
  (reliable, ordered) and `u` (`ordered: false, maxRetransmits: 0`),
  and sends `{t: "rtc", sdp}` over the WebSocket. The host answers with
  `{t: "rtc", sdp}` (ICE-lite, its host candidates embedded, no trickle).
  Unknown `t` is ignored by v1 peers, so a Node host or an old client
  simply never upgrades. `hello` gains `caps: ["rtc"]`.
- **ICE on the LAN.** Phones' browsers hide their addresses behind mDNS
  `.local` candidates (no camera permission). We never need to resolve
  them: the host is ICE-lite with host candidates on its LAN
  interfaces; the phone sends connectivity checks to them and the host
  learns the phone's address as peer-reflexive. One UDP socket per
  interface serves all phones (demultiplexed by ICE username), on a
  fixed port when configured (firewall rules, §6).
- **No STUN, no TURN by default.** A phone on mobile data (tunnel mode)
  cannot reach a LAN address and stays on the WebSocket; that is the
  fallback working, not a failure. STUN for server-reflexive candidates
  is an opt-in later (it reveals the host's public address to every
  player with the join key, which the tunnel otherwise hides). TURN is
  never: the WebSocket is our relay.
- **Binding to the player.** The SDP exchange runs over the
  authenticated WebSocket, so the DTLS fingerprints are bound to the
  player's session; a data channel is attached to exactly that player.
  Over a tunnel the signaling is TLS; in LAN mode (plain HTTP) an active
  attacker on the Wi-Fi could swap fingerprints, the same exposure as
  the WebSocket itself (§3.2).
- **What travels where.** The WebSocket stays the source of truth:
  `welcome`, `state`, `bye`, and acts when the channel is down. When `r`
  is open the client sends `act` on it and the host may send `state`,
  `ack`, `msg` on it (same JSON, same `rev`; a gap triggers `resync`
  over either). Act ids already make delivery exactly-once across paths
  (the server's per-player cache), so switching paths mid-game is safe.
  `u` carries a new message, `{t: "in", s, data}`: continuous input,
  latest wins by sequence `s`, never acked; `party.poll()` surfaces it
  as `Event::Input` coalesced per player per tick. Node hosts never get
  `in` (no `rtc`), and a client without a channel sends `in` over the
  WebSocket, rate-limited to the token bucket.
- **Failure.** No open channel within 3 s, an ICE failure or a closed
  channel: the client keeps using the WebSocket, retries once after a
  network change (`online` event, page visible), and tells the host
  (`{t: "rtc", state: "failed"}`) for the logs.

## 3. Join modes

`PartyOptions::join(…)`: `Join::Tunnel(provider)`, `Join::Lan(bind)`, or
**`Join::Auto` (the default)**: a tunnel (Cloudflare, OpenTunnel when it
answers 429), and the LAN link when no tunnel comes up within 10 s (no
internet: a cabin, a train, a captive portal). The TV shows whichever
link is live; the host may serve both at once (the WebSocket and pages
answer on loopback for the tunnel and on the LAN address).

### 3.1 Tunnel (as built in Node)

Join link `https://HOST/?r=CODE#k=JOINKEY`; the server binds 127.0.0.1
and accepts `Host` = the tunnel's name (party-design §6). The page is
**HTTPS, a secure context**, reachable from mobile data, and needs no
firewall exception (loopback listener only). Costs: internet required,
Cloudflare reads everything (OpenTunnel ends TLS in tvpipe), 6–7 s to a
Cloudflare link (0.2 s OpenTunnel), and the RTT of §2.1.

With feature `webrtc`, tunnel mode is the **hybrid**: page, keys and
signaling over the tunnel, the data channel directly over the LAN when
the phone is on the same network. This is the only way for an HTTPS page
to reach the LAN host directly: `ws://192.168.…` from an `https:` page
is mixed content (blocked), and Chrome's Local Network Access gates
fetches from public origins to private addresses behind a permission
prompt; WebRTC's DTLS transport is neither (Local Network Access does not
cover WebRTC today; to re-check as Chrome evolves).

### 3.2 LAN only

- **Bind** one LAN address (default: the interface with the default
  route, from tvpipe's `net::iface`; `--lan 192.168.1.23`), port 7420 by
  default (fixed, so a firewall rule or a bookmarked link survives a
  restart; the next free one if taken).
- **Link** `http://192.168.1.23:7420/?r=CODE#k=JOINKEY` in the QR code;
  the IP, not a `.local` name (Android resolves mDNS names unreliably).
- **Access rule** (a third case next to tvpipe's local and public ones):
  `Host` must be exactly `IP:PORT` (or a loopback name), WebSocket
  `Origin` `http://IP:PORT`. DNS rebinding and cross-site WebSocket
  hijacking stay closed. Keys as before; the join key is in the fragment
  but crosses the Wi-Fi in the `hello` **in clear**: anyone who can read
  the network (open Wi-Fi, or WPA2-Personal peers with the passphrase)
  can join. That is the LAN mode's trade, said on the TV's setup line and
  in the docs; tunnel mode is the answer on untrusted networks.
- **Works without internet**; fails with Wi-Fi client isolation (hotels,
  many guest networks), phones on mobile data or a phone VPN, and
  separate VLANs. The host cannot detect these (no OS network APIs), so
  the TV shows "phones must be on the same Wi-Fi as this computer" and
  the address; `Join::Auto` prefers the tunnel when there is internet.

**No secure context over `http://IP`.** What that means for controllers:

| Works on plain HTTP | Needs a secure context (tunnel mode) |
|---|---|
| DOM, CSS, canvas, touch and pointer events | Screen Wake Lock (drawbluff asks for it: a silent no-op; a locked phone reconnects and resumes, as designed) |
| WebSocket (`ws:`), `localStorage`, `sessionStorage` | `crypto.subtle`, `crypto.randomUUID()` |
| `RTCPeerConnection` and data channels | DeviceOrientation/DeviceMotion (iOS `requestPermission()`, Chrome): tilt and shake controllers |
| Audio after a tap, Fullscreen (Android) | `getUserMedia` (camera and microphone: selfie avatars, voice) |
| `crypto.getRandomValues()` | service workers, offline/PWA install, Web Share, async clipboard, notifications |

Consequences: the shared client and drawbluff's phone UI use nothing
from the right column without a fallback (a CI check loads them from an
`http://` non-loopback origin in headless Chrome); a game that needs
the right column declares it (`PartyOptions::require_secure_context()`)
and LAN-only is then refused at start with a clear error. Browsers mark
the page "Not secure"; whether Chrome's HTTPS-First mode puts an
interstitial in front of `http://` private addresses on current Android
and whether iOS does anything similar is to check on devices in plan step 4.

**HTTPS on the LAN** was considered and left out: self-signed
certificates (a full-page warning on every phone), installing a local CA
(not for party guests), and a real certificate for a per-install name
that resolves to the LAN address (the plex.direct approach: a DNS zone
and ACME on a service we would have to run). The hybrid mode (§3.1)
gives HTTPS plus LAN latency whenever there is internet; question 6 in §9
asks whether a hosted name service is ever wanted.

### 3.3 As built (plan step 4)

- `Join::Lan(Option<IpAddr>)` (`None`: the default route's address,
  `iface::default_ipv4`), `PartyOptions::lan_port` (default `LAN_PORT`
  = 7420, else the next free of the following 20; 0: any). The LAN
  listener is a **second** `httpd` server next to the loopback one, so
  the local links (`screen_url`, `local_join_url`) keep working in every
  mode; each listener has its own Host rule (loopback: the loopback
  names and the tunnel's name; LAN: exactly `IP:PORT`) and Origin rule
  (LAN: exactly `http://IP:PORT`). `tests/lan.rs` checks both against
  a LAN listener bound to 127.0.0.1 (no network, no firewall prompt).
- `Join::Auto` runs `Tunnel::start_auto` on its own thread and waits
  `PartyOptions::tunnel_timeout` (default 10 s); then the attempt is told
  to stop and the LAN link is used, with a warning in the log.
- `PartyOptions::require_secure_context()`: `Join::Lan` fails at start,
  `Join::Auto` returns the tunnel's error instead of falling back.
- `Party::link()` (`Link::{Tunnel, Lan, Local}`) and `Party::lan_addr()`;
  the screens' `join` gains `mode` (`tunnel`, `lan`, `local`; the Node
  host sets `tunnel` or `local`), for the TV's "same Wi-Fi" line.
- Checked live (`scripts/live/party-buzzer.ts lan`): headless Chrome as
  two phones on `http://192.168.…:7420`, `isSecureContext` false, the
  shared client and the example's page work (localStorage resume, acks).
- Not measured yet: LAN RTT from a real phone, what iOS Safari and
  Android Chrome show for plain HTTP on a private address (HTTPS-First),
  the Windows firewall prompt. They need devices; the loopback numbers on
  one machine say nothing.

## 4. The reference game: drawbluff in Rust

One binary, `drawbluff`, same rules, prompts and phone UI as the Node
version (party-design §10), proving the whole path: game logic,
`tvpipe-party` (phones), `tvpipe` (the TV), `tvpipe-canvas` (the TV
picture), embedded assets.

```
games/drawbluff/
  Cargo.toml          package `drawbluff` (publish = false), a workspace member
  src/main.rs         CLI: --to NAME…, --lan / --tunnel=…, --bots N, --rounds N, --fast, --size WxH, --web-screen
  src/rules.rs        port of game/rules.ts: validation, options, scoring, reveal order, mulberry32
  src/game.rs         port of game/game.ts: the phase machine on Party (§1.3), Collect for draw/lie/vote
  src/screen.rs       the TV renderer on tvpipe-canvas (port of screen-video.ts)
  src/sound.rs        generated cues (join, tick, reveal) as PCM for Stream::write_audio
  src/bots.rs         fake players over real WebSockets, for tests and --bots
  data/prompts.txt    the prompts, one per line: read by prompts.ts (`readFileSync`) and by `include_str!`
  fonts/*.ttf         TrueType versions of the OFL fonts the web UI uses (§5)
  web/phone/          unchanged, embedded with assets! (and served from disk with TVPIPE_PARTY_ASSETS)
  web/screen/         the web screen, embedded for --web-screen
  game/ main.ts …     the Node version, unchanged apart from reading data/prompts.txt
```

- **Same rules, checked.** `games/drawbluff/test/vectors/rules.txt` (shared): seeded
  games as inputs (players, prompts, drawings' validity, lies, votes, the
  mulberry32 seed) and the expected options, reveal order and scores per
  round. The TS tests (`games/drawbluff/test/`) and the Rust tests replay
  the same file; mulberry32 is three lines in both languages, so shuffles
  agree. Drawing validation vectors (point limits, coordinates, palette)
  likewise.
- **Same phone UI**, byte for byte: the phone talks to either host
  through the shared client; nothing in `web/phone/` knows which host it
  has. The live check (`test/live/play.ts`, headless Chrome as the
  phone) gains a `--host rust` switch, running the binary instead of
  `node main.ts`, so one script checks both.
- **The TV**: the video screen by default (rendered in Rust, §5, one
  binary, no browser needed); `--web-screen` casts `web/screen/` with
  `tvpipe::share` (feature `headless`, needs Chrome/Edge/Chromium/Brave or
  Firefox, as in Node), driven by the same `screen` view. `--to browser`
  for development, as everywhere in tvpipe.
- **Threads**: the game on the main thread (`party.wait(next_deadline)`,
  ticks on events and deadlines); a render thread that takes the latest
  screen view snapshot and the clock, draws and writes frames at 30 fps
  when something moves (countdowns, reveal steps), otherwise at the
  encoder's minimum rate (`wants_frame()`); the audio cues are written by
  the render thread from `msg`-like cue events. TV restarts (a receiver
  gone) re-create the `Stream` without touching the party.
- **Size budget** (to confirm when built): tvpipe defaults (2.7 MB) +
  party (~0.3 MB) + canvas (~0.1 MB) + fonts (~0.3 MB) + phone UI
  (~0.1 MB) ≈ 3.5 MB stripped, 5.6 MB with `webrtc`.

### 4.1 As built (plan step 6)

`games/drawbluff` is a workspace member (package `drawbluff`, a library
for the tests plus the binary; `publish = false`):

- `src/rules.rs`: the rules, `Rng` (mulberry32), JavaScript's
  `Math.round` for coordinates, `title_key` on `tvpipe_party::fold` (NFKD
  without marks, now public with `clean_text` and `name_key`) and
  `\p{L}\p{N}` as `is_alphanumeric` minus four `So` ranges that are
  `Other_Alphabetic`. `test/vectors/rules.txt` is written by `node
  test/vectors.ts --bless` from `game/rules.ts`: mulberry32 outputs,
  shuffles, 34 titles (accents, ligatures, full width, Greek, Turkish,
  CJK, Hangul, emoji, bidi), 23 drawings (rounding, clamping, every
  refusal, the stroke and point limits) and 40 seeded turns (options,
  what each voter sees, points, voters, reveal order); both test suites
  replay it.
- `src/game.rs`: the phase machine (`Drawbluff::new/open/event/tick/
  wake/take_events`), `Collect` per phase, the views field for field as
  `game.ts` (the phone and web screen pages are unchanged). Moderation of
  lies is asked before `Collect::offer` (which holds the party).
- `src/bots.rs`: a party client in Rust (hello, welcome, diffs as the
  browser client applies them, acks, resync, resume after a dropped
  socket) and the bots of `bots.ts`. `tests/game.rs` plays the five
  games of `game.test.ts` over real WebSockets (~10 s).
- `src/screen.rs`: `screen-video.ts` on `tvpipe-canvas`, same layout;
  ★ and ✓ drawn as shapes (the fonts lack them), emoji stripped, the
  lobby's QR code does not pulse (a still lobby costs the encoder
  nothing). 0.1–1.4 ms per 1280×720 frame for text screens, 6.3 ms with
  a drawing (release, one M-series core); redrawn only when the view or
  the countdown's second changes. `tests/screen.rs` draws every phase
  (`DRAWBLUFF_SCREEN_OUT=DIR` writes the frames and the views, which the
  Node renderer drew side by side for comparison).
- `src/sound.rs`: the web screen's cues (triangle notes, its
  envelope), mixed over silence into continuous 48 kHz stereo.
- `src/tv.rs`: the TV on its own thread, `Stream::start` (video) or
  `tvpipe::share` (`--web-screen`, tvpipe feature `headless`), started
  again with a backoff when the session ends.
- `src/main.rs`: the CLI, arguments hand-rolled (`argument` stays in
  tvpipe-cli). No Ctrl-C handler (the CLI's is in tvpipe-cli): an
  interrupted game ends without the receivers' teardown.
- `node test/live/play.ts --host rust [--video]`: the Rust binary with
  the Node bots and a phone in headless Chrome, the TV in the browser
  player; passes with both screens (13 screenshot pairs each).
- Size: 5.5 MB stripped (release as configured, macOS arm64), 4.8 MB
  with LTO and one codegen unit; the `tvpipe` CLI is 5.2 MB and 4.4 MB
  built the same ways. The 3.5 MB estimate above was low: tvpipe with
  its default features is the bulk, the game adds ~0.4 MB.

## 5. The TV screen rendered in Rust

drawbluff's video screen in Node uses `@napi-rs/canvas` (Skia). In Rust
the equivalent dependency (`tiny-skia` + a font crate, or Skia bindings)
is not allowed and not needed: a party game's TV picture is rectangles,
rounded panels, circles, anti-aliased strokes, a QR code and a lot of
text in three fonts. **Proposal: `tvpipe-canvas`, std only.**

| Piece | How | Size (estimate) |
|---|---|---|
| Canvas | an RGBA8 buffer (straight alpha, sRGB treated as linear like browsers' canvas), clip rectangle, `fill_rect`, `blit`, `copy_from` (images, the previous frame) | 200 lines |
| Paths | lines and quadratic/cubic Béziers flattened to polygons; filled with a **signed-area coverage accumulator** (the `font-rs` technique: exact anti-aliasing, non-zero winding, one pass per row); rounded rectangles, circles and arcs as paths | 300 lines |
| Strokes | drawbluff's strokes as round-capped, round-joined polylines: per segment, coverage from the distance to the segment (a capsule) over its bounding box, max-combined per stroke so joints do not double-blend | 150 lines |
| TrueType | `cmap` (formats 4 and 12), `loca`/`glyf` (simple and composite glyphs, quadratic outlines), `hmtx`/`hhea`, `kern` format 0 (optional); outlines → the same accumulator; no hinting (TV sizes are ≥ 18 px); glyph coverage cached per (font, glyph, size, 1/4-pixel x offset) | 700 lines |
| Text | left-to-right layout, kerning, `measure`, alignment, word wrap into a width, ellipsis at a maximum width, fallback through a list of fonts, then a box | 250 lines |
| QR | `tvpipe::qr` modules scaled to whole pixels, quiet zone, dark on white | 40 lines |

Tests: known-answer coverage for shapes (a half-covered pixel is 50 %,
a circle's area matches πr² within 0.5 %), glyph bitmaps of a few
characters against checked-in PGM fixtures, a TrueType parser that
returns errors (never panics) on truncated and fuzzed font data, and
drawbluff's renderer drawing every phase without error and with the QR
modules on the expected pixels (as `test/video.test.ts` does in Node).

**Fonts.** The web UI ships WOFF2, which is Brotli-compressed with a
transformed `glyf` table: decoding it would mean a Brotli decoder
(~1 500 lines plus a 120 KB dictionary). Instead the game embeds TTF
versions of the same OFL fonts (Plex Sans, Michroma, Sixtyfour), subset
to Latin, Latin-1 and Latin Extended-A once with a font tool outside the
build (the subset files are checked in with `OFL.txt`); ≈ 50–150 KB each.
CFF-flavoured OpenType (`.otf`) is not supported: say so and pick `glyf`
fonts.

**Considered:**

- *Bitmap font* (public-domain or OFL pixel fonts): ~100 lines and no
  parser, but blocky when scaled to TV headline sizes, one look only,
  and drawbluff's display face (Michroma) is gone. Fine for a debug
  overlay, not for the game.
- *Pre-baked SDF/MSDF atlas*: smooth at any size from one texture and
  cheap to draw, but needs a generator (msdfgen or our own rasteriser
  anyway), a fixed character set chosen at build time (players' names
  are arbitrary), and blurs small text. It saves no code once we have a
  rasteriser.
- *Pre-rendered glyph atlases at fixed sizes* (made once with a tool,
  checked in): zero font code at run time, but every new size or font is
  a regeneration step and names outside the set fall back to boxes. The
  fallback plan if the rasteriser overruns its budget.

**Limits, stated:** no shaping (no Arabic, Hebrew, Indic scripts, no
ligatures), no colour emoji. Player names with emoji are common; the TV
draws them as the name without the emoji (the phones, being DOM, show
them). A monochrome emoji font (Noto Emoji, OFL, ~1 MB+) is question 5.
Performance: a 1280×720 frame of a busy phase (two drawings with 12 000
points, 40 lines of text with a warm glyph cache) should render in
under 10 ms on one core; the renderer redraws only when the view or an
animation changes, otherwise it re-sends the last frame.

### 5.1 As built (plan step 5)

`crates/tvpipe-canvas` (no dependencies), about 2 900 lines with tests:
`Canvas` (RGBA8, straight alpha, clip, source-over; `fill_rect`,
`fill_rounded_rect`, `fill_circle`, `fill_path`, `stroke_path`,
`stroke_polyline`, `draw_rgba`/`draw_canvas`, `draw_rgba_scaled`
(bilinear, premultiplied), `draw_qr` (any module grid: `tvpipe::QrCode`'s
`size()`/`dark()`, so the crate needs no tvpipe)), `Path` (lines, quads,
cubics, rects, rounded rects, ellipses, arcs; flattened to 0.02 px),
the font-rs accumulator with clipping on all four sides, capsule
strokes, `Font` (cmap 4/12, hmtx, simple and composite glyphs, `kern`
format 0 **and GPOS pair adjustment** (formats 1 and 2, extension
lookups): the subset fonts carry kerning only in GPOS), `Text` (layout,
kerning, measure, `Align`, `Baseline`, wrap, ellipsis, fallback fonts,
glyph cache per font/size/quarter pixel, `strip_emoji`/`is_emoji`).

- TTFs come from the WOFF2 the pages use, made once by
  `scripts/fonts/ttf.py` (fontTools via `uvx`): variable fonts pinned to
  static instances (Plex Sans 400 and 600, Sixtyfour's default), subset
  to Latin, Latin-1, Latin Extended-A and common punctuation, hinting
  dropped, GPOS `kern` kept: `games/drawbluff/fonts/` (18–31 KB each,
  OFL.txt next to them) and the test font
  `crates/tvpipe-canvas/tests/fonts/plex-sans.ttf` (16 KB).
- Known answers come from fontTools, not from the renderer itself
  (`tests/fonts/plex-sans.txt`): glyph ids, advances, the outlines' exact
  areas (`AreaPen`) and bounds (`BoundsPen`) and GPOS kerning. At 1000
  px/em the coverage sum matches the area within 0.3 % and the ink box
  the bounds within a pixel; at 32 px within 2 %. A rendered line is
  checked against `tests/fonts/line.pgm` (`TVPIPE_BLESS=1`); truncated
  and byte-mutated fonts parse, outline and draw without panicking.
- Speed (`examples/frame.rs`, release): a busy 1280×720 frame with two
  drawings of 6 000 points, 40 lines of text and a QR grid in 6.5 ms on
  one Apple M-series core.

## 6. Shipping and security

### 6.1 Platform prompts and signing

| | Tunnel mode, WebSocket only | LAN mode / WebRTC fast path |
|---|---|---|
| Listeners | 127.0.0.1 only (TCP) | a LAN address: TCP (pages, WebSocket) and UDP (ICE/DTLS) |
| **Windows** | no firewall prompt (loopback); outbound to TVs as tvpipe today | Windows Defender Firewall asks on the first non-loopback listen ("allow on private networks"), signed or not; declined → phones cannot connect (LAN mode fails, the fast path falls back). The game should listen once at start, not mid-game, so the prompt comes up before people scan |
| **macOS** | Local Network permission for the TVs (tvpipe already: `NSLocalNetworkUsageDescription`, `NSBonjourServices` for `_airplay._tcp`, `_raop._tcp`, `_googlecast._tcp` in a bundled game's `Info.plist`) | the same, plus the application firewall (off by default; when on, it asks to accept incoming connections for unsigned apps and allows signed ones). Whether Local Network permission also gates answering inbound LAN connections is to check on hardware |
| **Linux** | nothing | a host firewall (ufw, firewalld) may block the port silently: print the port and the protocol on start; Flatpak needs network access |

Signing: a downloaded macOS binary must be Developer ID signed and
notarized or Gatekeeper refuses it (a CLI tool needs no bundle, but the
Local Network prompt then names the terminal that ran it; a game shipped
as an `.app` gets its own name in prompts). On Windows, Authenticode
signing avoids SmartScreen's "unknown publisher" page; the firewall
prompt shows the file description, so set it. tvpipe-party documents
this in a "Shipping a party game" page next to `rust-sdk-design` §6.6;
the repository's own `drawbluff` is built by CI unsigned, like `tvpipe`.

### 6.2 Security, carried over and new

Everything in party-design §6 holds unchanged in Rust: 128-bit keys
from `ring::rand`, compared in constant time (`ring::constant_time`);
the join key in the fragment; Host and Origin checks (now with the LAN
case, §3.2); `hello` within 5 s; 64 sockets per server; failed `hello`s
throttled per address; message size and rate limits; names cleaned
(§1.6); text drawn as text (DOM `textContent`, canvas glyphs) never as
markup; the CSP without inline scripts; the `moderate` hook; kicks by
the VIP and the host. The Rust port reuses the session transcripts
(§1.5) so the limits behave identically, and adds the crate to the fuzz
smoke tests (HTTP requests, WebSocket frames, JSON, actions).

New with the fast path:

- **DTLS is mandatory in WebRTC** (DTLS 1.2, ECDHE, AES-GCM), so data
  channels are encrypted even in LAN mode, where the WebSocket is not.
  The host makes a fresh self-signed certificate per run.
- **Fingerprints are bound to a player** (§2.4): the SDP with the
  fingerprints only travels inside an authenticated WebSocket session;
  a channel belongs to the player whose session negotiated it. ICE
  credentials (ufrag/pwd) are per peer connection, so a stray STUN
  packet from the LAN cannot attach to someone else's channel.
- **UDP exposure**: one socket per interface, answering STUN only for
  known ufrags (no amplification: a binding response is not larger than
  the request plus the address), rate-limited per source address like
  joins; DTLS handshakes only after a valid ICE check; at most one peer
  connection per player and the socket cap overall.
- **Address disclosure**: host candidates reveal the host's LAN
  addresses to players (who are on that LAN anyway); with STUN (later,
  opt-in) the public address too, which the tunnel otherwise hides.
- **LAN mode is plaintext HTTP** (§3.2): keys can be sniffed on the
  Wi-Fi; said on the TV and in the docs, tunnel mode recommended on
  networks with strangers.

## 7. Per-player video on phones (design only)

Some games want a private picture per phone (your own hand of cards is
DOM, but your own camera in a 3D world, a map only you see, a second
screen with the game's own art). Options, cheapest first:

1. **Don't stream; render on the phone.** The phone gets its view as
   state and draws it in DOM or canvas (drawbluff does). Covers most
   party games, costs nothing, crisp at any resolution. The default
   recommendation; video only when the picture is genuinely made on the
   host (a 3D render).
2. **tvpipe's browser player per phone.** Each phone is a `browser`
   receiver of its own `Stream` (`to("browser")`, tvpipe's player page
   and WebSocket, WebCodecs on the phone), embedded in the controller
   page as an iframe or a `<canvas>` fed by the same code. Exists today
   (the `player` feature), latency ~100 ms + network, one H.264 encode
   per phone (at 360p the built-in encoder's cost is small; 8 phones at
   once is to measure). Needs a secure context for WebCodecs on some
   browsers (to check: Safari), so tunnel mode or the hybrid.
3. **WebRTC video tracks** on the same peer connection as the data
   channel (`rtc` has RTP/SRTP and H.264 packetisation): the lowest
   latency (jitter buffer ~50 ms), congestion control and NACK built in,
   hardware decode on every phone. Needs our encoder to produce
   constrained-baseline H.264 that phones' WebRTC accepts and to honour
   keyframe requests (tvpipe's encoder already does both for AirPlay
   and Cast).
4. **Chrome as the peer** (§2.3) with `captureStream()` from a canvas
   per player: hardware encoding, but the game would render in a
   browser, which is not the Rust-first story.

Recommendation when it is wanted: 2 as the first experiment (no new
code paths, measures the encode budget), 3 as the real thing on top of
the plan step 7 fast path.

## 8. Plan

Smallest first; each step committed green (`cargo fmt`, `clippy -D
warnings`, `cargo test --workspace`, and `npm test` in `npm/party` and
`games/drawbluff` where touched), never pushed.

1. **tvpipe groundwork** (small, no new behaviour for the CLI): public
   `tvpipe::qr` and `tvpipe::Tunnel` (+ `Tunnel::start_auto`) and `tvpipe::QrCode` as planned in
   party-design §3; `#[doc(hidden)] pub mod __web` (http, httpd,
   websocket, access, iface, json); `httpd` with an explicit LAN bind and
   a configurable connection cap; the access rule's LAN case. Tests for
   each.
2. **Vectors from Node**: the built `@tvpipe/party` is the oracle.
   `crates/tvpipe-party/tests/vectors/` with `diff.txt`, `names.txt`,
   `sessions/*.txt` written by a Node script against the Node server, and
   replayed by `npm/party`'s tests; the shared client moves to
   `crates/tvpipe-party/web/` with the byte-compare tests.
3. **`tvpipe-party` over WebSocket, loopback and tunnel**: server,
   rooms, players, views and diffs, actions with acks and replay,
   `Collect`, limits, names, assets, the join link and QR; the vectors
   pass; an example game (`examples/buzzer.rs`: a lobby, a button, the
   fastest player on the terminal) with headless Chrome as the phone in
   a live script.
4. **LAN mode and `Join::Auto`**: the bind, the link, the access rule,
   `require_secure_context`; measure LAN RTT next to §2.1's tunnel
   numbers; check on real phones (iOS Safari, Android Chrome) what
   plain HTTP on a private address shows, and the Windows firewall
   prompt.
5. **`tvpipe-canvas`**: canvas, paths and the accumulator, strokes,
   TrueType, text, QR; known-answer and fuzz tests.
6. **drawbluff in Rust**: rules with the shared vectors (TS tests gain
   them too), the game on `Party`, the renderer, sound cues, bots,
   `--web-screen`; the live check with `--host rust` for both screens;
   then a real TV and a real phone if the owner wants.
7. **WebRTC fast path** (after question 1): feature `webrtc` on `rtc`
   with the `FastPath` seam, signaling messages, the `in` message, the
   client's upgrade and fallback, a datagram fuzz target, LAN latency
   and loss measurements (Wi-Fi with a phone, the `u` channel against
   the WebSocket), the hybrid mode.
8. **Docs and release**: `crates/tvpipe-party/README.md`,
   `docs/party-protocol.md` (protocol v1 + the `rtc`/`in` additions split
   out of party-design §5), a "Shipping a party game" page (§6.1), the
   manual's party chapter for Rust, the release workflow publishing the
   new crates with the others.

Later, not planned: per-player video (§7, option 2 first), STUN
opt-in, WASM controllers (game code compiled for the phone).

## 9. Questions for the owner

1. **Dependencies for WebRTC**: may `tvpipe-party` take `rtc` 0.21
   (ring backend, 148 crates, +2.1 MB) as an optional dependency behind
   the `webrtc` feature? Alternatives: `str0m` (C library, +4.3 MB, no
   ring), a hand-rolled stack (several thousand lines, SCTP), Chrome as
   the peer (an installed browser).
2. **Async rule**: keep "no async runtime" for `tvpipe-party` (and
   therefore `rtc`, not `webrtc`)? Recommended: yes.
3. **Crates and names**: `tvpipe-party` and `tvpipe-canvas` as separate,
   published crates (Apache-2.0, versioned with tvpipe)? Or the canvas
   as a module of the party crate, or unpublished inside drawbluff?
4. **Coupling**: `tvpipe-party` uses tvpipe internals through a
   doc-hidden `__web` module with an exact version pin, rather than a
   public `tvpipe::web` API. Agreed?
5. **Emoji on the TV**: drop them from names on the TV (phones show
   them), or embed a monochrome emoji font (~1 MB more)?
6. **HTTPS on the LAN**: is a hosted name service (per-install names
   with real certificates, like plex.direct) ever wanted, or is "hybrid
   when online, plain HTTP when offline" the answer?
7. **Default join mode**: `Join::Auto` (tunnel, LAN when offline), or
   LAN first (no third party sees the game, faster, but no secure
   context and no mobile-data players)?
8. **JSON in game code**: hand-rolled `Value` + `json!` only, or also an
   optional `serde` feature for games that already use serde?
9. **drawbluff**: the Rust version next to the Node one in
   `games/drawbluff/` (shared `web/`, shared vectors), with both kept
   working? Or does the Rust one replace the Node host eventually?
10. **Node as the oracle** for the first protocol vectors (step 2), so
    the Rust server is built against the implementation that already
    passed live checks?

## 10. Decisions

The owner's answers to §9 (2026-11-19):

1. **`rtc` is allowed** as an optional dependency of `tvpipe-party`
   behind the `webrtc` feature, never in `tvpipe` itself.
2. **No async** in `tvpipe-party` (nor in `tvpipe-canvas`): threads and
   blocking I/O, sans-IO `rtc` driven from our threads.
3. **`tvpipe-party` and `tvpipe-canvas` are separate published crates**,
   versioned in lockstep with `tvpipe`.
4. **The hidden `tvpipe::__web` module** with an exact version pin
   (`tvpipe = { version = "=0.x.y" }`) is how the party crate reaches
   tvpipe's HTTP/WebSocket code.
5. **Emoji are stripped on the TV** (drawn names lose them; phones,
   being DOM, show them). No emoji font.
6. **No HTTPS-on-LAN name service.** Hybrid when online, plain HTTP
   when offline.
7. **Default join mode: tunnel**, the LAN link when no tunnel comes up
   (offline). `Join::Auto` as in §3.
8. **Not serde: [`deser`](https://crates.io/crates/deser)** (the owner's
   crate) as an optional feature for game-defined message types; the
   hand-rolled `Value` and `json!` stay the default (§10.1).
9. **Both drawbluff hosts stay** (Node and Rust), sharing `web/`, the
   prompts and the rule vectors.
10. **Node is the reference** for the first protocol vectors (§8 step 2).

The dependency policy for the new crates therefore reads: `tvpipe`
keeps `ring` and optional `rustls` (and `argument` in `tvpipe-cli`);
`tvpipe-party` depends on `tvpipe`, `ring`, and optionally `rtc`
(`webrtc`) and `deser` (`deser`); `tvpipe-canvas` on nothing.

### 10.1 `deser` integration

State checked 2026-11-19: `deser` 0.10.2 (Apache-2.0, MSRV 1.88,
"experimental" in its README), a facade over `deser-core` with the
derive macros behind the `derive` feature. Its tree is small: `deser`,
`deser-core`, and with `derive` `deser-derive` + `syn` 3, `quote`,
`proc-macro2`, `unicode-ident`. Serialization is event based
(`Serialize::serialize` returns an `Emit` of atoms, sequences and maps;
a `Serializer` drives the events, a `Deserializer` feeds them to a type's
sink through a `DeserializeDriver`), it does not recurse on the stack,
rejects duplicate keys by default and has limits in a `Context`: a good
fit for untrusted phone data. `deser-value`'s `convert.rs` (≈ 430 lines)
is the model for converting between a dynamic value and typed data.

Design (feature `deser` of `tvpipe-party`: `deser` with `derive`, and
`deser-json`; Rust 1.88, deser's minimum, while the workspace stays at
1.85 without the feature):

- `tvpipe_party::to_value(&T) -> Result<Value, deser::Error>` (`T:
  Serialize`) and `from_value::<T>(&Value)` (`T: DeserializeOwned`).
  **As built** they go through JSON text (`deser_json::to_string` and
  our parser, our serializer and `deser_json::from_str`): one small
  string per message at party rates, no dependency on deser's
  driver/sink internals. A direct conversion (a `Serializer` building a
  `Value`, a `Deserializer` emitting its events, like `deser-value`'s
  `convert.rs`) and `Serialize`/`Deserialize` impls for `Value` itself
  are a later optimisation if profiles ask for it.
- Typed conveniences on top: `action.data_as::<T>()` (a conversion error
  becomes `PartyError("bad-data", deser's message)` to reply with), and
  `State::set_typed(&T)`, `replace_typed(&T)`, `get_typed::<T>()` for
  the room, the screen and players' own states.
- Without the feature the same game code is written with `Value`
  accessors and `json!`; the wire format is identical, so a phone cannot
  tell which a game uses.
- The `deser` version is pinned to its minor (`0.10`): it is
  pre-1.0 and experimental, and only the conversion module touches it,
  so a breaking `deser` release is a one-file change here.
