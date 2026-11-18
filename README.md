<p align="center">
  <img src="docs/logo/tvpipe.svg" alt="tvpipe logo" width="260">
</p>

# tvpipe

`tvpipe` is a command line tool for low-latency mirroring of video and audio
to **AirPlay** and **Google Cast** receivers (Apple TV, smart TVs,
Chromecast / Google TV).

It uses the real-time mirroring protocols of both ecosystems, not URL
playback, so the picture on the TV is a fraction of a second behind the
source. It can mirror files, URLs, capture devices, a built-in test pattern,
or raw frames that a game writes to its stdin. It can also send to several
receivers at once from a single encode.

## Building

```sh
cargo build --release        # → target/release/tvpipe
```

You need Rust 1.85 or newer and a C toolchain for `ring`. For general file
and capture input, put `ffmpeg` (and `ffprobe`) in `PATH`, or pass
`--ffmpeg PATH` or set `$TVPIPE_FFMPEG`.

## Quick start

```sh
# 1. Find receivers
tvpipe discover

# 2. Apple TV only: pair once (the TV shows a PIN)
tvpipe pair "Living Room"

# 3. Mirror something
tvpipe play --to "Living Room" movie.mp4
```

Press Ctrl-C, or `q` then Enter, to stop. Either way the receiver gets a
proper teardown and leaves mirroring mode.

## Usage

```
tvpipe discover [--timeout SECS] [--json] [--interface ADDR]
tvpipe pair NAME|ADDR [--pin PIN]
tvpipe unpair NAME|ADDR
tvpipe play --to NAME|ADDR [--to ...] [OPTIONS] (INPUT | - | -- FFMPEG_INPUT_ARGS...)
tvpipe test-pattern --to NAME|ADDR [OPTIONS]
tvpipe share --to NAME|ADDR [--open] [--port PORT] [OPTIONS]
tvpipe preview [--open] [--port PORT] [OPTIONS] [INPUT | - | -- FFMPEG_INPUT_ARGS...]
tvpipe debug-pipeline [OPTIONS] INPUT
```

Global flags: `-v` for debug logs, `--debug` for protocol traces. Run any
command with `--help` to see all of its options.

### Choosing receivers (`--to`)

| Form | Meaning |
|---|---|
| `"Living Room"` / `liv` | discovered name, case-insensitive; a unique prefix is enough |
| `192.168.1.20` | Cast on port 8009 (no discovery) |
| `192.168.1.20:7000` | AirPlay on port 7000 (no discovery) |
| `airplay:NAME` / `cast:NAME` | force a protocol (for TVs that speak both) |
| `browser` | a player page in a browser tab on this computer (below) |

A device that advertises both protocols uses Cast unless you write
`airplay:`. You can repeat `--to` to stream to several receivers at once.

### Inputs

```sh
# A file or URL
tvpipe play --to TV movie.mkv
tvpipe play --to TV --loop intro.mp4
tvpipe play --to TV https://example.com/stream.m3u8

# Any ffmpeg input after `--` (screen or camera capture)
tvpipe play --to TV -- -f avfoundation -i 1:0         # macOS
tvpipe play --to TV -- -f x11grab -i :0.0             # Linux
tvpipe play --to TV -- -f gdigrab -i desktop          # Windows

# Two receivers, one encode
tvpipe play --to "Apple TV" --to "Chromecast" movie.mp4

# Latency check: burned-in wallclock and frame counter, beep and flash every second
tvpipe test-pattern --to TV

# A browser tab, window or screen: open the printed URL, press Share
tvpipe share --to TV --open
```

`share` serves a page on 127.0.0.1 that captures with the browser
(`getDisplayMedia`) and encodes H.264 with WebCodecs (Chrome, Edge, Safari
16.4+, Firefox 130+).  Tick "share audio" in Chrome's picker for sound.  The
stream takes the TV's screen shape with the capture centered (black bars), at
the capture's size up to what the TV allows.  Stopping the share or closing the
page ends the session.

### Playing To A Browser (`--to browser`, `preview`)

```sh
# Preview a file, a capture or the test pattern (no INPUT) in a browser tab
tvpipe preview --open movie.mp4
tvpipe preview --open

# A browser tab next to a TV, on the same timeline
tvpipe play --to "Living Room" --to browser movie.mp4
```

`--to browser` is a receiver like a TV: tvpipe serves a player page on 127.0.0.1
and prints its URL (`--open` opens it in the default browser).  The URL stays
the same across runs (port 8765 unless `--port` says otherwise, and a token kept
in the config directory), so an open tab or an OBS source picks up the next run
by itself.  Use `--new-token` to change it, which locks out old URLs.

If 8765 is taken, tvpipe warns and uses another port for that run.  The page
decodes H.264 with WebCodecs and plays PCM with Web Audio (Chrome, Edge, Safari
16.4+, Firefox 130+), in step with the other receivers.  Any number of tabs can
watch and a tab that joins later starts synchronized.

