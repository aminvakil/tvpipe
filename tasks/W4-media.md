# W4 Media

Status: in progress. Owns `src/media/**`, `examples/slrw-gen.rs`,
`tests/fixtures/media/**`.

- [ ] `ffmpeg.rs`: locate ffmpeg (`--ffmpeg`, `$STREAMLIB_FFMPEG`, PATH),
      `-encoders` probe, argv builder from `EncodePlan` + input spec (PLAN
      §5.1, per-encoder low-latency flags), spawn with stdout pipe, stderr
      pump thread into the logger, kill/wait on drop.
- [ ] `mkv.rs`: live EBML/Matroska demuxer over `Read`: unknown-size
      Segment/Cluster, Tracks (CodecID, CodecPrivate, video/audio params,
      TrackTimestampScale/TimestampScale), SimpleBlock and BlockGroup
      (no lacing needed for our encodes, but parse Xiph/EBML/fixed lacing
      or reject cleanly), keyframe flag, timestamps → `EncodedFrame` in
      90 kHz / sample ticks. Element size limits. Never panics.
- [ ] `h264.rs`: NAL iteration (AVCC and Annex B), avcC parse/build, SPS
      parse (profile, level, dimensions incl. cropping, VUI fps if present),
      IDR detection.
- [ ] `rawin.rs` + `examples/slrw-gen.rs`: SLRW framing (PLAN §5.2), two
      localhost TCP listeners feeding ffmpeg rawvideo/s16le inputs; sample
      generator that writes moving color bars + a sine tone.
- [ ] `clock.rs`: media clock mapping first pts to `Instant`.
- [ ] `fanout.rs`: bounded per-sink queues + one thread per sink;
      keyframe-skip when full or when the sink `wants_keyframe()`.
- [ ] Wire `streamlib debug-pipeline` in `session.rs` (you may add
      `fn debug_pipeline` there): prints pts/keyframe/size per frame and
      stream info.

Done when: `debug-pipeline` output matches `ffprobe -show_packets` for
ffmpeg-generated test files (write a script under `scripts/` that checks
it), and SLRW works end to end with the generator.

Sources allowed: Matroska/EBML specs (RFC 8794, matroska.org), ITU-T H.264,
ISO/IEC 14496-15 (avcC), ffmpeg docs and black-box behaviour.
