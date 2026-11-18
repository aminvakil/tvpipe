# 0007: AirPlay mirror sink API and integration notes (W7)

Status: proposal for the lead. No frozen interface changes.

## API for `session.rs`

```rust
use crate::airplay::session::MirrorOptions;
use crate::airplay::sink::connect_mirror;

let opts = MirrorOptions {
    latency_ms: opts.latency_ms, // --latency; None = normal profile (75/85 ms)
    audio,                       // whether the encode carries audio
    ..Default::default()         // name "tvpipe", PTP allowed
};
let sink = connect_mirror(&airplay::pair::Target { addr, device }, &opts)?;
```

- `connect_mirror` pair-verifies (stored credentials, else transient),
  runs FairPlay, SETUP #1 (or media-first), the event channel, the `/info`
  refresh, the early RECORD and the audio SETUP, so `capabilities()` is
  final: `max_width/height` = the receiver's H.264 canvas (Apple TV:
  1280×720), `max_fps` = its `maxFPS`, `audio_formats` = ALAC 44.1 kHz
  stereo (empty when the receiver gave no audio ports or `audio` is off),
  `max_bitrate_kbps` = 12000. PTP failures retry once with NTP on a fresh
  connection (spec §2.5).
- `start(video, audio)`: H.264 only; audio must be `PcmS16le`, 44.1 kHz,
  ≤ 352 samples per frame (the session plans exactly that for
  `AudioFamily::AirPlay` + ALAC and wraps it as verbatim ALAC). Runs the
  video SETUP, connects the data channel, sends a pending RECORD and the
  initial volume when the policy says so, starts `/feedback`.
- `push` never blocks on the network for video (a writer thread owns the
  data socket; `Dropped` + `wants_keyframe()` when 32 packets are queued).
  Audio is sent from `push` (UDP). `Finished` once the receiver closes the
  video connection.
- `stop` drains the writer queue, sends TEARDOWN and closes everything;
  idempotent; also on drop.

## session.rs changes (made in this branch)

- `Target::AirPlay` carries the control address and the discovered device
  (for the credentials lookup), so names are not browsed twice.
- `audio_plan`: AirPlay + ALAC → `PcmS16le`, 352 samples, no bitrate.
- `plan`: GOP `ceil(fps)/2` (rounded up) when an AirPlay sink is present
  (spec §6.6: an IDR at least every ~0.5 s).
- The "not implemented yet (M6)" rejection is gone.

## Known gaps

- Legacy raw pairing (spec Appendix A, `airplay::legacy_pair`) and the
  legacy media profile (plaintext control/events, AES-CTR video, AES-CBC
  audio, NTP) are implemented and loop-tested only: **untested on
  hardware**. `connect_mirror` returns `AnyMirrorSink` (HAP or legacy).
  HAP stays the default: stored HAP pairings and non-candidates go through
  `pair::connect` unchanged; only unpaired candidates (no HAP pairing bit,
  or third-party bit 26/51 in the TXT features) try raw pair-setup first
  and fall back to HAP. Raw pairings are stored with `pairing = raw`
  (older credential files load unchanged). A legacy session without
  FairPlay sends random `shk`/`shiv`, keys the AES-CTR video from that
  `shk` and sends audio in the clear (spec §5.2; untested on any
  receiver, logged as such).
- Transient pair-setup (W5's `pairing.rs`, minimal W7 change): HKP 3 first
  for legacy-preferring receivers (from discovery), HKP 4 otherwise, one
  retry with the other value on an M1 rejection, one retry with the empty
  SRP password on an M4 authentication failure (spec Appendix A).
- No MediaDataControl channel, no HTTP Digest (password receivers), no
  AAC-ELD, no HEVC. `forceKeyFrame` makes `wants_keyframe()` true until
  the next IDR (the fanout skips to it; ffmpeg cannot be asked for one).
- The audio and video SETUPs are split by the encoder start-up (~0.1 s
  here): connect does SETUP #2, `start` does SETUP #3.
