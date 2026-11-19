<p align="center">
  <img src="docs/logo/tvpipe.svg" alt="tvpipe logo" width="260">
</p>

# tvpipe

`tvpipe` is a command line tool for low-latency mirroring of video and audio
to **AirPlay** and **Google Cast** receivers (Apple TV, smart TVs,
Chromecast / Google TV) and browser tabs. It uses the real-time mirroring
protocols of both ecosystems, not URL playback, so the picture on the TV is a
fraction of a second behind the source.

**Website and manual: <https://mitsuhiko.github.io/tvpipe/>**

```sh
tvpipe discover                                        # find receivers
tvpipe pair "Living Room"                              # Apple TV only, once
tvpipe play --to "Living Room" movie.mp4               # a file or URL
tvpipe play --to "Living Room" --to Kitchen movie.mp4  # two TVs, one encode
tvpipe screen --to Kitchen                             # the screen, with sound
tvpipe screen --to Kitchen --window Keynote            # one window
tvpipe share --to Kitchen --open                       # a browser tab or the screen
tvpipe share https://status.example.com --to Kitchen   # a web page
```

## Installing

```sh
curl -fsSL https://mitsuhiko.github.io/tvpipe/install.sh | sh     # macOS, Linux
```

```powershell
irm https://mitsuhiko.github.io/tvpipe/install.ps1 | iex           # Windows
```

