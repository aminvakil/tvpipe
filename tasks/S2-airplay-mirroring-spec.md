# S2 AirPlay mirroring spec (dirty room)

Status: in progress. Owns `docs/spec/airplay-mirroring.md`,
`docs/spec/data/**`, `tests/vectors/airplay-mirror/**`. **Never writes in
`src/`.**

Write a functional spec, from doubletake (and, where useful, UxPlay as a
receiver-side cross-check), detailed enough that a clean implementer can
build `airplay/{mirror,audio,timing,sink}.rs` (PLAN §7.4–7.5) without ever
seeing the source:

- Session flow after pair-verify and FairPlay: every RTSP request in order
  (method, URI, headers incl. `X-Apple-*`, Content-Type), the bplist body
  keys/types/values for SETUP #1 (session) and SETUP #2 (streams type 110
  and 96), RECORD, SET_PARAMETER volume, `/feedback`, TEARDOWN bodies;
  which response keys matter and what they mean.
- How the receiver's display size / supported audio formats are learned
  (our `/info` from the Apple TV has neither `displays` nor `audioFormats`).
- Video data channel: the 128-byte header field by field (offsets, widths,
  endianness, meaning), payload types (codec/avcC, video, heartbeat),
  timestamp format, heartbeat cadence, what is encrypted and how the
  AES-CTR keystream continues across packets. Which key material comes
  from FairPlay (that module is an LGPL port, decision 0005; describe the
  interface, not FairPlay internals).
- Timing: PTP anchoring via `X-Apple-RequestReceivedTimestamp` (exact
  computation), the NTP responder for legacy receivers (packet formats).
- Realtime audio: RTP header, payload format for ALAC (incl. wrapping
  PCM as uncompressed ALAC frames: frame layout bit by bit), encryption
  (AES-CBC/ChaCha variants, key/IV source), control port, sync packets,
  `spf`/`sr`/`latency` values.
- Receiver quirks per family (Apple TV, Samsung, LG, Roku…), as facts.
- Test vectors: concrete byte examples (bplist bodies as hex, a video
  packet header, an encrypted audio packet with key/IV) under
  `tests/vectors/airplay-mirror/`.

Rules: describe function and facts in prose and tables; no code and no
line-by-line pseudocode transliteration; constants/tables in canonical form
in `docs/spec/data/`. Cite sections of the source only in your report to the
lead, not in the spec (the spec must stand alone). Log your inputs in your
report; the lead updates `docs/cleanroom-log.md`.