Without a tab, the stream runs anyway but you can use `--wait-for-viewer` to
defer the start until a tab is open.  `preview` is short for `play --to
browser`.

On the page: double-click or `F` for fullscreen, `S` for statistics (delay,
frame rate, drops, the wallclock of the shown frame).  The controls and the
cursor hide when the mouse rests.  Browsers start audio only after a click:
click the page once (the "Click for sound" button).

- **OBS:** add the URL as a browser source with `&ui=0`; "Control audio
  via OBS" routes its sound into the mix.
- **Video calls:** present the tab in Meet, Zoom or Teams (share the tab
  with its audio).

Useful `play` options:

| Option | Default | |
|---|---|---|
| `--latency MS` | Cast 300 ms; AirPlay automatic (video 100, audio 170 ms); both 300 ms when mixed; browser alone 100 ms | playout delay, the same on every receiver (5–2000) |
| `--size WxH`, `--fps N`, `--bitrate KBPS` | from the input and the receivers | output limits |
| `--encoder` | `auto` | `builtin`, `x264`, `videotoolbox`, `nvenc`, `qsv`, `vaapi`, `mf` |
| `--no-audio` | | video only |
| `--loop` | | loop the input |
| `--realtime` / `--no-realtime` | real time for files | pace the input |
| `--open`, `--port PORT`, `--wait-for-viewer`, `--new-token` | port 8765 | the `--to browser` page |

### When ffmpeg is used

`tvpipe` avoids ffmpeg when it can:

- **Passthrough:** an MP4/MOV/MKV file with H.264, no B-frames and a
  keyframe at least every 4 s, at a size the receivers accept, is sent
  unchanged. Nothing is re-encoded.
- **Built-in encoders:** the test pattern always goes through the built-in
  H.264 encoder (and the AAC encoder for Cast audio); naming an ffmpeg
  encoder (`--encoder x264` etc.) streams ffmpeg's `testsrc2` instead. Raw
  game frames use the built-in encoders when you pass `--encoder builtin`,
  or when ffmpeg is not installed.
- **Encoded game input:** a game that encodes H.264 itself never needs
  ffmpeg for video.

Every other input is transcoded by ffmpeg.  Run with `-v` to see which path was
chosen and why.

### Game input (SLRW)

A game can spawn `tvpipe` and write frames to its stdin:

```sh
mygame | tvpipe play --to TV --raw-video 1920x1080@60:rgba --raw-audio 48000:2:s16le -
```

The stream starts with the 4-byte magic `SLRW`, followed by records:

```
u8 kind | u64le pts_us | u32le len | payload
kind: 0 = end, 1 = raw video frame, 2 = interleaved PCM,
      3 = H.264 access unit, 4 = H.264 config, 5 = control
```

- Raw pixel formats: `rgba`, `bgra`, `rgb0`, `bgr0`, `rgb24`, `bgr24`,
  `nv12`, `yuv420p`.
- Encoded H.264: use `--raw-video WxH@FPS:h264` and send kinds 3 and 4. If
  the game also enables keyframe requests (kind 5), `tvpipe` writes the line
  `keyframe` to its stdout whenever a receiver needs a new IDR.
- `cargo run --example slrw-gen` is a working sample producer.
- The full format is in [`docs/slrw.md`](docs/slrw.md).

## Configuration

AirPlay pairings are stored in `credentials.txt` in the config directory:

- Linux and other Unix: `$XDG_CONFIG_HOME/tvpipe` (default `~/.config/tvpipe`)
- macOS: `~/Library/Application Support/tvpipe`
- Windows: `%APPDATA%\tvpipe`

To use a different directory, set `TVPIPE_CONFIG_DIR`. The credentials file
holds a private key, so do not share it. `player-token` next to it holds
the browser player's URL token (`--new-token` replaces it).

## Development

```sh
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```

Tests that use the oracle (ffmpeg as a decoder) are skipped when ffmpeg is
missing. Tests that need a receiver or hardware are behind `--ignored`.

For details see [`Architecture`](ARCHITECTURE.md) about the how and why.

## License

`tvpipe` is Apache-2.0 ([`LICENSE`](LICENSE)), except
`src/airplay/fairplay/`, which is a port of
[doubletake](https://github.com/omarroth/doubletake) under
LGPL-3.0-or-later. While that module is included, the distributed binary is
under LGPL-3.0-or-later terms. See [`LICENSES/`](LICENSES).

Some H.264 encoder tables (`src/media/h264enc/tables.rs`) are transcribed
from [openh264](https://github.com/cisco/openh264) (BSD-2-Clause, see
[`LICENSES/BSD-2-Clause-openh264.txt`](LICENSES/BSD-2-Clause-openh264.txt)).
