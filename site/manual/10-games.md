---
title: Game input
summary: Stream frames from your own program.
group: Use
spec:
  Input: `-` (stdin)
  Format: SLRW
  Sample: `examples/slrw-gen.rs`
---

A game, or any program, can start tvpipe and write its frames and sound to tvpipe’s standard input. tvpipe encodes and streams them.

## Raw frames

:::aside Pixel formats
`rgba` `bgra` `rgb0` `bgr0` `rgb24` `bgr24` `nv12` `yuv420p`
:::

:::aside Audio
`RATE:CHANNELS:s16le`
:::

```sh
$ mygame | tvpipe play --to TV --raw-video 1920x1080@60:rgba --raw-audio 48000:2:s16le -
```

Raw frames go to the built-in H.264 encoder with `--encoder builtin` or when ffmpeg is not installed, and to ffmpeg otherwise. Sound is sent as PCM to AirPlay and browsers, and encoded with the built-in AAC encoder for Cast.

## The stream format

:::aside Byte order
Little-endian, what game engines on x86 and ARM write natively.
:::

The stream starts with the four bytes `SLRW`, followed by records:

```text
u8 kind | u64 pts_us | u32 len | payload
```

| Kind | Name | Payload |
|---|---|---|
| 0 | `end` | Empty. End of stream. |
| 1 | `video` | One raw frame in the `--raw-video` size and format. |
| 2 | `audio` | Whole interleaved PCM sample frames. |
| 3 | `h264` | `u8 flags`, then one access unit. |
| 4 | `h264-config` | `u8 format`, then `avcC` or Annex B SPS and PPS. |
| 5 | `control` | `u32 flags`. Bit 0 turns on keyframe requests. |

Writing a record takes a few lines in any language:

```rust
fn record(out: &mut impl Write, kind: u8, pts_us: u64, payload: &[u8]) -> io::Result<()> {
    out.write_all(&[kind])?;
    out.write_all(&pts_us.to_le_bytes())?;
    out.write_all(&(payload.len() as u32).to_le_bytes())?;
    out.write_all(payload)
}
```

## Sending your own H.264

:::aside No ffmpeg
In this mode video never goes through ffmpeg.
:::

A game that already encodes, typically with a hardware encoder, uses `--raw-video WxH@FPS:h264` and sends kinds 3 and 4. Access units must not contain B-frames, and their `pts_us` must increase.

### Keyframe requests
When a receiver needs a new keyframe, for example when it joins or after loss, tvpipe writes the line `keyframe` to its standard output, if the game turned this on with a `control` record. The game should make its next frame an IDR. Requests are limited to one per 200 ms.

## A working example

:::caution
Windows PowerShell 5.1 re-encodes pipes as text and corrupts the stream. Use `cmd.exe`, PowerShell 7.4+, or spawn tvpipe from the game.
:::

```sh
$ cargo run --example slrw-gen | tvpipe play --to TV --raw-video 1280x720@30:rgba -
```

`examples/slrw-gen.rs` produces both raw and encoded streams. The complete format, including timing and limits, is in `docs/slrw.md`.
