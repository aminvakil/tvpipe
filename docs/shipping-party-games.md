# Shipping a party game

A party game written in Rust on `tvpipe-party` ships as **one binary**:
its phone and screen pages are embedded, it streams its main screen to
the TV with `tvpipe`, and it serves the phones itself. This page is the
checklist for turning such a game into something people download. The
reference is `games/drawbluff` (`src/main.rs`); the protocol is in
`docs/party-protocol.md`.

## The build

```toml
[dependencies]
tvpipe = { version = "0.2", features = ["signals"] }        # + "headless" for a web screen on the TV
tvpipe-party = { version = "0.2", features = ["webrtc"] }   # webrtc: optional, see below
tvpipe-canvas = "0.2"                                        # a video screen drawn in Rust

[profile.release]
lto = true
codegen-units = 1
strip = true
```

- **Pages** go in with `tvpipe_party::assets!("web/phone", [...])`; a
  test calling `Assets::check_dir()` catches a file missing from the
  list. `TVPIPE_PARTY_DEV=1` serves them from disk while you edit.
- **Licenses.** tvpipe's default features are Apache-2.0 code only. The
  `fairplay` feature (AirPlay receivers that insist on FairPlay) links
  LGPL-3.0 code statically: then the LGPL's terms apply to the binary
  (the notice, the license, relinkable object files or source). Ship the
  notices in `LICENSES/` that your features bring (fonts with
  `player`/`share`, CDLA roots with `tunnel`). `cargo tree -e normal`
  shows what is in.
- **Size.** drawbluff is 5.5 MB stripped (4.8 MB with LTO). The `webrtc`
  feature adds about 2.9 MB and ~140 crates (`rtc` and its tree).
- **Rust.** 1.85 for the crates; 1.89 with `webrtc` (rtc's minimum),
  1.88 with `deser`.

## Ctrl-C and SIGTERM

std cannot catch Ctrl-C: without a handler the process dies at once and
the TV stays in mirroring mode, the phones see a dead socket. tvpipe's
`signals` feature (off by default) installs one:

```rust
let waker = party.waker();
tvpipe::signals::on_signal(move || waker.wake());   // or stop_on_signal(stream.stopper())
while !tvpipe::signals::requested() {
    for event in party.wait_until(next_tick) { /* … */ }
    party.flush();
}
party.close();                              // phones: "the room closed"
let _ = stream.stop();                      // the TV gets its TEARDOWN/STOP
party.shutdown(Duration::from_secs(2));     // wait for the goodbyes
std::process::exit(130);
```

A second Ctrl-C exits at once. On Windows it is the console handler
(Ctrl-C, Ctrl-Break); a GUI game without a console ends sessions in its
own quit path.

## How phones reach the game

| `Join` | Phones | Prompts | Notes |
|---|---|---|---|
| `Auto` (default) | anywhere, through a tunnel (Cloudflare, else OpenTunnel); the LAN link when offline | none for the tunnel (loopback only) | the tunnel provider can read the traffic (Cloudflare) or not (OpenTunnel ends TLS in tvpipe) |
| `Lan` | the same Wi-Fi | the firewall (below) | plain HTTP: no secure context (no camera, motion sensors, wake lock); anyone on the Wi-Fi can read keys |
| `Local` | this computer | none | development |

With `webrtc` the phones on the LAN also get data channels, in every
mode (with a tunnel that is the **hybrid**: the page and the WebSocket
through the tunnel, the channels on the LAN). The game loses nothing
when they do not open; they are faster (acts in under 1 ms on the LAN
against tens of ms through a tunnel) and do not stall on a lost packet.
Continuous input (`party.input()` on the phone, `Event::Input` on the
host) uses them.

Listen once at start, before people scan, so a firewall prompt comes up
while someone is at the computer. Print the ports: TCP 7420 for LAN
pages (`PartyOptions::lan_port`), UDP 7420 for WebRTC
(`PartyOptions::fast_port`), each falling back to the next free one.

## Platform prompts

| | Tunnel, WebSocket only | LAN link or WebRTC |
|---|---|---|
| **Windows** | none (loopback); outbound to TVs as tvpipe | Windows Defender Firewall asks on the first LAN listen ("allow on private networks"), signed or not; declined: LAN phones cannot connect, the fast path falls back to the WebSocket |
| **macOS** | Local Network permission for the TVs: a bundled game's `Info.plist` needs `NSLocalNetworkUsageDescription` and `NSBonjourServices` (`_airplay._tcp`, `_raop._tcp`, `_googlecast._tcp`) | the same, and the application firewall when it is on (asks for unsigned apps, allows signed ones) |
| **Linux** | nothing | a host firewall (ufw, firewalld) may drop the ports silently; Flatpak needs network access |

## Signing

- **macOS**: a downloaded binary must be Developer ID signed and
  notarized, or Gatekeeper refuses it. A bare binary works, but the
  Local Network prompt then names the terminal that started it; an
  `.app` bundle gets its own name in prompts.
- **Windows**: Authenticode signing avoids SmartScreen's "unknown
  publisher" page. The firewall prompt shows the file description: set
  it (a `VERSIONINFO` resource).
- The repository's own `drawbluff` is built unsigned, like `tvpipe`.

## Robustness

- **Never panic on network data** holds for tvpipe and tvpipe-party.
  The one exception we know of is in `rtc` 0.21.1 (two panics reachable
  by a player who completed the WebRTC handshake;
  `docs/patches/rtc-0.21.1-unmarshal-panics.patch`): tvpipe-party
  contains them with `catch_unwind`, which costs that phone its fast
  path. **With `panic = "abort"` the fast path does not start** (a
  warning; the WebSocket works), until an rtc release has the fix.
- A player is never trusted: names are cleaned and unique, `moderate`
  can refuse words, actions are validated by the game, text reaches the
  TV as text. The VIP and the host can kick.
- Test with bots: drawbluff's `--bots N` joins fake players over real
  WebSockets (`src/bots.rs`), which is also how its tests play whole
  games.

## Before a release

- Play a whole game on a real TV with real phones, on the network you
  expect (a home Wi-Fi with a phone on mobile data, too).
- Check Ctrl-C mid-game: the TV leaves mirroring mode, the phones say
  the room closed.
- With `webrtc`: `scripts/live/party-rtc.ts` shows whether the channels
  open (`lan` and `auto`); on phones, the client's `transport` (or the
  `stick` example's status line) says `WebRTC` or `WebSocket`.
