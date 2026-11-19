# The party protocol (version 1)

What a party host (`@tvpipe/party` in Node, `tvpipe-party` in Rust) and
its clients (phones, screens, watchers; the browser client
`party-client.js`) say to each other. The Node host is the reference:
the session vectors in `crates/tvpipe-party/tests/vectors/` are recorded
from it (`node test/vectors.ts --bless` in `npm/party`) and replayed by
both hosts. The design behind it: `docs/party-design.md` §5 (WebSocket)
and `docs/multiscreen-design.md` §2.4–2.5 (the WebRTC fast path).

## 1. Transport

- One WebSocket per client at `/party/ws` on the party's origin (a
  tunnel's `https://`, a LAN `http://IP:PORT`, or loopback). Text
  messages, each one JSON object with a `t` field. Binary messages are
  reserved: a host closes the socket on one (`bye protocol`).
- **Unknown `t` values and unknown fields are ignored** by both sides.
  Every addition below relies on this: an older peer does not see it.
- Optional, Rust hosts with the `webrtc` feature: two WebRTC data
  channels per player, signaled over the WebSocket (§9). The WebSocket
  stays open and remains the source of truth.

## 2. Roles and links

| Role | Joins with | Sees (`view`) | May send |
|---|---|---|---|
| `player` | the join key and a name, or a resume token | `roster`, `room` (shared by players), `me` (own) | actions, input |
| `screen` | the screen key | `roster`, `screen`, `join` (`code`, `url`, `qr`, `mode`) | actions |
| `watch` | the watch key | like `screen` | nothing (actions are refused `read-only`) |

Links: join `ORIGIN/?r=CODE#k=JOINKEY` (in the QR code), screen
`http://127.0.0.1:PORT/screen/#r=CODE&k=SCREENKEY&role=screen`, watch
`ORIGIN/screen/#r=CODE&k=WATCHKEY&role=watch`. Keys sit in the fragment
so they never reach request lines or logs. Keys and resume tokens are
128 random bits, base64url, compared in constant time.

## 3. Messages

### Client → host

