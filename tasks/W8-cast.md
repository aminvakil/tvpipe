# W8 Cast control channel + Cast Streaming (M8–M9)

Status: in progress. Owns `src/cast/**`, `tests/fixtures/cast/**`,
`tests/vectors/cast/**`, `scripts/receivers/cast*`.

Builds on: `crypto::{aes, modes}` (CTR), `crypto::ring` (rng),
`codec::{json, protobuf}` (CastMessage + `encode_framed`), `net::discover`
(merged); `media::*` (W4, landing in pieces) only for the sink glue.

- [ ] `channel.rs`: TLS to port 8009 with rustls (ring provider, a custom
      `ServerCertVerifier` that accepts any cert but still checks the
      handshake signatures), `u32be` length-prefixed `CastMessage` framing
      with a size limit, a reader thread, PING/PONG heartbeat every 5 s,
      CONNECT/CLOSE virtual connections.
- [ ] `messages.rs`: typed builders/parsers for CONNECT, PING/PONG, LAUNCH,
      GET_STATUS, STOP, RECEIVER_STATUS, OFFER, ANSWER, and errors, using
      `codec::json`. Launch the mirroring app `0F5096E8`, find its
      `transportId`, CONNECT to it.
- [ ] `streaming.rs`: OFFER construction (per PLAN §8.2: video h264 + audio
      opus/aac streams, `rtpProfile: cast`, payload types, SSRCs, random
      `aesKey`/`aesIvMask`, `targetDelay`, `timeBase`, max fps/bitrate,
      resolutions) and ANSWER parsing (udpPort, sendIndexes, ssrcs,
      constraints, display) → `types::Capabilities`.
- [ ] `rtp.rs`: Cast RTP packetization with its header extensions
      (keyframe flag, frame id, packet id, max packet id, reference frame
      id, playout-delay extension), ≤~1400-byte packets, whole-frame
      AES-128-CTR with the nonce from `aesIvMask` and frame id. Byte-exact
      tests against openscreen's unit-test fixtures.
- [ ] `rtcp.rs`: Sender Reports (RTP↔NTP), Receiver Reports, Cast feedback
      (checkpoint ACK, NACK bitmasks, target-delay changes), PLI, receiver
      log (parse/ignore). Fixture tests from openscreen.
- [ ] `retransmit.rs`: per-stream packet history over the target-delay
      window, NACK resends, max-frames-in-flight throttling.
- [ ] `sink.rs`: `StreamingSink: types::Sink` plus a
      `cast::connect(device/addr) -> StreamingSink` that does
      connect+launch+OFFER/ANSWER; one sender thread per socket role.
- [ ] Test receiver: try to build openscreen's standalone `cast_receiver`
      (it needs gn/ninja and a dev certificate; it may be heavy). Put the
      build/run steps in `scripts/receivers/cast-receiver.sh`. If building
      it is impractical here, write a small fake receiver in tests
      (TLS + OFFER→ANSWER + RTP/RTCP parse-and-ACK) and report.

Done when: video (then audio) plays on openscreen's `cast_receiver`; stream
survives 2% loss (a lossy UDP relay in tests is fine); later, real Cast
hardware (none on the LAN right now; the human may provide one).

Sources allowed: openscreen (BSD-3; port with attribution:
`cast/streaming/*`, `cast/common/channel/*`, `cast/standalone_sender/*`),
pychromecast/node-castv2/rust_cast (MIT), RFC 3550/4585. Clone openscreen
to `~/.cache/checkouts/` if needed.