Playing files, URLs and capture devices needs `ffmpeg` (and `ffprobe`); see
[Installation](https://mitsuhiko.github.io/tvpipe/manual/install/) for
ffmpeg, the release archives and building from source.

From Node.js, `npm install @tvpipe/sdk` installs an SDK and the binary for your
platform ([`npm/tvpipe`](npm/tvpipe/README.md)):

```js
import { play } from "@tvpipe/sdk";
await play("movie.mp4", { to: "Living Room" });
```

From Rust, `cargo add tvpipe` links tvpipe into your program (no binary,
no async runtime); a game streams its own frames and sound
([`crates/tvpipe`](crates/tvpipe/README.md),
[From Rust](https://mitsuhiko.github.io/tvpipe/manual/rust/)):

```rust
let mut s = tvpipe::stream(Options::new().to("Living Room").video(1280, 720, 60, PixelFormat::Rgba))?;
s.write_video(&frame, s.elapsed())?;
```

## Features

- Plays files and URLs, and mirrors cameras, capture cards and the screen
  through ffmpeg; suitable MP4/MOV/MKV files are sent without re-encoding.
- Mirrors a screen or a single window with the computer's sound, captured
  by a Chromium-based browser it starts (or by ffmpeg).
- Shares a browser tab, window or screen, or puts a web page on the TV in a
  headless browser of its own.
- Sends to several receivers at once, on one timeline, and reconnects one
  that drops out.
- Plays to a browser tab too: previews, OBS sources, video calls; and on a
  public link for people elsewhere, who can also share their screen to
  your TV.
- Takes raw or H.264 frames and sound from a game on stdin.
- One self-contained binary for macOS, Linux and Windows, with its own mDNS,
  protocol stacks and H.264/AAC encoders.

## Public links

```sh
tvpipe preview --tunnel                    # anyone with the link watches in a browser
tvpipe share --to "Living Room" --tunnel   # someone elsewhere shares their screen to the TV
```

`--tunnel` also puts the browser player or the share page on a public
https link. Two providers, both built in (no account, no extra program):

- `--tunnel` (or `--tunnel=cloudflare`): a [Cloudflare Quick
  Tunnel](https://try.cloudflare.com) on `*.trycloudflare.com`, about 7 s
  to set up, a new hostname every run. Cloudflare ends TLS, so it can read
  the page and the stream.
- `--tunnel=opentunnel`: [OpenTunnel](https://opentunnel.xyz), with TLS
  ending in tvpipe (the relay cannot read the page or the stream). The
  first use creates the machine's tunnel and certificate (about 15 s,
  once; kept in the config directory under `opentunnel/`), later runs
  take 0.2 s.

The link is new every run and has a token of its own; at most 4 viewers
come in that way. A remote share shows up only after you answer `y` in the
terminal (`--yes` skips the question), and its link lets people in for 60
minutes (`--tunnel-ttl`). Details: [`docs/tunnels.md`](docs/tunnels.md).

In a terminal, tvpipe also draws the public link as a QR code, so a phone
can open it by scanning the screen (`--no-qr` or `TVPIPE_NO_QR=1` turns
that off; never with `--machine`).

## Manual

| Page | Covers |
|---|---|
| [Overview](https://mitsuhiko.github.io/tvpipe/manual/) | what tvpipe does and does not do |
| [Installation](https://mitsuhiko.github.io/tvpipe/manual/install/) | the installers, ffmpeg, building from source |
| [Quick start](https://mitsuhiko.github.io/tvpipe/manual/quick-start/) | find a TV, pair it, play a file |
| [Receivers](https://mitsuhiko.github.io/tvpipe/manual/receivers/) | `discover`, `pair`, `unpair`, `--to`, reconnecting |
| [Playing media](https://mitsuhiko.github.io/tvpipe/manual/play/) | files, URLs, cameras |
| [Sharing the screen](https://mitsuhiko.github.io/tvpipe/manual/screen/) | `screen` and `share`: the screen, a window or a browser tab |
| [A web page on the TV](https://mitsuhiko.github.io/tvpipe/manual/web-page/) | `share URL` |
| [Playing to a browser](https://mitsuhiko.github.io/tvpipe/manual/browser/) | `preview`, `--to browser`, OBS, video calls |
| [Public links](https://mitsuhiko.github.io/tvpipe/manual/links/) | `--tunnel` |
| [Game input](https://mitsuhiko.github.io/tvpipe/manual/games/) | SLRW frames on stdin |
| [From Node.js](https://mitsuhiko.github.io/tvpipe/manual/node/) | the `@tvpipe/sdk` npm package |
| [From Rust](https://mitsuhiko.github.io/tvpipe/manual/rust/) | the `tvpipe` crate |
| [Party games](https://mitsuhiko.github.io/tvpipe/manual/party/) | `@tvpipe/party`: games played with phones, the TV as the main screen |
| [Commands and options](https://mitsuhiko.github.io/tvpipe/manual/options/) | every command, the shared options, encoders |
| [Configuration](https://mitsuhiko.github.io/tvpipe/manual/config/) | the config directory, environment variables |
| [Diagnostics](https://mitsuhiko.github.io/tvpipe/manual/diagnostics/) | `-v`, `--debug`, common problems |
| [Latency](https://mitsuhiko.github.io/tvpipe/manual/latency/) | `test-pattern`, `--latency`, measuring |
| [ffmpeg and encoding](https://mitsuhiko.github.io/tvpipe/manual/encoding/) | when ffmpeg runs, `debug-pipeline` |

## Development

```sh
cargo build --release        # → target/release/tvpipe (Rust 1.85+, a C compiler)
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

The workspace has three crates: [`crates/tvpipe`](crates/tvpipe/) (the
library, Apache-2.0), [`crates/tvpipe-cli`](crates/tvpipe-cli/) (the
`tvpipe` command on top of it) and
[`crates/tvpipe-fairplay`](crates/tvpipe-fairplay/) (the LGPL FairPlay port,
behind the library's opt-in `fairplay` feature).

The Node SDK's tests run against the debug binary: `cargo build && cd
npm/tvpipe && npm ci && npm run check && npm test`. A release bumps the
version in `Cargo.toml`, `Cargo.lock`, `npm/tvpipe/package.json` and
`npm/party/package.json` (with its `@tvpipe/sdk` peer dependency)
together (CI checks), then tags `vX.Y.Z`; `release.yml` publishes the GitHub
release and the npm packages. Programs in other languages can drive tvpipe
through [`--machine`](docs/machine.md).

Tests that use ffmpeg as an oracle are skipped without it; tests against
real receivers or hardware run with `--ignored`. How and why tvpipe works is
in [`ARCHITECTURE.md`](ARCHITECTURE.md); protocols, formats, measurements and
per-OS status are in [`docs/`](docs/), and the website's sources in
[`site/`](site/).

## License

`tvpipe`'s source is Apache-2.0 ([`LICENSE`](LICENSE)), except the
FairPlay crate [`crates/tvpipe-fairplay/`](crates/tvpipe-fairplay/), a port
of [doubletake](https://github.com/omarroth/doubletake) under
LGPL-3.0-or-later, which is only built with the `fairplay` feature. The
distributed binary includes it, so it is under LGPL-3.0-or-later terms.

The root certificates in `crates/tvpipe/src/net/roots/` (for the `--tunnel` service's
HTTPS) are Mozilla's list from the CCADB, under CDLA-Permissive-2.0 (see
[`LICENSES/CDLA-Permissive-2.0-ccadb.txt`](LICENSES/CDLA-Permissive-2.0-ccadb.txt)).

Some H.264 encoder tables (`crates/tvpipe/src/media/h264enc/tables.rs`) are transcribed
from [openh264](https://github.com/cisco/openh264) (BSD-2-Clause), and the
fonts in `crates/tvpipe/src/browser/fonts/` (also used by the website) are under the SIL
Open Font License 1.1. The license texts are in [`LICENSES/`](LICENSES).
