# Making ffmpeg optional

Status: A and B decided and implemented by W9 (decision 0006,
`media::{mp4,passthrough,input}`, decision 0008); experiment results
below.

Today every session runs ffmpeg (PLAN §5). The question is when we can skip
it, also when the input format doesn't match what a receiver takes, and
what that would cost.

## What the receivers take

| | Video | Audio |
|---|---|---|
| AirPlay mirroring (spec) | H.264 (HEVC partly specified) | ALAC (incl. *verbatim*, i.e. PCM in an ALAC frame), AAC-ELD/AAC-LC per the format mask |
| Cast Streaming (openscreen `constants.h`) | H.264, VP8, VP9, HEVC, AV1 | AAC, Opus only (no PCM) |

Both mirroring protocols want low-latency streams: one timestamp per frame
and frames sent in presentation order, so in practice **no B-frames**.

## Where ffmpeg is doing real work

1. **Demuxing** files (MP4, MKV, TS, …). Easy to replace for the
   common containers: we already have a Matroska demuxer; MP4/MOV is a
   moderate hand-rolled job (boxes, `stsz`/`stco`/`stts`/`ctts`, fragmented
   MP4).
2. **Decoding + re-encoding video** when the codec, B-frames, size or frame
   rate don't fit. Not replaceable in-house: a decoder for H.264/HEVC/VP9 is
   far too big to hand-roll.
3. **Encoding raw game frames** (SLRW). Replaceable only with an encoder.
4. **Audio**: resampling and encoding to Opus/AAC for Cast; ALAC for AirPlay
   is already "free" (verbatim ALAC from PCM, confirmed on the Apple TV).
5. **Capture** (`-f avfoundation`, `gdigrab`, …): platform code we don't
   want in streamlib anyway.

## Options

### A. Passthrough when the input already fits (cheap, within the rules)

Demux ourselves and send the encoded frames unchanged when the input is
H.264 without B-frames (check: no `ctts` / pts == dts, or SPS
`max_num_reorder_frames == 0`), at a size/fps the receiver takes, with
AAC/Opus audio (or PCM/ALAC for AirPlay). This covers screen recordings,
OBS/ffmpeg low-latency exports, many camera files, and anything we produced
ourselves. Fall back to ffmpeg otherwise. Work: MP4 demuxer, a
compatibility check, and a "source" abstraction that isn't tied to ffmpeg's
stdout. Benefit: no encode latency, no quality loss, no CPU.

Experiments worth running before deciding how strict to be:
- Do the Apple TV / openscreen receiver play H.264 **with** B-frames if we
  send frames in decode order with correct timestamps? If yes, passthrough
  covers most files.
- Does the Apple TV scale a stream larger than its advertised canvas (it
  reports a 4K HDR capability)?
- AAC-LC passthrough into AirPlay mirroring audio (the spec lists formats;
  untested).

### B. Games bring their own encoder (cheap, within the rules)

Add an SLRW record kind for **already-encoded H.264** (AVCC or Annex B,
with a keyframe flag) next to raw video. Engines usually have hardware
encoders (NVENC, AMF, VideoToolbox, MF) available and can encode on the
GPU with lower latency than piping raw RGBA through ffmpeg. Audio stays
PCM: AirPlay needs no encoder (verbatim ALAC); Cast still needs Opus/AAC
(see C). A side channel back to the game ("keyframe please") gives
on-demand IDR, which ffmpeg as a subprocess can't (PLAN §5.1).

### C. Hand-rolled encoders (in the spirit of the project, large)

- **Audio for Cast**: a simple AAC-LC encoder (MDCT, fixed bit allocation,
  no psychoacoustic model, ~1.5–3k lines) or an Opus CELT-only encoder
  (harder). At 128–256 kbit/s a naive AAC-LC is acceptable. This would make
  the PCM path ffmpeg-free for both protocols.
- **Video**: a minimal H.264 encoder (I16x16/I4x4 intra, P16x16 + skip with
  a small motion search, CAVLC, fixed QP or simple rate control). Several
  thousand lines; much worse compression than x264 and CPU-heavy at
  1080p60 without SIMD, but on-demand IDR, zero subprocess latency and good
  enough for game content at a generous bitrate on a LAN. Intra-only
  (every frame IDR) is simpler but needs very high bitrates.

### D. Native encoders/decoders (fast, but against current rules)

- OS media APIs via FFI (VideoToolbox, Media Foundation, VAAPI/NVENC):
  best latency and quality, but breaks "platform-neutral, no OS media
  APIs" and means three code paths.
- A crate such as `openh264` (BSD-2, builds Cisco's C++ with `cc`; on-demand
  IDR, real-time presets) or Opus via `audiopus`/libopus: would need a
  `docs/deps.md` proposal and a C toolchain (we already need one for
  `ring`).

### E. Cast "remoting" mode (passthrough of almost anything on Cast)

