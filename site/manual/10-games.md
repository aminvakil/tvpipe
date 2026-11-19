---
title: Game input
summary: Frames and sound from your own program.
group: Use
spec:
  Input: `-` (stdin)
  Format: SLRW
  Example: `crates/tvpipe/examples/slrw-gen.rs`
---

A game, or any other program, can start tvpipe and write its frames and sound to tvpipe’s standard input.

## Raw frames

:::aside Pixel formats
`rgba` `bgra` `rgb0` `bgr0` `rgb24` `bgr24` `nv12` `yuv420p`
:::

:::aside Sound
`RATE:CHANNELS:s16le`
:::

```sh
$ mygame | tvpipe play --to TV --raw-video 1920x1080@60:rgba --raw-audio 48000:2:s16le -
```

## The stream format

:::aside Byte order
Little endian, as x86 and ARM write it.
:::

The stream starts with the four bytes `SLRW`, followed by records:

```text
u8 kind | u64 pts_us | u32 len | payload
```

| Kind | Name | Payload |
|---|---|---|
| 0 | `end` | Nothing. The end of the stream. |
| 1 | `video` | One raw frame in the `--raw-video` size and format. |
| 2 | `audio` | Interleaved PCM samples. |
| 3 | `h264` | `u8 flags`, then one access unit. |
| 4 | `h264-config` | `u8 format`, then `avcC` or Annex B SPS and PPS. |
| 5 | `control` | `u32 flags`. Bit 0 turns on keyframe requests. |

Writing a record takes a few lines:

```rust
fn record(out: &mut impl Write, kind: u8, pts_us: u64, payload: &[u8]) -> io::Result<()> {
    out.write_all(&[kind])?;
    out.write_all(&pts_us.to_le_bytes())?;
    out.write_all(&(payload.len() as u32).to_le_bytes())?;
    out.write_all(payload)
}
```

## Sending your own H.264

A game that encodes video itself, for example with a hardware encoder, uses `--raw-video WxH@FPS:h264` and sends kinds 3 and 4. The frames must not contain B-frames, and `pts_us` must increase.

### Keyframe requests
When a receiver needs a keyframe, tvpipe writes the line `keyframe` to its standard output. The game then makes its next frame a keyframe. A `control` record turns this on.

## An example

:::caution
Windows PowerShell 5.1 breaks binary pipes. Use `cmd.exe`, PowerShell 7.4 or newer, or start tvpipe from the game.
:::

```sh
$ cargo run --example slrw-gen | tvpipe play --to TV --raw-video 1280x720@30:rgba -
```

[`slrw-gen.rs`](https://github.com/mitsuhiko/tvpipe/blob/main/crates/tvpipe/examples/slrw-gen.rs) writes both raw and H.264 streams. The full format is in [docs/slrw.md](https://github.com/mitsuhiko/tvpipe/blob/main/docs/slrw.md).
