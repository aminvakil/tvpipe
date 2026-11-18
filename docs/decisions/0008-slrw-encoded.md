# 0008: SLRW encoded H.264 and the keyframe back channel (amends 0002)

Status: accepted by lead. Proposed by W9. Affects only the game-input
format and `media::input`. Implements option B of
`docs/ffmpeg-optional.md` (decision 0006).

## Selecting the mode

`--raw-video WxH@FPS:h264` (pixel format `h264`) means the game sends
encoded H.264; no CLI change is needed. In this mode video never goes
through ffmpeg. `WxH` and `FPS` are used when the SPS lacks them (no VUI
timing) and for a warning when the SPS size differs. Raw records (kind 1)
are an error in this mode, and the new kinds are an error in raw mode
(with a message naming the other mode).

## New record kinds

Header unchanged (`u8 kind | u64 pts_us | u32 len`, little-endian, 0002).

| kind | name | payload |
|---|---|---|
| 3 | `h264` | `u8 flags` then one access unit |
| 4 | `h264-config` | `u8 format` then the decoder configuration |
| 5 | `control` | `u32 flags` (little-endian) |

- **`h264`** (3): one access unit (all NAL units of one picture). `flags`
  bit 0 (`0x01`): keyframe (IDR); bit 1 (`0x02`): Annex B (start codes),
  otherwise AVCC with 4-byte big-endian lengths. Other bits are reserved:
  writers set 0, readers ignore them. `pts_us` is the presentation time
  and must increase (no B-frames: decode order = presentation order). The
  keyframe flag is a hint; streamlib also checks for an IDR NAL unit and
  trusts that.
- **`h264-config`** (4): `format` 0: an `avcC` record
  (AVCDecoderConfigurationRecord, ISO/IEC 14496-15 §5.3.3.1); `format` 1:
  Annex B NAL units containing at least one SPS and one PPS. Optional: if
  absent, the SPS/PPS of the first keyframe (in band, as hardware encoders
  emit them before every IDR) are used. Sending the same configuration
  again is fine; a different one after streaming started is an error
  (receivers were set up with the first). `pts_us` is ignored.
- **`control`** (5): `flags` bit 0 (`0x01`): the game reads keyframe
  requests on its end of streamlib's stdout (below). Other bits reserved.
  May appear anywhere; the last one wins. `pts_us` is ignored.
- Audio stays kind 2 (PCM, `--raw-audio`).

Payload limits: 64 MiB per `h264` record, 1 MiB for `h264-config`, `len`
exactly 4 for `control`.

## Keyframe back channel

When a receiver needs a keyframe (a sink joining, a queue overflow, a PLI
or loss on Cast), and the game enabled it with a `control` record,
streamlib writes the ASCII line

```
keyframe\n
```

to **its stdout**. The game spawned streamlib and already holds both ends
of its stdio, so this needs no port, no fd passing and no platform code;
streamlib writes nothing else to stdout in `play` (logs go to stderr).
Requests are coalesced and rate-limited (at most one per 200 ms while a
receiver keeps asking); the game should make its next frame an IDR. A game
that does not read stdout must not enable the channel: the line would end
up wherever stdout points (and a full pipe only stalls the writer thread,
never the stream). Lines are newline-terminated words so that more
requests (e.g. `bitrate 4000`) can be added later; readers ignore lines
they do not know.

Considered and rejected: a localhost TCP port (the game has to learn the
port: another CLI flag or a parse of our stderr) and a request record on
stdin (stdin is the game's write side).

## Timeline

The first kept record (audio, or the first keyframe; non-key video before
it is dropped) defines media time 0. Video pts = `(pts_us - origin)` in
90 kHz ticks; a pts that does not increase is bumped by one tick with a
warning. Audio is timed by its sample count from its first record's
`pts_us`, and resyncs when the game's `pts_us` drifts more than 50 ms
from the count (a real gap).

## Audio

PCM goes directly to a family whose receivers all accept `PcmS16le` at the
game's rate and channel count (AirPlay: sent as verbatim ALAC by the sink),
cut into frames of the size the session asks for. Cast needs Opus or AAC:
until the built-in AAC-LC encoder (W10) lands, Cast receivers get video
only from an encoded-SLRW game (a warning says so); then that family is
encoded in-process.

Code: `src/media/slrw.rs` (format, shared with `examples/slrw-gen.rs`),
`src/media/input.rs` (`SlrwEncodedInput`).