| `t` | Fields | |
|---|---|---|
| `hello` | `v` (1), `room`, `role`, `key`, `name` (a new player), `resume` (a token from an earlier `welcome`), `agent`, `caps` | first message, within 5 s |
| `act` | `id` (integer 1…2⁵³−1, increasing per player), `type`, `data` | answered by `ack` |
| `in` | `s` (integer ≥ 1), `data` | continuous input, never answered (§8) |
| `ping` | `c` (the client's clock, ms) | answered by `pong` |
| `resync` | | asks for the full view (after a gap in `rev`) |
| `leave` | | the player leaves for good |
| `rtc` | `sdp` (an offer) or `state` (`failed`, `closed`) | the fast path (§9) |

`caps` is a list of strings; `"rtc"` says the client can upgrade to
WebRTC (players only). Hosts ignore capabilities they do not know.

### Host → client

| `t` | Fields | |
|---|---|---|
| `welcome` | `v`, `room` (`code`), `you` (`id`, `role`; players: `name`, `vip`, `resume`, `lastAct`), `now` (the host's clock, ms since the epoch), `view` (the full view), `rev` (0), `rtc` (true: the host offers the fast path) | after a good `hello` |
| `state` | `rev`, and `set` / `del` (a diff) or `full` (the whole view) | §5 |
| `ack` | `id`, `ok`; `result` (if any) or `error` (`code`, `message`); `dup` (a replay of an id the host has no result for any more) | §6 |
| `msg` | `type`, `data` | transient: a sound cue, a toast; lost when away |
| `pong` | `c` (echoed), `now` | |
| `bye` | `reason`, optional `message` | then the host closes the socket |
| `rtc` | `sdp` (the answer) or `state` (`failed`, `closed`) | the fast path (§9) |

## 4. Closing and reconnecting

`bye` reasons and close codes:

| Reason | Code | Reconnect? |
|---|---|---|
| `protocol` | 4002 | no |
| `bad-key` | 4003 | no (the identity is forgotten) |
| `no-room` | 4004 | no |
| `no-player` (a resume token nobody has, and no name) | 4005 | no (forgotten) |
| `name-taken`, `name-invalid` | 4006, 4007 | no |
| `rate-limited` | 4008 | no |
| `replaced` (a newer socket with the same token) | 4009 | no |
| `kicked` | 4010 | no (forgotten) |
| `room-closed` | 4011 | no |
| `slow` | 4013 | yes |
| `full`, `locked` | 4029 | no |
| (`left`, after `leave`) | 1000 | no |

On anything else (network loss, 1001, 1006, 1012) the client reconnects
with backoff (0.25 s doubling to 5 s, ±25 % jitter), at once when the
page becomes visible, and resumes: `hello` with `resume` gets the same
player, the full view and `lastAct`.

## 5. Views and diffs

The host never tells a client what happened; it keeps what the client
shows. A player's view is `{roster, room, me}`, a screen's `{roster,
screen, join}`; `roster` is `[{id, name, vip, connected}]` in join order.

- After changes the host sends each client one `state` with the
  difference against what that client last got: `set` is merged into the
  view (objects merge recursively; anything else, `null` and arrays
  included, replaces), then every path in `del` (a list of key lists) is
  removed.
- `rev` increases by one per `state` on a connection (`welcome` is 0). A
  client that sees a gap sends `resync` and gets `{t: "state", rev,
  full}`. A `state` whose `rev` is not above the client's is dropped (it
  can arrive late over the other path, §9).
- A client that does not read (more than 64 KiB queued) gets no diffs
  until it drains, then one diff against what it has (latest wins);
  past 256 KiB of anything else it gets `bye slow`.

## 6. Actions: exactly once

- `act` ids increase per player identity; `welcome.you.lastAct` tells a
  reloaded page where to continue. Screens' ids are per connection.
- The client keeps unacknowledged actions (at most 32) and sends them
  again after a reconnect (or when the fast path goes down). The host
  runs each id once: an id at or below the last one it ran is a replay,
  answered from a cache of the last 32 acks (`dup: true` when it is no
  longer there). Hence: **ids must arrive in order** — a client must not
  send an action on one path while an earlier one may still be on its
  way over another (§9).
- Within one host turn, `state` goes out before the acks: an ack's
  effects are in the view when the promise resolves.
- Unanswered actions get `error.code` `unknown-action`; watchers get
  `read-only`.

## 7. Clock

`ping` on connect (five, 300 ms apart) and every 10 s; the client keeps
the offset of the fastest of the last 16 round trips. Deadlines in views
are host times (`now()` on both sides), so a phone and the TV count down
to the same instant.

## 8. Continuous input (`in`)

For a stick, a tilt, a slider: `{t: "in", s, data}`. Never answered,
may be lost, **latest wins**:

- `s` is a sequence number per connection; an `in` whose `s` is not
  above the last one seen on that connection is dropped, as are those
  without an integer `s` ≥ 1, those from screens and watchers, and those
  over 4 KiB.
- Rust: `Event::Input { player, data }`, at most one per player per
  poll (the latest), ahead of the poll's other events, so an action sent
  after an `in` sees it. Node: `player.input` and the `input` event.
- Over the WebSocket `in` counts against the socket's rate limit
  (20/s), so the client sends it there at most every 100 ms (the latest
  data). Over the fast path's `u` channel it has a bucket of its own
  (120/s, 60 at once); what is over is dropped silently.

## 9. The WebRTC fast path

Rust hosts with the `webrtc` feature. The phone is the offerer, the host
an ICE-lite answerer with host candidates on its LAN address (or what
`fast_addrs` says). No STUN, no TURN: a phone that cannot reach the LAN
address (mobile data) stays on the WebSocket.

1. `hello.caps` has `"rtc"`; `welcome.rtc` is `true`.
2. The client creates `RTCPeerConnection({iceServers: []})` with two
   **negotiated** channels: `r` (`negotiated: true, id: 0, ordered:
   true`) and `u` (`negotiated: true, id: 1, ordered: false,
   maxRetransmits: 0`), and sends `{t: "rtc", sdp: OFFER}` over the
   WebSocket (no trickle; the host learns the phone's address from its
   connectivity checks).
3. The host answers `{t: "rtc", sdp: ANSWER}` (`a=ice-lite`, its
   candidates inline), or `{t: "rtc", state: "failed"}` (no capability,
   a bad offer, too many peers).
4. Both channels open: the fast path is up.

While it is up:

- `r` carries what the WebSocket carries, as the same JSON text: the
  client sends `act` (only while no earlier act is unacknowledged on the
  WebSocket), `ping`, `resync`; the host sends `state`, `ack`, `msg`,
  `pong`. `welcome`, `bye` and `rtc` only ever travel on the WebSocket.
  Messages over 60 KiB go over the WebSocket.
- `u` carries only `in`.
- The host's limits are the WebSocket's (`r` shares the socket's rate
  bucket and message sizes); 256 KiB unacknowledged on `r` ends the fast
  path (too slow).

It ends when the WebSocket that signaled it closes (the channels never
outlive it), when a channel closes or ICE fails, when the channels do
not open within 3 s of the offer (the client says `{t: "rtc", state:
"failed"}`), or when the host ends it (`{t: "rtc", state: "closed"}`).
The client then sends its actions that were on `r` again over the
WebSocket, and tries once more after a network change (`online`, the
page becoming visible). Peers that do not know `rtc` never offer or
answer, so a Node host (no `welcome.rtc`) and an old client (no `caps`)
simply stay on the WebSocket.

Security: the SDP, with the DTLS fingerprints, only travels inside an
authenticated WebSocket session, so a channel belongs to the player
whose session negotiated it; ICE credentials are per peer, and the host
answers STUN only for its peers' ufrags. Data channels are DTLS
encrypted even where the WebSocket is plain HTTP (LAN links). Host
candidates show players the host's LAN address.

## 10. Limits

| | |
|---|---|
| `hello` within | 5 s |
| messages per socket | 20/s, 50 at once (`bye rate-limited`) |
| message size | 256 KiB for `act`, 16 KiB for the rest, 4 KiB for `in` (dropped) |
| sockets per server | 2 per player + 16, at least 64 (`socketsFor(maxPlayers)` / `sockets_for`; 64 up to 24 players, 116 at 50); screens and watchers per room 16 |
| failed `hello`s per address | 20 a minute |
| pending actions per client | 32 |
| liveness | WebSocket pings every 15 s, closed after 45 s without a pong |

## 11. Hosts compared

| | Node (`@tvpipe/party`) | Rust (`tvpipe-party`) |
|---|---|---|
| Protocol v1, vectors | reference | replays them |
| `in` | WebSocket | WebSocket, and `u` with `webrtc` |
| `rtc` | never offered | with the `webrtc` feature |
| Links | tunnel, loopback | tunnel, LAN, loopback (`join.mode`) |
