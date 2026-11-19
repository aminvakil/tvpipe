# Public links (`--tunnel`)

`--tunnel` puts a browser page of tvpipe's on a public https URL, for
people elsewhere:

- **the player** (`play`/`preview`/`test-pattern`/`screen --to browser
  --tunnel`): they watch the stream;
- **the share page** (`share --tunnel`): someone remote shares a tab,
  window or screen to the local receivers, after the local user says yes;
- **any server on 127.0.0.1** (`tvpipe tunnel PORT`): a program's own web
  app; see [below](#any-port-tvpipe-tunnel-port). `tvpipe playshare`
  starts its tunnel the same way for the phones' listener
  ([`playshare-design.md`](playshare-design.md) §11: OpenTunnel first).

Two providers are built in, natively (no new crate, no external program),
behind one small interface (`net::tunnel::Tunnel`); everything else here
is shared:

| | `--tunnel` = `--tunnel=cloudflare` | `--tunnel=opentunnel` |
|---|---|---|
| Service | [Cloudflare Quick Tunnel](https://try.cloudflare.com) (`*.trycloudflare.com`) | [OpenTunnel](https://github.com/anomalyco/opentunnel) (`*.opentunnel.xyz`) |
| Account, state | none; a new tunnel and hostname every run | none; the machine's tunnel and certificate, made once (config directory, `opentunnel/`) |
| Who can read the traffic | Cloudflare (it ends TLS) | nobody but tvpipe and the viewer (TLS ends in tvpipe) |
| Protocol | cloudflared's, undocumented (HTTP/2 + Cap'n Proto RPC), ported from its source | documented, with test vectors |
| Operator | Cloudflare; no uptime promise for quick tunnels | one small operator (Anomaly), on Fly.io; self-hostable |
| Code | `crates/tvpipe/src/net/cftunnel/` | `crates/tvpipe/src/net/opentunnel.rs`, `crates/tvpipe/src/net/roots/` |

The value is attached (`--tunnel=opentunnel`): a separate word would be
the command's input. Cloudflare is the default because it needs no state
and its path is short (an edge in most cities); OpenTunnel is the private
one.

## What the user sees

```
$ tvpipe preview --tunnel
browser: open http://127.0.0.1:8765/?t=… to watch
tunnel: connected to Cloudflare (vie02)
browser: anyone with https://words-words-words.trycloudflare.com/?t=…&delay=300 can watch (Cloudflare quick tunnel; the link is new every run)

$ tvpipe share --to "Living Room" --tunnel=opentunnel
share: someone with https://…opentunnel.xyz/?t=… can ask to share their screen to the receivers (the link is new every run and lets people in for 60 min; you confirm each share here)
share: 203.0.113.9 (Chrome 157) wants to share window 1440x900 "Slides"
share: show it on the receivers? [y/N] y
share: remote share: latency 300 ms
```

When stderr is a terminal, the public link is followed by its QR code
(`crates/tvpipe/src/util/qr.rs`: byte mode, level M or L, half blocks, black on white
from the 256-color palette), so a phone can open it from the screen;
`--no-qr` or `TVPIPE_NO_QR=1` turns it off, `--machine` never draws it.
The local `http://127.0.0.1` links get none: another device cannot open
them.

With `--machine` a `tunnel` event carries the link (`docs/machine.md`);
the Node SDK has `tunnel` options and `session.publicUrl`. In machine mode
there is no terminal to ask on: remote shares need `--yes`.

## Access rules (`crates/tvpipe/src/browser/access.rs`, provider-neutral)

The page servers still bind `127.0.0.1` only; the tunnel client connects
to them from loopback like everything else, so the decision is made on
`Host` and the token, never on the peer address.

- **Host:** the loopback names as before, plus exactly the tunnel's
  hostname (`:443` tolerated). Anything else: 421.
- **A token per way:** the public host has its own token, new every run.
  A request naming the public host needs that token, a loopback request
  the local one. The player's lasting local token never leaves the
  machine; a link that went over the internet dies with the run.
- **Origin:** WebSockets over the public host need `Origin:
  https://HOST` (browsers always send it), the share page's included.
- **Expiry:** `--tunnel-ttl MIN` (share: 60 by default, player: none):
  afterwards public requests get `410 Gone`; open connections go on.
- **Viewer cap:** at most 4 player tabs over the tunnel (503 beyond).
- **Remote shares need a yes:** a `capture` from a page over the tunnel
  is described on the terminal (the visitor's address from the relay,
  the browser, surface, size, label, sound; control characters removed,
  as in every log line) and waits for `y` + Enter; the page shows
  "Waiting for the host to accept…". No terminal (stdin is not a tty):
  remote shares are refused unless `--yes`. A share turned down gets
  `end` and is closed; the next capture may come.
- **Latency floor:** a remote share plays out at least 300 ms after
  capture (`--latency` lower is raised), the public player URL asks for
  `delay=max(latency, 300)` (measurements below).
- **Logging:** public requests without the right token are logged
  (scanners show up within seconds of a new hostname).
- **Per-viewer flow control** (`browser::player`, docs/player-protocol.md):
  pages acknowledge the bytes they got (`ack` every 32 KiB); a tab may
  have 256 KiB unacknowledged, beyond that its frames are dropped and it
  resumes at the next keyframe. Behind a tunnel a slow viewer's socket
  never stalls (the relay buffers), and all viewers share the relay link,
  whose buffers would otherwise fill up for everyone.

## Any port (`tvpipe tunnel PORT`)

```
$ tvpipe tunnel 8080
tunnel: connected to Cloudflare (vie02)
tunnel: https://words-words-words.trycloudflare.com/ leads to http://127.0.0.1:8080 (Cloudflare quick tunnel; the link is new every run)
$ tvpipe --machine tunnel 8080 --tunnel=opentunnel
{"type":"tunnel","page":"port","provider":"opentunnel","url":"https://….opentunnel.xyz/"}
```

The same providers, for a server tvpipe does not own: the link carries
requests to `127.0.0.1:PORT` until Ctrl-C, `q`, or (`--machine`) the
`stop` command or stdin closing. None of the access rules above apply:
**the server decides**. Requests arrive with the public name as `Host`
(the server should accept only that and its loopback names: DNS
rebinding), WebSockets with `Origin: https://HOST` (check it: cross-site
hijacking), and the link has no token, so anything not meant for
everyone needs a key of the server's. Through Cloudflare the visitor's
address is in `cf-connecting-ip`; through OpenTunnel the server sees only
loopback. The QR code is drawn on a terminal as for the pages. From Node:
`tunnel({ port, provider })` in `@tvpipe/sdk`.

`tvpipe qr [--svg | --rows] TEXT` prints the same QR codes for other
uses: terminal art, an SVG document, or one line of `0`/`1` per module row
for programs (the SDK's `qr()`, a party game's TV screen).

## Comparison

Measured the same way for both: 2026-11-18/19 from Vienna (residential
fibre), macOS 26 on Apple Silicon, release builds, Chrome 157 headless as
the viewer on the same Mac (every number crosses the internet twice).
Details per provider below.

| | Cloudflare | OpenTunnel |
|---|---|---|
| Client code (without tests, comments, blank lines) | 1713 lines (2067 with comments; tests 793) | 1221 lines (1457 with comments; tests 724) |
| Binary, stripped (`strip -x`), over the shared parts | +223 KB, measured alone with four embedded public roots (rustls' client-side chain verification is new to the binary); it now uses the Mozilla list it shares with OpenTunnel | +425 KB (rustls server side, webpki, Mozilla roots 33 KB) |
| Setup to the first successful GET | 6.7 s median (6.7-8.9; trycloudflare.com takes 4.3-5.5 s to answer) | 0.34 s median warm; 15.8 s on a machine's first use |
| WebSocket round trip, median / p95 | 12 ms / 72-82 ms (cloudflared itself, http2 or QUIC: the same) | 43-45 ms / 107-113 ms |
| Player 1080p30 ~6-7 Mbit/s, `delay=300` | nothing late | nothing late |
| Same, `delay=100` | 0-6 late frames, up to 2.7 % late audio | one 0.5 s hiccup (5 frames) |
| Edge/bridge connection cut | back after 1.0 s, same URL | back after 0.39 s (median of 5), same URL |
| Rate limit (one address) | 429 after ~29 tunnels in 90 min | 30 tunnel creations per hour (only the first run creates one) |

Shared parts (access rules, share confirmation, player flow control,
CLI): +50 KB on the main of the time (ca0bac1), +17 KB on today's, which
already had some of what they use. Both providers together: +600 KB over
the shared parts (they share rustls' client-side verification and the
Mozilla roots), +617 KB over main without `--tunnel` (5,119,528 B
stripped).

## Cloudflare Quick Tunnels (`crates/tvpipe/src/net/cftunnel/`)

### How it works

Ported from cloudflared 2026.11.1 (Apache-2.0,
`github.com/cloudflare/cloudflared`); there is no specification.

1. **Credentials.** `POST https://api.trycloudflare.com/tunnel` (empty
   body) answers `{success, result: {id, hostname, account_tag, secret}}`:
   an ordinary tunnel's credentials plus its hostname
   (`cmd/cloudflared/tunnel/quick_tunnel.go`). The service's certificate is
   checked against Mozilla's root list (`crates/tvpipe/src/net/roots/`, shared with
   OpenTunnel).
2. **Edge.** TLS to `region1.v2.argotunnel.com:7844` (`region2` on every
   other attempt; the SRV record `_v2-origintunneld._tcp.argotunnel.com`
   lists these two), SNI `h2.cftunnel.com`, no ALPN. The edge's
   certificate chains to Cloudflare's *origin* CA, not a public root;
   `edge_ca.pem` holds the one that signs it today (CloudFlare Origin SSL
   ECC CA, valid until 2029-08-15; cloudflared embeds it with two more,
   `tlsconfig/cloudflare_ca.go`).
3. **HTTP/2, roles swapped** (`connection/http2.go`). tvpipe dialed but is
   the HTTP/2 *server*: it sends SETTINGS, the edge sends the client
   preface and opens streams. `h2.rs` is that server: frames, flow control
   both ways (1 MiB stream and 16 MiB connection windows advertised,
   credit returned as streams consume), PING, GOAWAY, CONTINUATION; HPACK
   (`hpack.rs`) decodes fully (Huffman, dynamic table) and encodes only
   literals.
4. **Registration** (`connection/control.go`,
   `tunnelrpc/registration_client.go`). The edge's first stream carries
   `Cf-Cloudflared-Proxy-Connection-Upgrade: control-stream`; we answer
   200 and speak Cap'n Proto RPC on its body as the client: `Bootstrap`
   (question 0), then `Call` question 1 pipelined on its answer:
   `RegistrationServer.registerConnection(auth {accountTag, tunnelSecret},
   tunnelId, connIndex 0, options {client {clientId, features [], version
   "tvpipe/X", arch}, numPreviousAttempts})`. The `Return` carries
   `ConnectionDetails {uuid, locationName}` (e.g. `vie02`) or
   `ConnectionError {cause, shouldRetry}`. On shutdown,
   `unregisterConnection` (question 2). `capnp.rs` builds single-segment
   messages and reads any (far pointers included); field offsets from
   `tunnelrpc.capnp` and zombiezen's `rpc.capnp.go`.
5. **Visitors.** Every other stream is a request: `:method`, `:path`,
   `:authority` and the visitor's headers (`cf-connecting-ip` among them);
   `Cf-Cloudflared-Proxy-Connection-Upgrade: websocket` marks a WebSocket.
   tvpipe sends it to `127.0.0.1:PORT` as HTTP/1.1 (`Host` from
   `:authority`, so the page's Host/token/Origin checks apply unchanged)
   and answers with `:status` (101 becomes 200: HTTP/2 has none),
   `content-length`, and the origin's headers serialized into
   `cf-cloudflared-response-headers` (`base64(name):base64(value)` joined
   by `;`, unpadded) plus `cf-cloudflared-response-meta: {"src":"origin"}`
   (`connection/header.go`). A WebSocket's bytes then flow as DATA both
   ways. Configuration pushes and TCP (WARP) streams get 502.
6. **DNS.** The hostname reaches Cloudflare's name servers 0.5-10 s after
   registration, and a resolver that asks earlier caches the NXDOMAIN for
   minutes (1.1.1.1 did in tests). So `start` returns, and the URL is
   printed, only once `kevin.ns.cloudflare.com` answers for it (plain DNS
   over UDP, no recursion, a new source port per query: those servers also
   remember a negative answer for ~5 s, per port).
7. **Reconnecting.** If the connection drops (or the edge goes silent: a
   PING after 15 s idle, given up after 30 s) tvpipe dials again with the
   same credentials after 0.2 s, 0.4 s, ... up to 25.6 s, alternating
   regions. The hostname stays: it belongs to the tunnel, not the
   connection.

The visitor's address for the share confirmation comes from
`cf-connecting-ip`, by the local address of the connection to the page.

### Measurements

2026-11-18/19 from Vienna (residential fibre, macOS 26, Chrome 157
headless), tvpipe and the viewer on the same machine: every number crosses
the internet twice. Edge locations vie02/vie07.

| | |
|---|---|
| Code | `crates/tvpipe/src/net/cftunnel/`: 2870 lines of Rust, 2067 without tests, 1713 without tests, comments and blank lines (HTTP/2 461, HPACK 226, Cap'n Proto 334, the rest 692); the edge CA (20 lines of PEM) |
| Binary | release over the provider-neutral commit, alone: +223 KB with `strip -x`, +218 KB with `CARGO_PROFILE_RELEASE_STRIP=symbols` (`__text` +149 KB); rustls' certificate-chain verification (webpki) is a good part of it, new to tvpipe then |
| Setup | process start to first successful HTTPS GET through the link: 6.66, 6.69, 6.71, 7.19, 8.88 s, **median 6.7 s**. Of that, `api.trycloudflare.com` takes 4.3-5.5 s to answer; connecting and registering 0.35-0.45 s; DNS 0.5-3 s |
| WebSocket | 100 pings (the player's JSON ping/pong): median 12 ms, p95 72-82 ms, 11-15 % above 30 ms. cloudflared 2026.11.1 over the same path, `--protocol http2` and `quic`, measured the same (median 11-13 ms, p95 72-80 ms): the spikes are Cloudflare's |
| Player, 1080p30 ~7 Mbit/s, 60 s | `delay=300`: no late or dropped frames, no late audio (two runs). `delay=100`: one run clean, one with 6 frames dropped late and 2.7 % of audio records late (max delay 117 ms) |
| Share page | fake 800×450 capture with audio from headless Chrome through the link (share --yes): 20 fps (the fake source's rate), no drops, capture-to-tvpipe jitter peaks ~65 ms above the average |
| Reconnect | edge connection cut (in-process, the live test): the same URL answered again 1.0 s later (1.9 s with the earlier 0.5 s first backoff). Open WebSockets break with the connection; the player page reconnects by itself (0.3 s, 0.6 s, ...) |

The public player URL asks for `delay=300`, which these numbers support.

Rate limit: `api.trycloudflare.com` answered `429 Too Many Requests`
after about 29 tunnels in 90 minutes from one address (tvpipe reports
it as is). A quick tunnel outlives its connector, though: its
credentials registered again after 120 s and the hostname served at once
(`live_quick_tunnel_reuse`), so `tvpipe playshare` keeps the last ones
and reuses them (`Tunnel::start_reusing`, docs/playshare-design.md §11).

### Stability and terms

- **Protocol.** What tvpipe implements is cloudflared's HTTP/2 transport
  as of 2026.11.1. It has existed since 2020-09 (TUN-3400, the Go HTTP/2
  library replacing h2mux) and is still the fallback when UDP (QUIC) is
  blocked, so the edge keeps it. The registration RPC dates from named
  tunnels (2020); the schema keeps every deprecated method and struct for
  wire compatibility, and the feature list it sends is optional
  (`serialized_headers` and friends are "universally supported" and no
  longer advertised, TUN-10904). The edge accepted `version
  "tvpipe/0.1.1"` and no features.
- **Support policy.** cloudflared's README: Cloudflare supports cloudflared
  versions within one year of the latest release; breaking changes
  "unrelated to feature availability may be introduced that will impact
  versions released more than one year ago". There is no promise for
  third-party clients, and no announcement channel other than cloudflared's
  releases. In practice: re-check this client against a new cloudflared
  at least yearly (`connection/http2.go`, `connection/header.go`,
  `tunnelrpc/proto/tunnelrpc.capnp`, `quick_tunnel.go`), and expect to
  change `edge_ca.pem` before 2029-08 (the edge's CA expires then).
- **Terms.** Quick tunnels have no uptime guarantee and fall under the
  Cloudflare Online Services Terms; the CDN terms say free plans should not
  serve "video and other large files" without the paid products. A live
  stream to a few viewers is in that grey zone. Cloudflare ends TLS, so it
  sees the page, the token and the stream.

### Limits

- One edge connection (like cloudflared for quick tunnels); while it is
  down, visitors get Cloudflare's error page.
- Quick tunnels: 200 requests in flight, no server-sent events, a new
  hostname every run.
- The link is printed only when it resolves; someone who opens it within
  a few seconds may still meet a Cloudflare name server that remembers the
  NXDOMAIN from before (tvpipe's own probes create such entries for ~5 s).

## OpenTunnel

### The service

OpenTunnel (MIT, by Anomaly; first release 2026-03; hosted on Fly.io,
self-hostable) is a *blind TLS* relay:

- Each machine has a tunnel `ID.opentunnel.xyz` and a **wildcard
  certificate whose private key never leaves the machine**: the client
  sends a CSR, the service gets the certificate (ZeroSSL, DNS-01) and
  renews it for the same key.
- The relay reads only the SNI of a visitor's TLS ClientHello and
  forwards the still-encrypted connection over the client's bridge
  WebSocket; the client terminates TLS. The relay sees neither the page,
  its token, nor the stream.
- The protocol is documented with test vectors (`docs/protocol.md` in its
  repository): a JSON HTTP API (`POST /api/tunnel`, `POST/GET
  /api/tunnel/:id/certificate`) and one WebSocket per client
  (`wss://opentunnel.xyz/api/tunnel/:id/connect`, subprotocol
  `opentunnel`) with JSON control messages (`attach`, `attached`, `open`,
  `end`, `reset`, `ping`/`pong`, `drain`) and binary frames
  `u32be conn | ≤32 KiB`. Additive changes keep the subprotocol name.
- Limits of the hosted service: 30 tunnel creations per hour per address,
  5 new certificates per tunnel per day, 256 connections per bridge,
  512 KiB buffered per connection and 4 MiB per bridge at the relay.
  No uptime promise; one operator.

### The client (`crates/tvpipe/src/net/opentunnel.rs`)

- **Identity** in the config directory, `opentunnel/` (the OpenTunnel
  CLI's layout, so it can share it; files 0600): `tunnel.json` (id,
  hostname, certificate expiry), `token`, `private-key.pem` (P-256,
  PKCS#8, made with ring), `certificate.pem`, `chain.pem`. The PKCS#10
  request is hand-rolled DER (known-answer test; `openssl req -verify`
  accepts it). An interrupted provision resumes with the saved key. The
  service's renewed certificate is fetched on a reconnect at most every
  12 hours and used for new visitors.
- **Route:** a random 16-hex label per run, `ROUTE.ID.opentunnel.xyz`.
  Route names are not in certificate logs (the certificate is a
  wildcard), so the URL cannot be guessed; it stays the same through
  reconnects.
- **Visitors:** each `open` gets a rustls server session with our
  certificate (HTTP/1.1 ALPN) and a TCP connection to `127.0.0.1:PORT`;
  the plaintext is copied both ways. The relay's `peer` is remembered by
  the local connection's address, which is how the share page knows who
  asks (`Tunnel::peer_lookup`).
- **Reconnect:** a dropped, silent (`idle_timeout_ms`) or draining bridge
  is attached again with the same route: backoff 250 ms doubling to 30 s
  with jitter, reset by a successful attach; `drain` (a deploy) at once.
  A refused token is fatal and logged.
- **Back-pressure:** the bridge reader never waits on the bridge: TLS
  answers and control messages are queued. One writer thread sends
  control messages first, then data round-robin over the connections,
  each with its own 256 KiB queue; a full queue stops reading that
  connection's local socket only. Writes to the local server time out
  after 2 s (the visitor is reset). The wire has no per-connection flow
  control, and the bridge is one TCP connection whose socket buffers hold
  everyone's data alike: hence the player's per-viewer limit above.
- **Trust:** the API and bridge (`opentunnel.xyz`, behind Cloudflare,
  whose edge certificates come from several public CAs) are checked
  against Mozilla's root list, the same source as the webpki-roots crate:
  `crates/tvpipe/src/net/roots/mozilla.pem` from the CCADB (CDLA-Permissive-2.0,
  `LICENSES/`), compiled in as trust anchors only (`roots/mozilla.rs`,
  subject + key + name constraints: 33 KB in the binary instead of
  165 KB), refreshed by `scripts/update-roots.sh` (a test fails if the
  two disagree). `$SSL_CERT_FILE` adds roots (a company proxy).
  Pinning was rejected: Cloudflare picks among Let's Encrypt, Google
  Trust Services, SSL.com and others per certificate, and a self-hosted
  service has its own. The token is what the TLS protects most: with it,
  someone could bind a certificate of their own to the tunnel.
- **Self-hosted service:** `TVPIPE_OPENTUNNEL_API=https://HOST[:PORT]`
  (tunnels are then `ID.HOST`); `TVPIPE_OPENTUNNEL_DIR` moves the
  identity.
- **Tests** (`crates/tvpipe/src/net/opentunnel/tests.rs`): a fake relay on loopback
  (the API, the bridge, visitors with real TLS against a test CA):
  provisioning, visitors end to end, the visitor's address, reconnects
  after a dropped bridge, `drain` and a silent relay under the same
  route, a refused token, a slow relay with a big download, the writer's
  order and limits; plus DER/CSR known answers and the roots check.
  Debug builds: `TVPIPE_TUNNEL_DROP_AFTER=SECS` cuts the first bridge,
  to watch a reconnect against the real service.

### Measurements

2026-11-18, from Vienna (residential fibre; the relay's anycast address
answers ICMP in 24-83 ms, avg 32 ms), macOS 26 on Apple Silicon, release
build, viewer on the same Mac (so every number crosses the internet
twice), Chrome 157 headless.

| | |
|---|---|
| Provider client | `crates/tvpipe/src/net/opentunnel.rs` 1457 lines (1221 without blank and comment lines), its tests 724, `net/tunnel.rs` 54; the roots: `roots/mozilla.rs` (generated, 150 KB of byte strings) and `mozilla.pem` (its source, not compiled in) |
| Binary (stripped, `strip -x`, vs main 4,266,904 B) | provider-neutral parts +49 KB; OpenTunnel +425 KB (rustls's server side ~105 KB and webpki path validation ~31 KB are new to the binary, the roots ~33 KB, the client and its generics the rest); together +474 KB |
| Setup, cold (new tunnel and certificate) | 15.5 s to the URL, 15.8 s to the first successful GET |
| Setup, warm (n=5) | URL after 0.12-0.23 s (median 0.18), first GET 0.27-0.46 s (median 0.34) |
| WebSocket round trip (100 pings, stream flowing) | median 43-45 ms, p95 107-113 ms |
| HTTP round trip (100 GETs, one connection) | median 44-45 ms, p95 107-112 ms |
| Reconnect after the bridge is cut (n=5) | 344-456 ms (median 390), same URL; a watching tab was back after 0.56 s |

Player over the tunnel:

| Stream | delay 100 ms | delay 300 ms |
|---|---|---|
| test pattern, 1080p30 built-in encoder (~2.4 Mbit/s) | 29.9 fps, frames drawn 104-107 ms after capture, 8 late frames, 13 % of audio records late | 30 fps, nothing late |
| ffmpeg `testsrc2` + noise, 1080p30 at 6000 kbit/s (5.0-5.5 Mbit/s actual) | 28 fps, one hiccup of 0.5 s (the in-flight limit dropped 5 frames, the tab skipped to the next keyframe) | 30.1 fps, nothing late or dropped |

Share over the tunnel (a canvas stream at 720p30 from Chrome): 29-30 fps
from the page, capture to tvpipe 11 ms average with peaks of 75-87 ms;
with the 300 ms floor nothing was late.

Hence the 300 ms floor: 100 ms is not enough over the internet for audio,
300 ms was clean in every run.

## Other options (for the record)

- **SSH reverse tunnels** (localhost.run and similar through an installed
  `ssh`): speed-limited, rotating names, the service sees everything.
- **Tailscale Funnel** (installed `tailscale`): needs an account and a
  tailnet policy, ports 443/8443/10000, unpublished bandwidth limits.
- **cloudflared as an external tool:** works (prototyped), but an extra
  program; replaced by the native client.

## Sources

- OpenTunnel: `github.com/anomalyco/opentunnel` (`docs/protocol.md`,
  `docs/server.md`, `docs/trust-boundary.md`, `spec/vectors/`).
- CCADB: Mozilla included roots report (websites trust bit), CDLA
  Permissive 2.0.
- RFC 2986 (PKCS#10), RFC 5280, RFC 5480, RFC 5758.
- cloudflared 2026.11.1 (Apache-2.0): `cmd/cloudflared/tunnel/quick_tunnel.go`,
  `connection/{http2,header,control,protocol}.go`, `client/config.go`,
  `features/features.go`, `proxy/proxy.go`,
  `tunnelrpc/{registration_client.go,pogs/registration_server.go}`,
  `tunnelrpc/proto/tunnelrpc.capnp`, `edgediscovery/allregions/discovery.go`,
  `tlsconfig/cloudflare_ca.go`, `README.md` (support policy),
  `RELEASE_NOTES`.
- zombiezen.com/go/capnproto2 v2.18.0 `std/capnp/rpc/rpc.capnp.go` (field
  offsets); capnproto.org `encoding.html`, `rpc.capnp`.
- RFC 9113 (HTTP/2), RFC 7541 (HPACK; the tests use Appendix C), RFC 1035.
