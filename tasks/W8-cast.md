# W8 Cast control channel + Cast Streaming (M8–M9)

Status: done except real hardware (none on the LAN). Owns `src/cast/**`,
`tests/fixtures/cast/**`, `tests/vectors/cast/**`, `scripts/receivers/cast*`.

Builds on: `crypto::{aes, modes}` (CTR), `crypto::ring` (rng),
`codec::{json, protobuf}` (CastMessage + `encode_framed`), `net::discover`
(merged); `media::*` (W4, landing in pieces) only for the sink glue.

- [x] `channel.rs`: TLS to port 8009 with rustls (ring provider, a custom
      `ServerCertVerifier` that accepts any cert but still checks the
      handshake signatures), `u32be` length-prefixed `CastMessage` framing
      with a size limit, a reader thread, PING/PONG heartbeat every 5 s,
      CONNECT/CLOSE virtual connections.
- [x] `messages.rs`: typed builders/parsers for CONNECT, PING/PONG, LAUNCH,
      GET_STATUS, STOP, RECEIVER_STATUS, OFFER, ANSWER, and errors, using
      `codec::json`. Launch the mirroring app `0F5096E8`, find its
      `transportId`, CONNECT to it.
- [x] `streaming.rs`: OFFER construction (per PLAN §8.2: video h264 + audio
      opus/aac streams, `rtpProfile: cast`, payload types, SSRCs, random
      `aesKey`/`aesIvMask`, `targetDelay`, `timeBase`, max fps/bitrate,
      resolutions) and ANSWER parsing (udpPort, sendIndexes, ssrcs,
      constraints, display) → `types::Capabilities`.
- [x] `rtp.rs`: Cast RTP packetization with its header extensions
      (keyframe flag, frame id, packet id, max packet id, reference frame
      id, playout-delay extension), ≤~1400-byte packets, whole-frame
      AES-128-CTR with the nonce from `aesIvMask` and frame id. Byte-exact
      tests against openscreen's unit-test fixtures.
- [x] `rtcp.rs`: Sender Reports (RTP↔NTP), Receiver Reports, Cast feedback
      (checkpoint ACK, NACK bitmasks, target-delay changes), PLI, receiver
      log (parse/ignore). Fixture tests from openscreen.
- [x] `retransmit.rs`: per-stream packet history over the target-delay
      window, NACK resends, max-frames-in-flight throttling.
- [x] `sink.rs`: `StreamingSink: types::Sink` plus a
      `cast::connect(device/addr) -> StreamingSink` that does
      connect+launch+OFFER/ANSWER; one sender thread per socket role.
- [x] Test receiver: openscreen's standalone `cast_receiver` builds here
      (`scripts/receivers/cast-receiver.sh build|build-av|run`), plus an
      in-test fake receiver (`src/cast/fake_receiver.rs`: TLS +
      OFFER→ANSWER + RTP reassembly/decrypt + Cast RTCP, injectable loss).

Done when: video (then audio) plays on openscreen's `cast_receiver`; stream
survives 2% loss (a lossy UDP relay in tests is fine); later, real Cast
hardware (none on the LAN right now; the human may provide one).

Results (2026-11-17, macOS, openscreen 4ec5cc77):

- Headless `cast_receiver`: 5 s of 1280×720@30 H.264 + 20 ms Opus, all
  150 video and 248 audio frames played out, each ~340 ms after capture at
  a 400 ms target delay.
- 10 s through a relay dropping 2% each way: all 300 video and 496 audio
  frames played out (58 video / 9 audio packets retransmitted, 0 frames
  dropped).
- `build-av` (ffmpeg + SDL2) decodes the H.264 and Opus without errors.
  It logs one "playout drift ~100 ms, re-synchronizing" per stream early
  in the session; likely the receiver reading the first Sender Report late
  while it initialises SDL (its clock-offset estimate then settles).
- `cargo test`: the fake receiver checks byte-exact delivery of every
  frame with 0% and 5% loss, PLI handling, LAUNCH_ERROR, and receiver
  disconnect.

How to run the receiver loop:

```sh
scripts/receivers/cast-receiver.sh build      # once; ~3 GB of deps
scripts/receivers/cast-receiver.sh run        # terminal 1
TVPIPE_CAST_SECS=10 TVPIPE_CAST_LOSS=0.02 \
    cargo test cast::receiver_loop -- --ignored --nocapture   # terminal 2
```

Open items / for the lead:

- Wiring: see `docs/decisions/0003-cast-sink.md` (API for `session.rs`,
  private H.264/clock helpers that duplicate W4, AAC framing question).
- Real hardware (Chromecast / Google TV): untested. Things to watch:
  device-auth not done; Android-TV payload-type hack is
  `CastOptions::android_rtp_hack`; AAC raw vs ADTS.
