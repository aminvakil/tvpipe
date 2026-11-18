---
title: Options
summary: The options of the streaming commands.
group: Reference
spec:
  Commands: `play`, `preview`; `share`, `test-pattern`
---

The options the streaming commands share. Every command lists all of its options with `--help`.

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
| `--ffmpeg PATH` | The ffmpeg to use. |

## Browser player

| Option | Effect |
|---|---|
| `--open` | Open the player page. |
| `--port PORT` | The port of the player page. Default 8765. |
| `--wait-for-viewer` | Wait for a tab before starting. |

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
