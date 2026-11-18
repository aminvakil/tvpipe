# Public links (`--tunnel`)

`--tunnel` puts a browser page of tvpipe's on a public https URL, for
people elsewhere:

- **the player** (`play`/`preview`/`test-pattern --to browser --tunnel`):
  they watch the stream;
- **the share page** (`share --tunnel`): someone remote shares a tab,
  window or screen to the local receivers, after the local user says yes.

This document is the design and its measurements. The transport is
[OpenTunnel](https://github.com/anomalyco/opentunnel) (opentunnel.xyz),
implemented natively (`src/net/opentunnel.rs`), with no new dependency and
no external program. A native Cloudflare quick-tunnel client is being
built in parallel for comparison; both sit behind the same small
interface (`net::tunnel::Tunnel`), and everything else here is shared.

## What the user sees

```
$ tvpipe preview --tunnel
browser: open http://127.0.0.1:8765/?t=… to watch
browser: anyone with https://4f1c…e2.vhjwpcgu3tch.opentunnel.xyz/?t=…&delay=300 can watch (OpenTunnel; the link is new every run)

$ tvpipe share --to "Living Room" --tunnel
share: someone with https://…opentunnel.xyz/?t=… can ask to share their screen to the receivers (the link is new every run and lets people in for 60 min; you confirm each share here)
share: 203.0.113.9 (Chrome 157) wants to share window 1440x900 "Slides"
share: show it on the receivers? [y/N] y
share: remote share: latency 300 ms
```

The first `--tunnel` on a machine creates its tunnel and certificate (about
15 s, once); later runs have the URL in 0.2 s.

## Access rules (`src/browser/access.rs`, provider-neutral)

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

### The client (`src/net/opentunnel.rs`)

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
  `src/net/roots/mozilla.pem` from the CCADB (CDLA-Permissive-2.0,
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
- **Tests** (`src/net/opentunnel/tests.rs`): a fake relay on loopback
  (the API, the bridge, visitors with real TLS against a test CA):
  provisioning, visitors end to end, the visitor's address, reconnects
  after a dropped bridge, `drain` and a silent relay under the same
  route, a refused token, a slow relay with a big download, the writer's
  order and limits; plus DER/CSR known answers and the roots check.
  Debug builds: `TVPIPE_TUNNEL_DROP_AFTER=SECS` cuts the first bridge,
  to watch a reconnect against the real service.

## Measurements

2026-11-18, from Vienna (residential fibre; the relay's anycast address
answers ICMP in 24-83 ms, avg 32 ms), macOS 26 on Apple Silicon, release
build, viewer on the same Mac (so every number crosses the internet
twice), Chrome 157 headless.

| | |
|---|---|
| Provider client | `src/net/opentunnel.rs` 1457 lines (1221 without blank and comment lines), its tests 724, `net/tunnel.rs` 54; the roots: `roots/mozilla.rs` (generated, 150 KB of byte strings) and `mozilla.pem` (its source, not compiled in) |
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

Earlier prototype numbers (an external `cloudflared` quick tunnel, removed
since): WebSocket round trip 15-16 ms, provisioning 5-6 s per run (a new
hostname every time, rate-limited to a handful per hour), and Cloudflare
terminates TLS.

## Other options (for the record)

- **SSH reverse tunnels** (localhost.run and similar through an installed
  `ssh`): speed-limited, rotating names, the service sees everything.
- **Tailscale Funnel** (installed `tailscale`): needs an account and a
  tailnet policy, ports 443/8443/10000, unpublished bandwidth limits.
- **cloudflared as an external tool:** works (prototyped), but an extra
  program; replaced by the native comparison.

## Sources

- OpenTunnel: `github.com/anomalyco/opentunnel` (`docs/protocol.md`,
  `docs/server.md`, `docs/trust-boundary.md`, `spec/vectors/`).
- CCADB: Mozilla included roots report (websites trust bit), CDLA
  Permissive 2.0.
- RFC 2986 (PKCS#10), RFC 5280, RFC 5480, RFC 5758.
