# W4 Media

Status: done (clock, fanout and the fps tweak wait for merge from `w4-media`). Owns `src/media/**`, `examples/slrw-gen.rs`,
`tests/fixtures/media/**`.

- [x] `ffmpeg.rs`: locate ffmpeg (`--ffmpeg`, `$TVPIPE_FFMPEG`, PATH),
      `-encoders` probe, argv builder from `EncodePlan` + input spec (PLAN
      §5.1, per-encoder low-latency flags), spawn with stdout pipe, stderr
      pump thread into the logger, kill/wait on drop.
- [x] `mkv.rs`: live EBML/Matroska demuxer over `Read`: unknown-size
      Segment/Cluster, Tracks (CodecID, CodecPrivate, video/audio params,
      TrackTimestampScale/TimestampScale), SimpleBlock and BlockGroup
      (no lacing needed for our encodes, but parse Xiph/EBML/fixed lacing
      or reject cleanly), keyframe flag, timestamps → `EncodedFrame` in
      90 kHz / sample ticks. Element size limits. Never panics.
- [x] `h264.rs`: NAL iteration (AVCC and Annex B), avcC parse/build, SPS
      parse (profile, level, dimensions incl. cropping, VUI fps if present),
      IDR detection.
- [x] `rawin.rs` + `examples/slrw-gen.rs`: SLRW framing (PLAN §5.2), two
      localhost TCP listeners feeding ffmpeg rawvideo/s16le inputs; sample
      generator that writes moving color bars + a sine tone.
- [x] `clock.rs`: media clock mapping first pts to `Instant`.
- [x] `fanout.rs`: bounded per-sink queues + one thread per sink;
      keyframe-skip when full or when the sink `wants_keyframe()`.
- [x] `tvpipe debug-pipeline`: put the command body in
      `media::debug::run(&PipelineOptions)` in your module (prints stream
      info and pts/keyframe/size per frame); the lead wires it into
      `session.rs` (`Command::DebugPipeline(o) => crate::media::debug::run(&o)`).
      `--raw` = stream copy (input packets as-is), else `debug::default_plan()`.
      `scripts/check-pipeline.sh` checks it against ffprobe and runs SLRW e2e.

Notes for integrators:

- `media::source::MediaReader` turns ffmpeg's Matroska into `StreamInfo`
  (`VideoInfo` + per-family `AudioInfo`) and `EncodedFrame`s; the track
  layout comes from `ffmpeg::Invocation::tracks`.
- ffmpeg's ALAC encoder always emits 4096-sample frames; `ffmpeg::build`
  rejects ALAC plans with other frame sizes. For AirPlay's 352-sample
  frames plan `AudioCodec::PcmS16le` (exact 352-sample packets) and frame
  it as uncompressed ALAC in the sink.
- SLRW byte order: docs/slrw.md (little-endian).
- Measured (M3): 720p30 SLRW → x264 zerolatency → demuxed frame ≈ 20 ms
  for the first frame on an M-series Mac (release build).

Done when: `debug-pipeline` output matches `ffprobe -show_packets` for
ffmpeg-generated test files (write a script under `scripts/` that checks
it), and SLRW works end to end with the generator.

Sources allowed: Matroska/EBML specs (RFC 8794, matroska.org), ITU-T H.264,
ISO/IEC 14496-15 (avcC), ffmpeg docs and black-box behaviour.
