# playshare: web games on the TV (design)

Status: implemented (`crates/tvpipe/src/playshare/`, `npm/party`,
`games/drawbluff`); what changed while building it is in §16. Supersedes
`docs/multiscreen-design.md` (Rust party host, WebRTC fast path, Rust
TV rendering) and the Node host of `docs/party-design.md` (§2 B, §4, §7,
§9). The wire protocol between phones and the game
(`docs/party-protocol.md`) stays.

Contents: 1. decisions, 2. the pieces, 3. the bundle, 4. listeners and
URLs, 5. the local API, 6. the relay, 7. the launcher, 8. the game
library, 9. casting, 10. lifecycle, 11. tunnels, 12. security, 13.
tests, 14. what goes, 15. plan.

## 1. Decisions (the owner's, final)

- A game is a **web app**: static files that run in browsers. No Node,
  no Rust game logic. tvpipe serves it: `tvpipe playshare ./game` or
  `tvpipe playshare ./game.zip`.
- The game's **host page** runs the game logic, in a visible browser
  window on the computer (an app window of tvpipe's own Chrome). Before
  it, a **launcher** in the same window picks the TVs (tvpipe's standard
  one, or the game's own on the same API).
- The **TV page** renders in a **headless** browser tvpipe starts on the
  same computer; tvpipe casts that tab (picture and sound) to the chosen
  AirPlay/Cast receivers: the `share URL` machinery, unchanged.
- **Phones** load the phone page through a tunnel (or the LAN); their
  WebSockets end in tvpipe, which **relays** them to the host page.
  tvpipe knows connections, bytes, limits and access rules; rooms,
  players, keys, views and actions live once, in the browser library
  `@tvpipe/party`, in the host page.
- Closing the window (or Ctrl-C) ends everything: the casts (TEARDOWN),
  the headless browser, the tunnel; phones are told the room closed.

## 2. The pieces

```
 ┌──────────── this computer ───────────────────────────────────────────┐
 │  app window (tvpipe's Chrome, visible)     headless Chrome           │
 │  ┌──────────────────────────┐              ┌──────────────────┐      │
 │  │ launcher → host page     │              │ TV page (screen) │──┐   │
 │  │ game logic, @tvpipe/party│              └───────┬──────────┘  │   │
 │  └──────┬───────────────────┘                      │ /_tvpipe/ws │   │
 │         │ /_tvpipe/api (API + relay)               │             │   │
 │  ┌──────┴──────────────── tvpipe playshare ────────┴───────────┐ │   │
 │  │ local listener 127.0.0.1:L   public listener 127.0.0.1:P    │ │   │
 │  │ bundle files, launcher, lib  bundle files, lib, /_tvpipe/ws │ │   │
 │  │ discovery, pairing, casting ◀── share session (capture.js) ◀┼─┘   │
 │  └──────────────────────────────────────▲────────┬─────────────┘     │
 └─────────────────────────────────────────┼────────┼───────────────────┘
                        tunnel (or LAN)    │        │ AirPlay / Cast
                                  phones ──┘        └──▶ TVs
```

| Piece | Where | Owns |
|---|---|---|
| `tvpipe playshare` | `crates/tvpipe/src/playshare/` (feature `playshare`, reached by the CLI through `__cli`), `crates/tvpipe-cli` (arguments) | the bundle (folder or zip), two HTTP listeners, the local API, the relay, discovery and pairing for the launcher, the cast (a `share URL` session), the app window, the tunnel, teardown |
| `@tvpipe/party` | `npm/party/` (TypeScript, browser-first; tvpipe embeds the built JS and serves it at `/_tvpipe/lib/`) | the host (`Party`: rooms, players, keys, VIP, late join, actions exactly once, views and diffs, clock), the phone/TV client (`PartyClient`), the local API client (`Tvpipe`), test helpers |
| standard launcher | `npm/party/launcher/` (built into `crates/tvpipe/src/playshare/web/launcher/`) | device list, AirPlay PIN entry, a "browser tab" receiver for trying without a TV, start |
| the game | a folder or zip with `tvpipe.json` | host page (rules), TV page, phone page, assets; optionally its own launcher |

## 3. The bundle

A folder or a `.zip` (stored or deflate entries; hand-rolled reader with
CRC checks, `crates/tvpipe/src/util/zip.rs`, inflate in
`util/inflate.rs`, known-answer tests) with `tvpipe.json` at its root
(in a zip: at the root or inside one top-level folder, as zipping a
folder makes it):

```json
{
  "name": "drawbluff",
  "title": "drawbluff",
  "root": "dist",
  "pages": {
    "host": "host/index.html",
    "tv": "screen/index.html",
    "phone": "phone/index.html",
    "launcher": null
  },
  "players": { "min": 3, "max": 50 },
  "tv": { "width": 1280, "height": 720 },
  "limits": { "message": 262144, "rate": 20, "burst": 50 }
}
```

| Field | | |
|---|---|---|
| `name` | required | `[a-z0-9-]{1,40}`: logs, the window's profile name |
| `title` | optional | the launcher's heading (default `name`) |
| `root` | optional | the served directory inside the bundle (default: where `tvpipe.json` is); lets a source folder point at its build output |
| `pages.host`, `pages.tv`, `pages.phone` | required | entry documents, paths relative to `root` |
| `pages.launcher` | optional | the game's own launcher (§7); default tvpipe's |
| `players.min`, `players.max` | optional | shown by the launcher, given to the host library; `max` (default 8, at most 1000) sizes the relay's connection limit |
| `tv.width`, `tv.height` | optional | the TV page's viewport (default: the receivers' shape, at most 1920x1080, as `share URL`) |
| `limits` | optional | the relay's per-connection limits (§6.3); bounded by tvpipe's maxima |

Unknown fields are ignored (forward compatibility); a missing or wrong
required field is an error naming the field. Paths are normalized: no
`..`, no absolute paths, no backslashes, no hidden (`.`) segments, and
files are read only below `root` (a folder's symlinks that leave `root`
are refused). A zip is read into memory once (at most 256 MiB unpacked,
at most 10 000 entries, every entry's CRC-32 checked; zip64, encryption
and other methods are refused with a clear error); a folder is read from
disk per request, so editing a game needs only a reload of the page.

## 4. Listeners and URLs

Two `net::httpd` servers, so the public side cannot reach the API even
by mistake:

| | Local listener `127.0.0.1:L` | Public listener `127.0.0.1:P` (+ LAN `IP:P'` with `--lan`) |
|---|---|---|
| Who | the app window, the headless TV page | phones (through the tunnel or the LAN), this computer for tests |
| `Host` | `127.0.0.1:L`, `localhost:L`, `[::1]:L` (421 otherwise) | the tunnel's name, loopback names with `P`; the LAN listener exactly `IP:P'` |
| `/` | redirect to the launcher | redirect to the phone page, the query kept (`/?r=CODE` → `/phone/index.html?r=CODE`; browsers keep the `#k=` fragment across the redirect) |
| bundle files | yes | yes (everything in a bundle is public: it is a web app; secrets do not belong in it) |
| `/_tvpipe/lib/*.js` | the library | the library (the client side is what phones use) |
| `/_tvpipe/ws` | relayed WebSocket (the TV page) | relayed WebSocket (phones) |
| `/_tvpipe/start?t=TOKEN` | sets the session cookie, redirects to the launcher | — (404) |
| `/_tvpipe/launcher/` | the standard launcher | — |
| `/_tvpipe/api` | the API WebSocket (token) | — |

Static files: GET/HEAD, a fixed MIME table, `Content-Length`,
`nosniff`, `Cache-Control: no-cache` with an ETag, `Referrer-Policy:
no-referrer`; HTML gets no CSP from tvpipe (the game's pages are the
game's: a game sets its own with `<meta http-equiv>`). `/_tvpipe/` is
reserved: a bundle's own `_tvpipe/` directory is never served.

## 5. The local API

**Auth.** tvpipe makes a 128-bit token per run. The window opens
`http://127.0.0.1:L/_tvpipe/start?t=TOKEN`, which answers with an
`HttpOnly; SameSite=Strict; Path=/` cookie `tvpipe=TOKEN` and a redirect
to the launcher, so the launcher and the host page (same origin,
navigations, reloads) carry it without handling it. Programs without a
cookie jar (tests, a host in Node) pass `?t=TOKEN` on the WebSocket URL.
The API WebSocket needs the token and, from browsers, `Origin`
`http://127.0.0.1:L` (403: cross-site WebSocket hijacking); compared in
constant time.

**Transport.** One WebSocket, `/_tvpipe/api`. Text messages are JSON:

- calls: `{"t":"call","n":7,"method":"cast.start","params":{…}}` →
  `{"t":"reply","n":7,"ok":true,"value":…}` or
  `{"t":"reply","n":7,"ok":false,"error":{"code":"…","message":"…"}}`;
- events from tvpipe: `{"t":"event","type":"devices","data":…}`;
- relay control (§6), on a socket that attached the relay.

Binary messages are relay data (§6). Unknown `t`, methods and fields:
ignored, or `unknown-method`.

| Method | Params → value | |
|---|---|---|
| `info` | → `{name, title, players, pages, tv, phone: {url, mode, lan}, tunnel, version, launcher}` | `phone.url`: the base phones use (`https://HOST/`, `http://IP:P'/` or `http://127.0.0.1:P/`), `mode` `tunnel`/`lan`/`local` |
| `devices.watch` | `{on: bool}` → the current list | discovery runs while any socket watches; `devices` events with the whole list when it changes |
| `select` / `selection` | `{devices: [name]}` → `{devices}` | the launcher's choice, kept by tvpipe for the host page (a navigation away) |
| `pair.start` | `{device}` → `{}` | AirPlay PIN pairing; `pair` events `{device, state: connecting\|pin\|done\|failed, error?}` |
| `pair.pin` | `{device, pin}` | the PIN the TV shows |
| `pair.cancel` | `{device}` | |
| `cast.start` | `{page, to?}` → `{}` | `page`: a path on the local origin with its fragment (the TV page and its screen key); `to`: device names (default: the selection), `"browser"` for a tab on this computer. Replaces a running cast. `cast` events `{state: starting\|streaming\|failed\|stopped, receivers: [{name, state}], player?, error?}` |
| `cast.stop` | | the TVs get TEARDOWN; the headless browser quits |
| `qr` | `{text}` → `{size, rows}` | tvpipe's QR encoder (`util::qr`): `rows` of `"0"`/`"1"` |
| `relay.attach` | `{}` → `{public, local}` | this socket becomes the relay host (§6); a newer attach replaces it |
| `quit` | | ends playshare (§10) |

Events also include `tunnel` (`{state: up|reconnecting, url}`) and
`closing` (tvpipe is ending: the host has 1 s to say goodbye).

## 6. The relay

### 6.1 Connections

A client WebSocket to `/_tvpipe/ws` (any query; tvpipe passes it on) is
accepted when a host is attached (else `503`), the `Host`/`Origin`
rules pass (§4; `Origin` must be the listener's own origin: `https://`
the tunnel's name, `http://IP:P'`, or the loopback origins), and the
connection limit is not reached. It gets a number (`u32`, never reused
in a run). tvpipe tells the host, on the API socket:

```json
{"t":"open","conn":12,"via":"public","addr":"203.0.113.9","query":"r=BCDF","agent":"Mozilla/5.0 …"}
```

`via`: `public` (tunnel or loopback on the public listener), `lan`,
`local` (the local listener: the TV page, or the host window's own
previews). `addr`: Cloudflare's `cf-connecting-ip`, OpenTunnel's peer,
or the socket's address.

### 6.2 Frames on the API socket

Data both ways is **binary**: `u8 op | u32be conn | payload`.

| op | tvpipe → host | host → tvpipe |
|---|---|---|
| 1 | the client sent a text message (UTF-8 payload) | send a text message to the client |
| 2 | the client sent a binary message | send a binary message |

Control is **text** JSON:

| `t` | Direction | Fields |
|---|---|---|
| `open` | → host | above |
| `close` | → host | `conn`, `code`, `reason`: the client's socket ended (its close frame's, or 1006) |
| `busy` / `drain` | → host | `conn`: more than 64 KiB wait to be written to this client / back under it |
| `close` | host → | `conn`, `code` (1000, 3000–4999), `reason` (≤ 120 bytes): tvpipe sends what is queued, then the close frame |

Order is kept: everything about one connection arrives in the order it
happened, and data the host sends before a `close` is written first.

### 6.3 Limits (generic, in tvpipe)

| | Default | Manifest | On breach |
|---|---|---|---|
| Message size from a client | 256 KiB | `limits.message` (≤ 1 MiB) | close 1009 |
| Messages per client | 20/s, 50 at once (token bucket) | `limits.rate`, `limits.burst` (≤ 200/s) | close 4008 |
| Connections | 2 × `players.max` + 16, at least 64 | via `players.max` | `503` at the upgrade |
| Upgrades per address | 60 a minute | — | `429` |
| Queued to one client | 1 MiB | — | close 4013 (the client reconnects) |
| Liveness | ping every 15 s | — | closed after 45 s of silence |
| First message | within 10 s of the upgrade | — | close 4002 |

The host library keeps the protocol's own rules on top (the `hello`
within 5 s, failed-`hello` throttling per address, 16 KiB for messages
other than `act`, …): tvpipe's limits are the outer bound that holds
even when a host page is slow or buggy.

### 6.4 Host changes and ending

- The host socket closing (a reload, a navigation, a crash) closes every
  client with **1012** (service restart): clients reconnect, and get
  `503` until a host attaches again. A host that loses its state on a
  reload therefore sends phones back to their name entry (`bye
  no-player`); the library may keep its state in `sessionStorage` later.
- A second `relay.attach` replaces the first (the old socket gets
  `{"t":"event","type":"detached"}` and is closed).
- At the end (§10) tvpipe sends `closing`, waits up to 1 s (the library
  sends every client `bye room-closed`), then closes every client with
  **4011** "closed": the code the client already treats as final, so
  phones show "the room closed" even when the window is gone.

## 7. The launcher

The window first shows **tvpipe's standard launcher**
(`/_tvpipe/launcher/`) unless `pages.launcher` names the game's own:

- the game's title and player counts (`info`),
- receivers as they are found (`devices.watch`), each with its kind
  (AirPlay, Cast, both), "pair" for AirPlay receivers that show a PIN and
  are not paired (`pair.start`, a PIN field, `pair.pin`), and
  **"A browser tab on this computer"** (`browser`) for trying without a TV,
- multi-select, then **Start**: `select`, then `location.assign(pages.host)`.

The contract for a game's own launcher (any page of the bundle, with
`@tvpipe/party`'s `Tvpipe` client): it may call any API method; it ends
with `select({devices})` and a navigation to the host page. The host
page casts (`party.cast()`, §8), because only the host knows the TV
page's screen key. A launcher may also skip TVs (an empty selection:
the TV page is opened in a tab or on an HDMI screen by hand, the host
shows its link).

## 8. The game library (`@tvpipe/party`)

TypeScript (erasable syntax only), compiled with `tsc` to ES modules;
the npm package ships `dist/` with types; `npm run build` also writes
the browser copy tvpipe embeds (`crates/tvpipe/src/playshare/web/`,
checked in, checked by CI to be up to date). Browser APIs only
(`WebSocket`, `crypto.getRandomValues`, `TextEncoder`, `Intl`,
`queueMicrotask`), so the same code runs in Node for tests.

### 8.1 Host side

```ts
import { Tvpipe, PartyError } from "/_tvpipe/lib/index.js";

const tvpipe = await Tvpipe.connect();                 // the API socket (cookie)
const party = await tvpipe.party({ maxPlayers: 50 });   // attaches the relay; links and QR
await party.cast();                                     // the TV page on the selected TVs
party.screen.set({ phase: "lobby" });
party.handle("start", (data, from) => { … });
const answers = await party.collect("answer", { until: party.now() + 60_000 });
```

`Party` keeps the Node host's API (`room`, `screen`, `player.state`
StateBoxes; `handle`, `collect`, `send`, `broadcast`, `players`, `vip`,
`lock`/`unlock`, `kick`, `close`, `inProgress`, `lateJoin`, `activate`,
`moderate`, `now`, `stats`, `screenView()`, events `join`, `reconnect`,
`away`, `leave`, `vip`, `active`, `screen`, `screen-view`, `input`) with
the transport replaced: it takes connections from a `Relay` (tvpipe's,
or `memoryRelay()` in tests) instead of a Node server. `party.links`:
`join` (public, in the QR code), `tv` (the local TV page with the screen
key), `watch` (a read-only screen, local: the host page may show it in
an `<iframe>` as a preview). `party.cast(to?)`, `party.stopCast()`;
`party.joinInfo` gets the QR rows from tvpipe's `qr`.

### 8.2 Client side

`PartyClient` (`/_tvpipe/lib/client.js`) is the existing browser client
ported to TypeScript without the WebRTC upgrade: `fromLocation()`,
`join(name)`, `connect()`, `act()`, `input()`, `on("view" | "msg" |
"status" | "bye" | "welcome")`, `now()`, `leave()`, `qrSvg()`,
`drawQr()`, `memoryStorage()`. Its default socket is
`/_tvpipe/ws` on the page's origin. Close codes it treats as final
without a `bye` (tvpipe's own closes): 4008, 4011, 1009. The protocol
(`docs/party-protocol.md`) is unchanged except: §9 (WebRTC) is gone,
the transport section names the relay, and the hosts table has one host.

### 8.3 `Tvpipe` (the API client)

`Tvpipe.connect({url?, token?})`, `info()`, `devices(on)` + the
`devices` event, `select()`, `selection()`, `pair(device)` (an object
with `state`, `submit(pin)`, `cancel()`), `cast({page, to})`,
`stopCast()`, `qr(text)`, `quit()`, `relay()` (attaches; returns the
`Relay`), events `devices`, `pair`, `cast`, `tunnel`, `closing`.

## 9. Casting

`cast.start` runs `tvpipe::share(Some(url), options)` with the
receivers: the session's own headless browser (Chrome, else Firefox;
`share URL`'s discovery), viewport from `tv` or the receivers' shape,
`capture.js` after every navigation, the crash restart and profile
sweep, the TVs' reconnect window. The URL must be on the local origin
(no other page can be cast through the API). The TV page reaches the
host through `/_tvpipe/ws` like any client (`via: local`). A cast that
fails (every receiver gone) is reported (`cast` event `failed`); the
host decides whether to retry (the library offers `party.cast()` again,
drawbluff shows the error and a button). `browser` casts to the player
page (`--to browser`): the event's `player` is its URL.

## 10. Lifecycle

1. `tvpipe playshare PATH [--lan [IP]] [--tunnel[=PROVIDER] | --no-tunnel] [--window app|open|none] [--browser PATH] [--port L]`:
   load and check the bundle (errors before anything starts), bind the
   listeners, start the tunnel (§11) in the background, open the
   window.
2. The window: tvpipe's Chromium-based browser (the discovery of
   `share URL`) with a temporary profile (swept like `share URL`'s),
   `--app=URL` (no tabs, no toolbar), 1100x760, over DevTools. Without
   one: `--window open` uses the system's opener (`platform::open_url`),
   `none` prints the URL (tests, a browser of one's choice).
3. The launcher, then the host page (§7, §8).
4. **End** on any of: the app window closed (no page target left, or
   the browser exited), `quit`, Ctrl-C/SIGTERM, `q` + Enter; with
   `open`/`none`, no API socket for 30 s after one was seen. Then:
   `closing` to the host (1 s), clients closed 4011, the cast stopped
   (TEARDOWN/STOP to every TV), the headless and the window browsers
   quit (profiles removed), the tunnel dropped, exit 0 (130 on a signal).
5. The tunnel reconnects by itself (same host); a tunnel that cannot
   start leaves phones on the LAN link if `--lan`, else on this
   computer only, with a warning in the launcher (`info.phone.mode`).

## 11. Tunnels

playshare starts often, so its default must not burn Cloudflare quick
tunnels (about 29 per address in 90 minutes, each a new hostname):

- **Default `--tunnel=auto`: OpenTunnel first.** Its identity is made
  once and kept in the config directory (`opentunnel/`); later runs
  attach in ~0.3 s with no creation at all (only a new random route).
  If OpenTunnel fails, Cloudflare. `--tunnel=cloudflare` asks for a
  quick tunnel explicitly.
- **Reusing quick-tunnel credentials** works: a quick tunnel outlives
  its connector (measured 2026-11-19: the same credentials registered
  again after 120 s and the same hostname served at once,
  `cftunnel::tests::live_quick_tunnel_reuse`). So when playshare uses
  Cloudflare it keeps the last quick tunnel's credentials in the config
  directory (`cloudflare-quick-tunnel.json`, 0600: the tunnel secret)
  and tries them first (`cftunnel::Tunnel::start_reusing`: register and
  resolve within 10 s, else a new quick tunnel, whose credentials replace
  them). A run on reused credentials is up in under a second instead of
  6–7 s. How long Cloudflare keeps an idle quick tunnel is not known; a
  deleted one costs 10 s before the new one. The same hostname across
  runs is harmless: every run has a new room code and join key, so an old
  QR code leads nowhere.
- `--lan [IP]`: phones on the same Wi-Fi over `http://IP:PORT` (no
  internet needed; the key crosses the Wi-Fi in clear: party-protocol
  security notes); `--no-tunnel`: this computer only.

## 12. Security

- The **API** is local only (a listener of its own on 127.0.0.1), needs
  the per-run token (cookie or `?t=`), and from browsers the local
  origin. The token never reaches the public listener, the TV's page or
  any log at the default level.
- **DNS rebinding**: `Host` checked on both listeners (421). **CSWSH**:
  `Origin` checked on every WebSocket (403).
- **Relay**: tvpipe forwards bytes and enforces limits (§6.3); it never
  parses what clients send. Keys, names and actions are checked by the
  host library as before (party-design §6): 128-bit keys in URL
  fragments, constant-time comparison, name cleaning, the `moderate`
  hook, text rendered as text.
- **Casting** only pages of the local origin.
- **Bundle**: paths normalized, nothing outside `root`, zip bombs bounded
  (size, entries, ratio checked as it inflates), CRCs checked.
- **The window's browser** has its own temporary profile: no cookies or
  extensions of the user's browser, nothing of the game kept.
- **What is public**: every file of the bundle (phones need them; the
  host page is code, not a secret) and the relay socket. A stranger with
  the tunnel's hostname can load pages but not join without the join key.

## 13. Tests

- Rust (`cargo test`): manifest parsing and errors; zip reading
  (stored, deflate fixtures from Python's `zipfile`, a top-level folder,
  traversal names, CRC mismatch, a bomb); inflate known answers (fixed,
  dynamic, stored blocks; zlib's test vectors); the listeners' Host,
  Origin, token and cookie rules; static serving; the relay with fake
  clients and a fake host over real sockets (open, data both ways,
  close codes, busy/drain, limits, host replaced, 1012, 4011 at the end);
  the API with a fake cast/discovery backend; the CLI's arguments.
  Opt-in (`TVPIPE_TEST_CHROME=1`): a real headless cast of a bundle's TV
  page to the fake Cast receiver, frames counted.
- `npm/party` (`node --test`): the host with `memoryRelay()` and the
  client (joins, resumes, replays, diffs, collect, late join: the old
  Node tests moved over), the API client against a fake, the session
  vectors replayed by the browser host; and, when a tvpipe binary is
  built, an end-to-end test: Node as the host page (`?t=`), fake phones
  over the public listener.
- drawbluff: the rule vectors and game tests unchanged in substance,
  run with `memoryRelay()` and fake phones; a live script
  (`test/live/playshare.ts`): `tvpipe playshare --window none`, the
  launcher and the host page in headless Chrome over DevTools, the
  `browser` receiver, bots as phones, screenshots of the launcher,
  lobby and a round; the same from the zip.

## 14. What goes

- `crates/tvpipe-party` (host, WebRTC fast path, examples): the relay
  needs only tvpipe's `httpd`/`websocket`, now used inside the crate;
  `tvpipe::__web` goes with it.
- `crates/tvpipe-canvas` and the Rust drawbluff (`games/drawbluff/src`,
  `tests`, `Cargo.toml`): no Rust game logic or TV rendering.
- The Node host of `@tvpipe/party` (`server.ts`, `ws.ts`, `create.ts`,
  `toTv`, the video screen) and drawbluff's `main.ts`,
  `screen-video.ts` (`@napi-rs/canvas`).
- `docs/multiscreen-design.md` (replaced by this document); the parts of
  `docs/party-design.md` about the Node host are marked historical.
- Kept: `@tvpipe/sdk`, the `tvpipe` library and CLI (`tunnel PORT`, `qr`
  stay), the party protocol and the phone/TV pages.

## 15. Plan

1. This document.
2. Rust: bundle + zip + inflate, the listeners, the API, the relay,
   casting through `share`, the window, the CLI; tests.
3. `@tvpipe/party` in the browser: host, client, API client, test
   relay, the standard launcher; the build into the crate.
4. drawbluff as a bundle (host page with `game/*.ts`, TV and phone pages
   as they are), tests, the live check, a zip.
5. Deletions, README, ARCHITECTURE, AGENTS, the manual's text, CI.

## 16. As built

- **Rust** (`crates/tvpipe/src/playshare/`, feature `playshare`, the CLI
  through `__cli::playshare`): `bundle` (manifest, folder, zip), `server`
  (the listeners), `hub` (API, relay, progress), `conn` (a WebSocket with
  an outbox: `busy`/`drain`, the backlog limit, pings), `backend`
  (discovery, pairing, cast: fakes in tests), `window`, `web` (the
  embedded library and launcher; `TVPIPE_PLAYSHARE_WEB=DIR` serves them
  from a directory). The API calls a game's code makes are as in §5.
- **The window**: Chromium-based browsers get the app window. Firefox has
  no app windows: with only Firefox installed, `--window app` opens the
  system's browser (`open`), and the end follows the pages' sockets.
  `--window headless` exists for tests.
- **The library** is TypeScript compiled by `tsc` (erasable syntax, ES
  modules with `.js` imports); `npm run build` writes `dist/` and copies
  the browser files into the crate (`scripts/web.ts`; `--check` in
  `npm run check`). The session vectors moved to `npm/party/test/vectors/`.
- **drawbluff**: `tvpipe.json` at the game's folder with `root: dist`;
  `npm run build` compiles `game/` and `host/` and rewrites the library
  imports to `/_tvpipe/lib/`; `npm run zip` writes `drawbluff.zip`
  (`scripts/zip.ts`, deflate through Node's zlib). The host page shows
  the room, the TVs (with "try again"), the players (remove, bots), the
  rounds, timers and late joining, and a muted preview of the TV (the
  watch link in an `<iframe>`).
- **Checked** (2026-11-19, macOS 26, Chrome 157): `node
  test/live/playshare.ts` (and `--zip`, and `--tunnel=opentunnel`): the
  launcher and the host page in headless Chrome, a phone in headless
  Chrome (joining through the phones' listener, drawing with touch,
  lying, voting), bots, the TV through tvpipe's own headless Chrome and
  the `browser` receiver, a whole game, the End button: tvpipe exits 0,
  every phone says the room closed. The opt-in Rust test with a real
  Chrome casts the TV page to the fake Cast receiver and notices the
  window closing.
- **Not yet**: a real TV (Apple TV, Chromecast) and a real phone; LAN
  mode with a phone; a game's own launcher (only the contract and the
  standard one); the host page keeping its state across a reload
  (`sessionStorage`), so a reload sends phones back to their name entry.
