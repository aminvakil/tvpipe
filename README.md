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

## Features

- Plays files and URLs, and mirrors cameras, capture cards and the screen
  through ffmpeg; suitable MP4/MOV/MKV files are sent without re-encoding.
- Shares a browser tab, window or screen, or puts a web page on the TV in a
  headless browser of its own.
- Sends to several receivers at once, on one timeline, and reconnects one
  that drops out.
- Plays to a browser tab too: previews, OBS sources, video calls.
- Takes raw or H.264 frames and sound from a game on stdin.
- One self-contained binary for macOS, Linux and Windows, with its own mDNS,
  protocol stacks and H.264/AAC encoders.

## Manual

| Page | Covers |
|---|---|
| [Overview](https://mitsuhiko.github.io/tvpipe/manual/) | what tvpipe does and does not do |
| [Quick start](https://mitsuhiko.github.io/tvpipe/manual/quick-start/) | find a TV, pair it, play a file |
| [Receivers](https://mitsuhiko.github.io/tvpipe/manual/receivers/) | `discover`, `pair`, `unpair`, `--to` |
| [Playing media](https://mitsuhiko.github.io/tvpipe/manual/play/) | files, URLs, cameras, the screen |
| [Sharing a tab or screen](https://mitsuhiko.github.io/tvpipe/manual/share/) | `share` |
| [A web page on the TV](https://mitsuhiko.github.io/tvpipe/manual/web-page/) | `share URL` |
| [Playing to a browser](https://mitsuhiko.github.io/tvpipe/manual/browser/) | `preview`, `--to browser`, OBS, video calls |
| [Game input](https://mitsuhiko.github.io/tvpipe/manual/games/) | SLRW frames on stdin |
| [Options](https://mitsuhiko.github.io/tvpipe/manual/options/) | the streaming options and encoders |
| [Configuration](https://mitsuhiko.github.io/tvpipe/manual/config/) | the config directory, environment variables |
| [Diagnostics](https://mitsuhiko.github.io/tvpipe/manual/diagnostics/) | `-v`, `--debug`, common problems |
| [Latency](https://mitsuhiko.github.io/tvpipe/manual/latency/) | `test-pattern`, `--latency`, measuring |

## Notes

<!-- Details the manual does not cover yet; drop each one once it does. -->

- **Installer:** `TVPIPE_VERSION` picks a version, `TVPIPE_INSTALL_DIR`
  another directory. The installers check the archive against the release's
  `SHA256SUMS`. The [releases](https://github.com/mitsuhiko/tvpipe/releases)
  have macOS, static Linux and Windows binaries for x86_64 and ARM64.
- **Config directory:** on Linux and other Unixes it is
  `$XDG_CONFIG_HOME/tvpipe` (default `~/.config/tvpipe`).
- **Pacing:** `--realtime` / `--no-realtime` paces the input in real time or
  not (default: real time for files).
- **`share URL`:** starting takes about a second, a reload or navigation
  leaves a gap of about half a second. A temporary profile left behind by a
  killed run is deleted by the next run. A site's 401/403/429 is reported.
  `--visible`, `--profile`, `--reload` and `--browser` only exist with a URL.
- **`--to browser` options** (`--open`, `--port`, `--wait-for-viewer`,
  `--new-token`) are the same for `play`, `test-pattern`, `preview` and
  `share`, and need `--to browser`; `share --open` without a URL opens the
  share page instead.

## Development

```sh
cargo build --release        # → target/release/tvpipe (Rust 1.85+, a C compiler)
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```

Tests that use ffmpeg as an oracle are skipped without it; tests against
real receivers or hardware run with `--ignored`. How and why tvpipe works is
in [`ARCHITECTURE.md`](ARCHITECTURE.md); protocols, formats, measurements and
per-OS status are in [`docs/`](docs/), and the website's sources in
[`site/`](site/).

## License

`tvpipe`'s source is Apache-2.0 ([`LICENSE`](LICENSE)), except
`src/airplay/fairplay/`, a port of
[doubletake](https://github.com/omarroth/doubletake) under
LGPL-3.0-or-later. While that module is included, the distributed binary is
under LGPL-3.0-or-later terms.

Some H.264 encoder tables (`src/media/h264enc/tables.rs`) are transcribed
from [openh264](https://github.com/cisco/openh264) (BSD-2-Clause), and the
fonts in `src/browser/fonts/` (also used by the website) are under the SIL
Open Font License 1.1. The license texts are in [`LICENSES/`](LICENSES).
