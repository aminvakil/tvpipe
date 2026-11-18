---
title: Commands and options
summary: Every command, and the options they share.
group: Reference
spec:
  Commands: `play`, `preview`; `share`, `test-pattern`
---

Every command lists all of its options with `--help`.

## Commands

| Command | Does |
|---|---|
| `discover` | Lists the receivers on the network. See [Receivers](04-receivers.md). |
| `pair`, `unpair` | Pairs with an Apple TV, or forgets the pairing. |
| `play` | Plays a file or URL, or mirrors a camera or the screen. See [Playing media](05-play.md). |
| `share` | Shares a browser tab, a window, the screen or a web page. See [Sharing](06-share.md). |
| `preview` | Plays in a browser tab. See [Playing to a browser](08-browser.md). |
| `test-pattern` | Sends a test pattern. See [Latency](13-latency.md). |
| `debug-pipeline` | Reads an input without a receiver. See [ffmpeg and encoding](14-encoding.md). |

## Streaming

:::aside Everywhere
`-v` More output.

`--debug` Protocol traces.
:::

| Option | Effect |
|---|---|
| `--to NAME|ADDR` | Where to send. Repeat for several receivers. |
| `--size WxH` | The largest picture size. |
| `--fps N` | Frames per second. |
| `--bitrate KBPS` | Video bitrate. |
| `--encoder NAME` | The video encoder, below. |
| `--no-audio` | Picture only. |
| `--loop` | Start over at the end. |
| `--realtime`, `--no-realtime` | Read the input at playback speed, or not. Files are read at playback speed. |
| `--reconnect SECS` | How long to reconnect a receiver that dropped out. Default 120, `0` is off. |
| `--latency MS` | How long receivers wait before they show a frame. See [Latency](13-latency.md). |
| `--raw-video`, `--raw-audio` | Frames and sound on standard input. See [Game input](09-games.md). |
| `--ffmpeg PATH` | The ffmpeg to use. |

## Browser player

These need `--to browser`.

| Option | Effect |
|---|---|
| `--open` | Open the player page. |
| `--port PORT` | The port of the player page. Default 8765. |
| `--wait-for-viewer` | Wait for a tab before starting. |
| `--new-token` | A new address for the player page. Old ones stop working. |

## Encoders

:::aside Hardware
The hardware encoders need an ffmpeg built with them.
:::

| Value | Encoder |
|---|---|
| `auto` | Picks one. The default. |
| `builtin` | tvpipe’s own encoder. |
| `x264` | Software, on every platform. |
| `videotoolbox` | macOS hardware. |
| `nvenc` | NVIDIA hardware. |
| `qsv` | Intel Quick Sync. |
| `vaapi` | Linux VA-API. |
| `mf` | Windows Media Foundation. |
