# SLRW: game input

SLRW is the format a game (or any program) writes to `tvpipe`'s stdin to
stream its own frames. This is the full reference; the manual's
[Game input](https://mitsuhiko.github.io/tvpipe/manual/games/) is the short version.

```sh
mygame | tvpipe play --to TV --raw-video 1920x1080@60:rgba --raw-audio 48000:2:s16le -
```

The same record framing is used on the WebSockets of the share page
([`share-protocol.md`](share-protocol.md)) and the browser player
([`player-protocol.md`](player-protocol.md)).

Code: `src/media/slrw.rs` (the format, shared with `examples/slrw-gen.rs`),
`src/media/input.rs` (encoded mode), `src/media/builtin.rs` (raw mode with
the built-in encoders). `cargo run --example slrw-gen` is a working
producer for both modes.

## Framing

The stream starts with the 4-byte magic `SLRW`, followed by records:

```
u8 kind | u64 pts_us | u32 len | payload
```

`pts_us` and `len` are **little-endian** (what game engines on x86 and ARM
write natively).

| kind | name | mode | payload |
|---|---|---|---|
| 0 | `end` | both | empty (`len` must be 0) |
| 1 | `video` | raw | one raw frame in the `--raw-video` size and pixel format |
| 2 | `audio` | both | any whole number of interleaved PCM sample frames (`--raw-audio`) |
| 3 | `h264` | encoded | `u8 flags` then one access unit |
| 4 | `h264-config` | encoded | `u8 format` then the decoder configuration |
| 5 | `control` | encoded | `u32 flags` (little-endian) |

- Kind 6 (`command`) exists only with `tvpipe --machine`
  ([`machine.md`](machine.md)), which also turns keyframe requests into
  JSON events.
- Any other kind is an error. EOF on a record boundary ends the stream
  like an `end` record.
- Audio records without `--raw-audio` are ignored.
- A raw video payload of the wrong size is an error for pixel formats
  whose size is known.

## Modes

**Raw** (`--raw-video WxH@FPS:FMT`, `FMT` one of `rgba`, `bgra`, `rgb0`,
`bgr0`, `rgb24`, `bgr24`, `nv12`, `yuv420p`): kind 1 carries frames, which
go to the built-in H.264 encoder (`--encoder builtin`, or when ffmpeg is
missing; keyframes on demand) or to ffmpeg (through two localhost TCP
listeners, the same on every OS). With ffmpeg, the first frames are dropped
while it starts (about 0.3 s with Debian's ffmpeg 7.1).

**Encoded** (`--raw-video WxH@FPS:h264`): the game sends H.264 it encoded
itself (typically with a hardware encoder) and video never goes through
ffmpeg. `WxH` and `FPS` are used when the SPS lacks them (no VUI timing)
and for a warning when the SPS size differs. Kind 1 is an error in this
mode and kinds 3–5 are an error in raw mode, with a message naming the
other mode.

## Encoded records

- **`h264`** (3): one access unit (all NAL units of one picture). `flags`
  bit 0 (`0x01`): keyframe (IDR); bit 1 (`0x02`): Annex B (start codes),
  otherwise AVCC with 4-byte big-endian lengths. Other bits are reserved:
  writers set 0, readers ignore them. `pts_us` is the presentation time
  and must increase (no B-frames: decode order = presentation order). The
  keyframe flag is a hint; tvpipe also checks for an IDR NAL unit and
  trusts that.
- **`h264-config`** (4): `format` 0: an `avcC` record
  (AVCDecoderConfigurationRecord, ISO/IEC 14496-15 §5.3.3.1); `format` 1:
  Annex B NAL units containing at least one SPS and one PPS. Optional: if
  absent, the SPS/PPS of the first keyframe (in band, as hardware encoders
  emit them before every IDR) are used. Sending the same configuration
  again is fine; a different one after streaming started is an error
  (receivers were set up with the first). `pts_us` is ignored.
- **`control`** (5): `flags` bit 0 (`0x01`): the game reads keyframe
  requests on its end of tvpipe's stdout (below). Other bits are reserved.
  May appear anywhere; the last one wins. `pts_us` is ignored.

Payload limits: 64 MiB per `h264` record, 1 MiB for `h264-config`, `len`
exactly 4 for `control`.

## Keyframe back channel

When a receiver needs a keyframe (a sink joining, a queue overflow, a PLI
or loss on Cast) and the game enabled the channel with a `control` record,
tvpipe writes the ASCII line

```
keyframe\n
```

to **its stdout**. The game spawned tvpipe and already holds both ends of
its stdio, so this needs no port, no fd passing and no platform code;
tvpipe writes nothing else to stdout in `play` (logs go to stderr).

- Requests are coalesced and rate-limited (at most one per 200 ms while a
  receiver keeps asking); the game should make its next frame an IDR.
- A game that does not read stdout must not enable the channel: the line
  would end up wherever stdout points (a full pipe only stalls the writer
  thread, never the stream).
- Lines are newline-terminated words so that more requests (e.g.
  `bitrate 4000`) can be added later; readers ignore lines they do not
  know.

## Timeline

The first kept record (audio, or the first keyframe; non-key video before
it is dropped) defines media time 0. Video pts = `pts_us - origin` in
90 kHz ticks; a pts that does not increase is bumped by one tick with a
warning. Audio is timed by its sample count from its first record's
`pts_us`, and resyncs when the game's `pts_us` drifts more than 50 ms from
the count (a real gap).

## Audio

PCM goes directly to receivers that take PCM (AirPlay, sent as verbatim
ALAC; the browser player), cut into the frame size the session asks for.
Cast needs Opus or AAC, so that family is encoded in-process with the
built-in AAC-LC encoder.

## Platform notes

Rust does no newline translation on stdio, so SLRW is binary-safe on every
OS. On Windows, pipe it from `cmd.exe`, PowerShell 7.4+, a file, or a
program that spawns tvpipe; Windows PowerShell 5.1 re-encodes pipes
between native programs as text and corrupts the stream
(`slrw: unknown record kind 10`).
