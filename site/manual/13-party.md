---
title: Party games
summary: Games on the TV that everyone plays with their phone.
group: Use
spec:
  Package: `@tvpipe/party` (npm)
  Crate: `tvpipe-party` (crates.io)
  Node: 22 or newer
  Rust: 1.85 or newer
  Example: `games/drawbluff`
---

The `@tvpipe/party` library (Node) and the `tvpipe-party` crate (Rust) are for games like the ones from Jackbox: the TV shows the game and a QR code, and everyone joins by scanning it with their phone. The phones open a web page the game provides, over a public link, so nobody installs anything. Both speak the same protocol to the same browser client.

```sh
$ npm install @tvpipe/party @tvpipe/sdk
$ cargo add tvpipe-party tvpipe --features tvpipe/signals
```

## What the library does

:::aside The split
tvpipe streams to the TV and provides the public link. The library handles players. The game brings its rules and its pages.
:::

- Serves the game's pages for the phones and for the TV.
- Puts them on a public link and makes its QR code, with `tvpipe tunnel` and `tvpipe qr`.
- Keeps track of players: names, the VIP who starts the game, and phones that drop out and come back.
- Sends each phone what it should show, and passes what players do back to the game.
- Puts the main screen on the TV, as a web page or as video the game draws.

## A small game

```js
import { createParty, PartyError } from "@tvpipe/party";

const party = await createParty({ phone: "web/phone", screen: "web/screen" });
await party.toTv({ to: "Living Room", mode: "web" });

party.screen.set({ phase: "lobby" });
party.handle("start", async (data, player) => {
  if (!player.vip) throw new PartyError("not-vip", "Only the VIP can start");
  const deadline = party.now() + 60_000;
  party.room.set({ phase: "answer", deadline });
  const answers = await party.collect("answer", { until: deadline });
  party.screen.set({ phase: "results", answers: [...answers.values()] });
});
```

The phone's page loads the library's small browser client:

```js
import { PartyClient } from "/party/client.js";

const party = PartyClient.fromLocation();
party.on("view", (view) => render(view));
await party.join("Ann");
await party.act("answer", { text: "a nervous cactus" });
```

The game never sends events to phones. It sets what each phone shows: `party.room` for every player, `player.state` for one player, and `party.screen` for the TV. A phone that comes back after its screen was locked gets everything it missed. Actions survive a dropped connection and are applied once.

For a stick or a tilt, `party.input(data)` on the phone sends continuous input: never answered, the latest wins. The host gets it as the `input` event.

## In Rust

:::aside One binary
The pages go into the binary with `assets!`. The game streams its main screen with the `tvpipe` crate and serves the phones itself.
:::

The Rust game owns the party and runs it from its own loop, without an async runtime:

```rust
use std::time::Duration;
use tvpipe_party::{assets, json, Assets, Event, Party, PartyError, PartyOptions};

static PHONE: Assets = assets!("web/phone", ["index.html", "phone.js"]);

let mut party = Party::start(PartyOptions::new().phone(&PHONE))?;
println!("join: {}", party.join_url());
party.room().set(json!({ "phase": "lobby" }));
let waker = party.waker();
tvpipe::signals::on_signal(move || waker.wake());
while !tvpipe::signals::requested() {
    for event in party.wait(Duration::from_millis(50)) {
        if let Event::Action(a) = event {
            let reply = match a.kind() {
                "start" => Ok(json!(null)),
                _ => Err(PartyError::new("unknown-action", "Not now")),
            };
            party.reply(a, reply);
        }
    }
    party.flush();
}
party.shutdown(Duration::from_secs(2));
```

`party.join_qr()` gives the QR code to draw on the TV. `party.screen_watch()` hands the screen's view to the thread that renders video frames, for example with the `tvpipe-canvas` crate. With tvpipe's `signals` feature, Ctrl-C ends the loop, so the TV leaves mirroring mode and the phones learn that the room closed.

### Faster on the same Wi-Fi

With the crate's `webrtc` feature, phones that can reach the computer on the local network also talk to it over WebRTC data channels, next to the WebSocket. Actions take under a millisecond there instead of tens of milliseconds through a tunnel, and continuous input is never held up by a lost packet. Phones elsewhere, or when anything goes wrong, keep using the WebSocket without noticing.

## The main screen

| `mode` | The TV shows |
|---|---|
| `"web"` | The game's screen page, in a browser of tvpipe's own (see [A web page on the TV](web-page.md)). |
| `"video"` | Frames the game draws into a canvas, see [From Node.js](node.md). |

The screen gets the join link and its QR code with everything else it shows. Without `to`, open `party.links.screen` in any browser, for example on a laptop connected to the TV. `to: "browser"` shows it in a browser tab.

## The link

:::aside Providers
`tunnel: "auto"` uses Cloudflare and falls back to OpenTunnel when Cloudflare refuses.
:::

The QR code holds the public link with the room's code and a key, so only people who can see the TV can join. People elsewhere can play too: they join with the same link and watch the screen with `party.links.watch`. `tunnel: false` keeps everything on this computer.

A Rust game can also stay on the local network (`Join::Lan`): no third party sees the game, but the phones must be on the same Wi-Fi and the page is plain HTTP. When no tunnel comes up, the default falls back to it. Listening on the network brings up the firewall prompt on Windows; [docs/shipping-party-games.md](https://github.com/mitsuhiko/tvpipe/blob/main/docs/shipping-party-games.md) covers prompts and signing.

## Safety

- The library only accepts requests for its public link and for this computer.
- Names are cleaned up and must be unique. A `moderate` function can refuse words you do not want on the TV.
- The VIP and the game can remove players.
- With Cloudflare, Cloudflare can read the game's traffic. With OpenTunnel, nobody else can.

## The demo game

`games/drawbluff` in the repository is a drawing and bluffing game for 3 to 8 players, with two hosts: Node and Rust. Both run the main screen either way.

```sh
$ node main.ts --to "Living Room"           # the screen page
$ node main.ts --to "Living Room" --video   # video frames
$ node main.ts --to browser --bots 3 --fast # alone, with fake players
$ cargo run --release -p drawbluff -- --to "Living Room"                # video drawn in Rust
$ cargo run --release -p drawbluff -- --to "Living Room" --web-screen   # the screen page
```

The protocol is in [docs/party-protocol.md](https://github.com/mitsuhiko/tvpipe/blob/main/docs/party-protocol.md), the designs in [docs/party-design.md](https://github.com/mitsuhiko/tvpipe/blob/main/docs/party-design.md) and [docs/multiscreen-design.md](https://github.com/mitsuhiko/tvpipe/blob/main/docs/multiscreen-design.md).
