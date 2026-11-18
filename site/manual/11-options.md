---
title: Options and encoders
summary: Every streaming option, and when ffmpeg runs.
group: Reference
spec:
  Applies to: `play`, `test-pattern`; `preview`, `share`
---

The options shared by the streaming commands. Run any command with `--help` for its full list.

## Streaming options

:::aside Global
`-v` Debug output.

`--debug` Protocol traces.
:::

| Option | Default | Effect |
|---|---|---|
| `--to NAME|ADDR` | | Receiver. Repeat for several. |
| `--latency MS` | [see latency](09-latency.md) | Playout delay on every receiver, 5–2000. |
| `--size WxH` | input, receivers | Largest output size. |
| `--fps N` | input | Output frame rate. |
| `--bitrate KBPS` | receivers | Video bitrate. |
| `--encoder` | `auto` | Video encoder, below. |
| `--no-audio` | | Video only. |
| `--loop` | | Loop the input. |
| `--realtime`, `--no-realtime` | real time for files | Pace the input. |
| `--ffmpeg PATH` | `PATH` | The ffmpeg binary. |

### Browser player

| Option | Default | Effect |
|---|---|---|
| `--open` | | Open the page in the default browser. |
| `--port PORT` | 8765 | Port on 127.0.0.1; 0 for any. |
| `--wait-for-viewer` | | Start once a tab is watching. |
| `--new-token` | | A new URL; old ones stop working. |

## Encoders

:::aside Hardware
All except `builtin` are ffmpeg encoders. The hardware ones need an ffmpeg built with them and a matching GPU.
:::

| Value | Encoder |
|---|---|
| `auto` | Picks one for the input and platform. |
| `builtin` | tvpipe’s own H.264 encoder. Test pattern and raw game frames only. |
| `x264` | Software, on every platform. |
| `videotoolbox` | macOS hardware. |
| `nvenc` | NVIDIA hardware. |
| `qsv` | Intel Quick Sync. |
| `vaapi` | Linux VA-API. |
| `mf` | Windows Media Foundation. |

## When ffmpeg is used

:::aside Check
`-v` prints which path was chosen and why.
:::

| Source | ffmpeg |
|---|---|
| Suitable H.264 file | No. Passed through unchanged. |
| Test pattern | No, unless an ffmpeg encoder is named. |
| Raw game frames | No with `--encoder builtin` or without ffmpeg. |
| A game’s own H.264 | Never for video. |
| Browser tab or page | No. The browser encodes. |
| Everything else | Yes, to decode and re-encode. |
