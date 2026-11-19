# @tvpipe/party

Party games on [tvpipe](https://mitsuhiko.github.io/tvpipe/): the TV shows
the game and a QR code, everyone plays with the browser on their phone.
This package is the shared part:

- a server on 127.0.0.1 for the game's pages (phones at `/`, the TV page
  at `/screen/`) and a WebSocket for everything else,
- the public link through `tvpipe tunnel` (Cloudflare or OpenTunnel) and
  its QR code (`tvpipe qr`), both from
  [`@tvpipe/sdk`](../tvpipe/README.md),
- rooms with a code and a join key, players with names, a VIP, phones that
  drop out and come back,
- **views** the host sets and phones show, **actions** phones send and the
  host handles (acknowledged, resent after a reconnect, applied once),
- the main screen on a TV: a web page in tvpipe's own browser, or frames
  the game draws.

Node 22 or newer, no dependencies besides `@tvpipe/sdk` (a peer
dependency, needed only for the link, the QR code and the TV). Design,
protocol and measurements: [`docs/party-design.md`](../../docs/party-design.md).
A whole game on it: [`games/drawbluff`](../../games/drawbluff).

## A game in a few lines

```js
import { createParty, PartyError } from "@tvpipe/party";

const party = await createParty({
  phone: new URL("./web/phone/", import.meta.url),
  screen: new URL("./web/screen/", import.meta.url),
  tunnel: "auto",              // Cloudflare, else OpenTunnel; false: this computer only
  maxPlayers: 8,
});
console.log(party.links.join); // the QR code's link
await party.toTv({ to: "Living Room", mode: "web" });

party.screen.set({ phase: "lobby" });
party.handle("start", (data, from) => {
  if (from.role !== "player" || !from.vip) throw new PartyError("not-vip", "Only the VIP can start");
  void round();
});

async function round() {
  party.lock();
  const deadline = party.now() + 60_000;
  party.room.set({ phase: "answer", deadline });
  party.screen.set({ phase: "answer", deadline });
  const answers = await party.collect("answer", { until: deadline });
  party.screen.set({ phase: "results", answers: [...answers.values()] });
}
```

A phone page (`web/phone/index.html` and a module):

```js
import { PartyClient } from "/party/client.js";

const party = PartyClient.fromLocation();          // the room and key from the link
party.on("view", (view) => render(view));          // { roster, room, me }
if (party.canResume) await party.connect();        // this phone played before
else await party.join(prompt("Your name?"));
button.onclick = () => party.act("answer", { text: input.value }).catch((e) => show(e.message));
```

A screen page uses the same client with `#role=screen` in its link
(`party.links.screen`); its view is `{ roster, screen, join }`, and
`qrSvg(view.join.qr)` draws the join link's QR code.

## Host API

`createParty(options)` starts a `PartyServer`, creates one `Party`, opens
the tunnel and makes the QR code. Options: `phone`, `screen` (directories),
`tunnel`, `qr`, `port`, `maxPlayers`, `moderate`, `limits`, `grace`, `csp`,
`log`/`logLevel`, `tvpipe` (options for every tvpipe process: `binary`,
`configDir`, `env`).

| | |
|---|---|
| `party.room`, `party.screen`, `player.state` | `StateBox`es: `set(partial)` (`undefined` removes a field), `replace(value)`, `update(draft => …)`, `value`. Changes in one tick reach each client as one diff. |
| `party.handle(type, (data, from) => result)` | Actions of a type; throw `PartyError(code, message)` to refuse. `from` is a `Player` or a screen (`from.role`). |
| `party.collect(type, { from, until, validate, replace, skipAway, reply, onAnswer, signal })` | One action per player, until all answered or the deadline (+ `grace`, 500 ms). Resolves with `Map<Player, value>`. |
| `party.send(player, type, data)`, `party.broadcast(type, data, role)` | Transient messages (`msg` events on phones); lost while a phone is away, so anything that matters goes into a view. |
| `party.players`, `party.connectedPlayers`, `party.vip`, `party.player(id)` | Players in join order; `player.id`, `name`, `vip`, `connected`, `data` (yours). |
| `party.lock()`, `unlock()`, `kick(player)`, `close()` | No new players (resumes still work), remove one, end the party. |
| `party.moderate(text, kind, player)` | Your `moderate` hook for any text the game shows (`false`: refused). |
| `party.now()` | The server clock (ms); deadlines in views use it, phones are synced to it. |
| `party.screenView()` | `{ roster, screen, join }`: what a video renderer draws. |
| `party.links` | `join` (public), `localJoin`, `screen` (local, with the screen key), `watch` (public, read-only screen). |
| `party.toTv({ to, mode: "web" })` | The screen page in tvpipe's browser (`share URL`). |
| `party.toTv({ to, mode: "video", canvas, render, fps, audio })` | `render(ctx, view, t)` at the frame rate into a canvas (e.g. `@napi-rs/canvas`), streamed with `stream()`. |
| `party.shutdown()` | TV sessions, the party, the server and the tunnel. |
| events | `join`, `reconnect`, `away`, `leave`, `vip`, `screen`, `screen-view` |
| `drawQr(ctx, qr, x, y, size)`, `qrToRgba(qr, scale)` | The QR code for a canvas or raw frames. |

Lower level: `new PartyServer({ phone, screen, port })`, `listen()`,
`allowHost(host)` (a tunnel's), `createParty()`, `route(method, path,
handler)` for your own HTTP endpoints.

## Browser client

`/party/client.js` (also `@tvpipe/party/client`, which runs in Node for
tests and bots): `PartyClient.fromLocation()` or `new PartyClient({ url,
room, key, role, storage })`; `join(name)`, `connect()`, `act(type, data)`
(rejects with `PartyActionError`: `code`, `message`), `on("view" | "msg" |
"status" | "bye" | "welcome", fn)`, `view`, `you`, `status`, `now()`,
`leave()`, `close()`; `qrSvg(qr)`, `drawQr(ctx, qr, x, y, size)`,
`memoryStorage()`. It reconnects by itself (and when the page becomes
visible again), keeps the player's identity in `localStorage`, and
resends unacknowledged actions.

## Security

The server binds 127.0.0.1 only and accepts only loopback `Host`s and the
tunnel's (421 otherwise); WebSockets need a matching `Origin` (403
otherwise); joining needs the join key from the QR code's link (in the
fragment, so it is in no request line or log), screens their own keys.
Per socket: 20 messages/s (burst 50), 256 KiB per message, `hello` within
5 s; failed joins are throttled per address; at most 64 sockets. Names are
normalised (NFC, invisible and control characters removed, 16 characters)
and unique; render everything players send as text. HTML pages get a
strict Content-Security-Policy (`csp` to change it).

## Tests

`npm test`: the WebSocket server against Node's client and hand-made
frames, static files and the Host/Origin/key checks, joins, names, VIP,
resumes, replayed actions, `collect`, rate limits and slow phones, the
view diffs (property test against the client's merge). Live (a real
tunnel): `TVPIPE_BINARY=… node test/live-rtt.ts cloudflare|opentunnel`.
