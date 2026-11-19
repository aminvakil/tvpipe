# tvpipe-party

Party games on [tvpipe](https://crates.io/crates/tvpipe), hosted in Rust:
the TV shows the game and a QR code, everyone joins by scanning it with
their phone, and the phones open the game's own web page. The host keeps
the room, its players and what each of them sees in sync. No async
runtime: the game owns the `Party` and drives it from its own loop.

```rust
use std::time::Duration;
use tvpipe_party::{assets, json, Assets, Event, Party, PartyError, PartyOptions};

static PHONE: Assets = assets!("web/phone", ["index.html", "phone.js"]);

let mut party = Party::start(PartyOptions::new().phone(&PHONE))?;
println!("join: {}", party.join_url());       // and party.join_qr() for the TV
party.room().set(json!({ "phase": "lobby" }));
loop {
    for event in party.wait(Duration::from_millis(50)) {
        match event {
            Event::Joined(id) => println!("+ {}", party.player(&id).unwrap().name()),
            Event::Action(a) => {
                let reply = match a.kind() {
                    "start" => { party.room().set(json!({ "phase": "draw" })); Ok(json!(null)) }
                    _ => Err(PartyError::new("unknown-action", "Not now")),
                };
                party.reply(a, reply);
            }
            Event::Input { player, data } => { /* a stick, a tilt: the latest per poll */ }
            _ => {}
        }
    }
    party.flush();   // one diff per phone, then the acks
}
```

The phone page loads the browser client the party serves:

```js
import { PartyClient } from "/party/client.js";

const party = PartyClient.fromLocation();
party.on("view", (view) => render(view));      // { roster, room, me }
await party.join("Ann");
await party.act("start");                      // acked, applied once across reconnects
party.input({ x: 0.3, y: -1 });                // continuous input, latest wins
```

## What it does

- **Serves** the game's pages, embedded in the binary (`assets!`), and
  the client at `/party/client.js`.
- **Links**: a public HTTPS link through a tunnel (`Join::Auto`, the
  default: Cloudflare, else OpenTunnel, else the LAN link when offline),
  the LAN only (`Join::Lan`, plain HTTP), or this computer
  (`Join::Local`). `join_url()` and `join_qr()` for the TV.
- **Players**: names (cleaned, unique), the VIP, phones that drop out and
  come back as the same player, `lock()`, `kick()`, a `moderate` hook.
- **Views, not events**: the game sets `room()` (every player),
  `player_state(id)` (one player) and `screen()` (the TV); each phone
  gets a diff of what changed. `screen_watch()` hands the TV's view to a
  renderer thread (for a video screen drawn with `tvpipe-canvas`).
- **Actions** with acks, exactly once across reconnects; `Collect` waits
  for one answer per player or a deadline.
- **Limits**: keys in the link's fragment, Host and Origin checks, rate
  and size limits per socket, join throttling per address; sockets per
  server follow `max_players` (`sockets_for`: 2 per player + 16, at least
  64).
- **At 50 phones**: a flush diffs only the view sections that changed,
  each shared section once for all phones on the same version;
  `Party::stats()` counts flushes, `state` messages, bytes and time.

## Features

| Feature | Default | Brings |
|---|---|---|
| `tunnel` | yes | public links through Cloudflare or OpenTunnel (`tvpipe/tunnel`) |
| `webrtc` | no | the WebRTC fast path: data channels to phones on the LAN beside the WebSocket (the `rtc` crate, ~140 crates, +2.9 MB; Rust 1.89) |
| `deser` | no | game-defined message types with the `deser` crate (Rust 1.88) |

With `webrtc`, phones that can reach the host's LAN address upgrade to
two data channels (reliable `r`, unreliable `u`); the WebSocket stays and
takes over whenever they are not open. Through a tunnel this is the
hybrid mode: the page over the tunnel, the game's messages on the LAN.
`PartyOptions::fast_port` (UDP, default 7420) is the port for firewall
rules; `Party::fast_path(id)` says whether a phone uses it.

## Ctrl-C

Use tvpipe's `signals` feature so a game ends cleanly (the TV leaves
mirroring mode, the phones learn the room closed):

```rust
let waker = party.waker();
tvpipe::signals::on_signal(move || waker.wake());
while !tvpipe::signals::requested() { /* the loop */ }
party.shutdown(std::time::Duration::from_secs(2));
```

## More

- The protocol (shared with the npm package `@tvpipe/party`, whose Node
  host is the reference; both replay the same vectors in
  `tests/vectors/`): `docs/party-protocol.md` in the repository.
- Shipping a game (signing, firewall prompts, licenses):
  `docs/shipping-party-games.md`.
- Examples: `examples/buzzer.rs` (the fastest finger),
  `examples/stick.rs` (joysticks, with `webrtc`). A whole game:
  `games/drawbluff` in the repository.
