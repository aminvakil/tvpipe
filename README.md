# tvpipe

`tvpipe` is a command line tool for low-latency mirroring of video and audio
to **AirPlay** and **Google Cast** receivers (Apple TV, smart TVs,
Chromecast / Google TV).

It uses the real-time mirroring protocols of both ecosystems, not URL
playback, so the picture on the TV is a fraction of a second behind the
source. It can mirror files, URLs, capture devices, a built-in test pattern,
or raw frames that a game writes to its stdin. It can also send to several
receivers at once from a single encode.

- One Rust binary with the same code on macOS, Linux and Windows. It uses no
  OS media or discovery APIs.
- Three dependencies: `argument`, `rustls` and `ring`. Everything else (mDNS,
  RTSP, plists, AES, SRP, Matroska/MP4 demuxers, H.264 and AAC encoders) is
  written by hand.
- `ffmpeg` is optional. It is needed only for inputs that must be transcoded.

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
```

Useful `play` options:

| Option | Default | |
|---|---|---|
| `--latency MS` | receiver profile (≈100–200 ms) | target playout delay |
| `--size WxH`, `--fps N`, `--bitrate KBPS` | from the input and the receivers | output limits |
| `--encoder` | `auto` | `builtin`, `x264`, `videotoolbox`, `nvenc`, `qsv`, `vaapi`, `mf` |
| `--no-audio` | | video only |
| `--loop` | | loop the input |
| `--realtime` / `--no-realtime` | real time for files | pace the input |

### When ffmpeg is used

`tvpipe` avoids ffmpeg when it can:

- **Passthrough:** an MP4/MOV/MKV file with H.264, no B-frames and a
  keyframe at least every 4 s, at a size the receivers accept, is sent
  unchanged. Nothing is re-encoded.
- **Built-in encoders:** the test pattern and raw game frames go through the
  built-in H.264 encoder (and the AAC encoder for Cast audio) when you pass
  `--encoder builtin`, or when ffmpeg is not installed.
- **Encoded game input:** a game that encodes H.264 itself never needs
  ffmpeg for video.

Every other input is transcoded by ffmpeg. Run with `-v` to see which path
was chosen and why.

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
- The full format is in `docs/decisions/0002-slrw.md` and
  `docs/decisions/0008-slrw-encoded.md`.
- `cargo run --example slrw-gen` is a working sample producer.

## Configuration

AirPlay pairings are stored in `credentials.txt` in the config directory:

- Linux and other Unix: `$XDG_CONFIG_HOME/tvpipe` (default `~/.config/tvpipe`)
- macOS: `~/Library/Application Support/tvpipe`
- Windows: `%APPDATA%\tvpipe`

To use a different directory, set `TVPIPE_CONFIG_DIR`. The credentials file
holds a private key, so do not share it.

## Development

```sh
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```

Tests that use the oracle (ffmpeg as a decoder) are skipped when ffmpeg is
missing. Tests that need a receiver or hardware are behind `--ignored`.

For details see [`Architecture`](ARCHITECTURE.md) about the how and why.

## License

`tvpipe` is Apache-2.0, except `src/airplay/fairplay/`, which is a port of
[doubletake](https://github.com/omarroth/doubletake) under
LGPL-3.0-or-later. While that module is included, the distributed binary is
under LGPL-3.0-or-later terms. See [`LICENSES/`](LICENSES).
