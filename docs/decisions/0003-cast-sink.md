# 0003: Cast sink API and integration notes (W8)

Status: proposal for the lead. No frozen interface changes.

## API for `session.rs`

```rust
use crate::cast::{connect, connect_device, CastOptions, StreamingSink};

let opts = CastOptions {
    latency_ms,                 // --latency (target playout delay)
    max_width, max_height,      // --size, or the plan's ceiling
    max_fps,                    // --fps
    max_bitrate_kbps,           // --bitrate
    audio: !no_audio,           // offers Opus, then AAC
    ..CastOptions::default()    // android_rtp_hack, burst_bitrate, timeouts
};
let sink: StreamingSink = connect_device(&device, &opts)?; // or connect(addr, name, &opts)
let caps = sink.capabilities(); // after OFFER/ANSWER
```

- `connect` does TLS + CONNECT + LAUNCH (`0F5096E8`) + CONNECT to the app
  transport + OFFER/ANSWER before returning, so capabilities are final
  (`max_width/height/fps/bitrate` = our offer clamped by the receiver's
  constraints; `audio_formats` = the one audio stream the receiver picked;
  `audio_family = Cast`; `preferred_latency_ms` = the offered target delay).
- `start(video, audio)` requires `VideoCodec::H264` and the negotiated audio
  codec (Opus or AAC-LC); `audio = None` streams video only.
- `push` never blocks on the network; `Dropped` on in-flight limits (then
  video waits for the next key frame: `wants_keyframe()` is true), or for
  audio of the other family. `Finished` once the receiver ends the session.
- `stop` sends STOP + CLOSE and joins all threads; idempotent; also on drop.

Because the receiver only answers after the OFFER, the OFFER is built from
the user's options, not from the final encode plan. If the plan ends up
smaller (another sink limits it), that should be fine: the offer only caps
resolution/bitrate (openscreen's receiver takes any size within it). If the plan needs a different
audio codec than the one negotiated, `start` fails.

## Duplication to resolve after W4 lands

Resolved on branch play-cast: `annexb` parses with `media::h264`; the
sink's clock is a `media::clock::MediaClock` (with the rule below kept).

- `cast::annexb` converts AVCC → Annex B and prepends SPS/PPS from the
  `avcC` on key frames (Cast receivers decode Annex B). If `media::h264`
  grows an equivalent, the cast module can switch to it.
- `cast::sink::MediaClock` anchors the first pushed frame's pts to its
  arrival `Instant` and re-anchors when frames are >100 ms late. If
  `media::clock` provides a shared session clock (needed for replication
  lip-sync across sinks anyway), the sink should take reference instants
  from it instead; the change is local to `push`.
- `cast::receiver_loop` parses ffmpeg's Annex B and Ogg Opus output for the
  opt-in loop test only; it can switch to the mkv demuxer later.

## Open questions

1. AAC: we pass raw AAC access units (no ADTS), like Chrome's sender. The
   openscreen receiver prefers Opus, so AAC is untested. Real devices may
   differ.
2. Device authentication (`tp.deviceauth`) is not implemented; Chromecasts
   do not require it from senders.
3. The Android TV payload-type hack (127/96) is off by default (openscreen's
   standalone sender default). If some TV needs it, expose it as a CLI flag
   or a per-model quirk.
