# W7 AirPlay mirror + audio + timing (M6–M7)

Status: in progress. Owns `src/airplay/{mirror,audio,timing,sink,session}.rs`
(and any further new files in `src/airplay/` except `fairplay/**`,
`discovery.rs`, `pairing.rs`, `secure.rs`, `credentials.rs`, `info.rs`,
`pair.rs`, which you may call but only change minimally with a note),
`tests/vectors/airplay-mirror/**` (read; add more only in the same format).

Clean room: implement ONLY from `docs/spec/airplay-mirroring.md`,
`docs/spec/data/**`, `tests/vectors/airplay-mirror/**`, the existing
`src/`, and permissive refs (pyatv, RFCs). Questions go to
`docs/spec/QUESTIONS.md` (and to the lead); never look in the tainted
sources. `src/airplay/fairplay/` is an LGPL port: call its public API
only, don't read its internals for protocol details beyond the doc
comments, and don't copy from it.

- [ ] Session setup per the spec: pair-verify (`pair::connect`), FairPlay
      (`fairplay::fp_setup`), SETUP #1, event channel, SETUP #2 (video 110 +
      audio 96), RECORD, volume, `/feedback` keep-alive, `/info` refresh,
      TEARDOWN; timeouts and failure rules exactly as specified.
- [ ] Video data channel: 128-byte headers, codec (avcC) packets,
      ChaCha20-Poly1305 video (our path) and AES-CTR (legacy), heartbeats.
- [ ] Timing: PTP anchoring from response headers, NTP responder (legacy).
- [ ] Audio: RTP, ALAC — from our `PcmS16le` 352-sample frames wrapped as
      verbatim (uncompressed) ALAC (ffmpeg's ALAC encoder can't do 352),
      ChaCha audio on our path, AES-CBC legacy; sync packets; control port;
      RFC 2198 redundancy as the spec says (note: verbatim frames are too
      big for redundancy).
- [ ] `MirrorSink: types::Sink` and `airplay::connect_mirror(...)`, wired by
      you into `session.rs` (`play` currently rejects AirPlay targets with
      "not implemented yet (M6)"; replace that path). You may edit
      `session.rs` for this; keep the Cast path working.
- [ ] Vector tests for every byte layout in the spec's vectors.

Done when: `streamlib play --to "Livingroom (2)" FILE` and
`streamlib test-pattern --to "Livingroom (2)"` show picture and play sound
on the Apple TV (human confirms), Ctrl-C leaves mirroring cleanly; later
Samsung/LG.
