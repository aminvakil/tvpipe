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

## 2. Transport

### 2.1 What latency is there to win

| Path (phone → host → phone) | RTT | Source |
|---|---|---|
| WebSocket through a Cloudflare quick tunnel | median 16.5–17.1 ms, p95 19.7 ms, max 26 ms | party-design §13 (client on the host's Mac, so the internet is crossed twice, as for a phone on the same Wi-Fi) |
| WebSocket through OpenTunnel | median 43 ms, p95 47 ms | same |
| WebSocket over the LAN (LAN mode, §3.2) | Wi-Fi: typically 2–10 ms, spikes of 50–200 ms when a phone's Wi-Fi power saving kicks in | to measure in phase 2 |
| WebRTC data channel over the LAN | same Wi-Fi floor; no head-of-line blocking on unordered channels | to measure in phase 4 |

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
   data channel), unless measurements in phase 4 say otherwise.

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
and whether iOS does anything similar is to check on devices in phase 2.

**HTTPS on the LAN** was considered and left out: self-signed
certificates (a full-page warning on every phone), installing a local CA
(not for party guests), and a real certificate for a per-install name
that resolves to the LAN address (the plex.direct approach: a DNS zone
and ACME on a service we would have to run). The hybrid mode (§3.1)
gives HTTPS plus LAN latency whenever there is internet; question 6 in §9
asks whether a hosted name service is ever wanted.