Cast Streaming also has a remoting mode (`CastMode::kRemoting`) in which the
receiver runs a full media pipeline and the sender streams the file's own
encoded frames (any codec the device decodes, with B-frames) plus RPC
messages. That would make passthrough of most files possible on Cast
without re-encoding, at the cost of higher latency than mirroring and a
second, larger protocol (openscreen `cast/streaming/remoting*`, BSD).

## Experiments (W9)

Run with `scripts/receivers/cast-receiver.sh run` (openscreen's
`cast_receiver`, the ffmpeg + SDL build) and

```sh
STREAMLIB_PT_KIND=bframes STREAMLIB_PT_ORDER=dts \
    cargo test media::input::experiments -- --ignored --nocapture
```

(`src/media/input.rs`, `mod experiments`: 1280x720@30 x264 files made by
ffmpeg, sent unchanged through `cast::StreamingSink`, frames paced in
decode order; the verdict comes from the receiver's log.)

| File | Timestamps sent | Result on openscreen `cast_receiver` |
|---|---|---|
| no B-frames | pts | all 180/240 frames consumed and decoded; no warnings, also at 120 ms playout delay |
| B-frames (`-bf 3`, pyramid, reorder depth 2) | pts (presentation) | **broken**: our Cast sink keeps RTP timestamps increasing and its in-flight window works on reference times, so frames going back in time trip `MaxDurationInFlight`; the sink then waits for the next keyframe (59 of 180 frames dropped) |
| B-frames | dts (decode time) | all frames consumed, no decode errors or drift warnings, also at 120 ms playout delay (play-out margin at consume 47 ms vs 84 ms without B-frames) |
| no B-frames + AAC-LC (only AAC offered; local patch, receivers prefer Opus) | raw AAC access units (MP4) | video fine; **every AAC frame fails** (`avcodec_send_packet: Invalid data`): the standalone receiver has no AudioSpecificConfig and runs ffmpeg's parser, which needs ADTS |
| same | AAC wrapped in ADTS | audio decodes and plays, no errors |
| MKV, H.264 + Opus | pts | video and audio decode, no errors |

Why "dts" works there: openscreen's decoder (`decoder.cc`) assigns decoded
pictures to sent frames first-in-first-out, so the n-th picture in
presentation order plays at the n-th frame's timestamp. With decode-time
stamps that is exactly the right cadence, delayed by the reorder depth;
the last frames of the stream stay in the decoder. A receiver that maps
output pictures by their own timestamps (Chrome's pipeline, and probably
the Apple TV) would show B-frames at their decode time instead, i.e. out
of order. So the openscreen result does **not** show that B-frames are
safe; it only shows the receiver does not reject them.

Decisions taken in the code (W9):

- Passthrough refuses B-frames (`passthrough::check`; detected from the
  MP4 sample table, the first seconds of other files, or the SPS's
  `max_num_reorder_frames`). `CheckOptions::allow_reordering` exists for
  hardware experiments (Chromecast, Apple TV once W7 merges).
- Passthrough refuses keyframe intervals over 4 s: one
  `MaxDurationInFlight` drop at session start (the receiver not ACKing
  yet) cost a whole 2 s GOP in several runs; the file's GOP is what
  recovery waits for.
- AAC passthrough to Cast needs (a) a way to offer AAC first or only
  (`CastOptions`, W8; openscreen picks Opus otherwise) and (b) knowing
  whether real receivers want raw AAC or ADTS. Until then AAC files go
  through ffmpeg for Cast (Opus); AAC for AirPlay is untested (W7).

Follow-up (W9, after the wiring):

- The Cast sink now sends AAC ADTS-framed, as Chromium's Cast sender does
  (`media/cast/encoding/audio_encoder.cc`), and `CastOptions` can offer
  AAC alone. A file with AAC that could pass is offered AAC only; an
  encoding game's PCM goes through `media::aacenc` to Cast. On openscreen
  both play without decode errors (`slrw-gen --encoded --exec streamlib
  play --ffmpeg /bogus ...`: 180 video and 281 AAC frames, 3 keyframe
  requests honoured).
- B-frames on hardware: `STREAMLIB_EXPERIMENT_BFRAMES=dts` (or `pts`)
  makes `play FILE` pass files with B-frames through in decode order,
  stamped with decode (or presentation) times. Unset, they go through
  ffmpeg.
- AAC-LC into AirPlay is not possible yet: the AirPlay sink sets up ALAC
  (verbatim, from 44.1 kHz PCM) or AAC-ELD (not implemented), so AirPlay
  passthrough carries audio only for 44.1 kHz s16le PCM files.

Still open (hardware): B-frames on the Apple TV and a Chromecast; scaling
of a stream larger than the advertised canvas.

## Suggested order

1. A (passthrough + MP4 demuxer) and B (encoded SLRW): small, keep ffmpeg
   as the fallback, and immediately make "already-compatible" inputs and
   engine-encoded games ffmpeg-free.
2. Run the B-frame/scaling/AAC experiments to see how far A reaches.
3. C-audio (AAC-LC encoder) if PCM→Cast without ffmpeg matters.
4. Decide between C-video and D only if raw-frame games without their own
   encoder are an important use case.
