# W9 Passthrough sources + encoded SLRW (options A + B)

Status: in progress. Owns `src/media/{mp4,input,passthrough}.rs` (new),
`src/media/{slrw,rawin}.rs` (extend), `examples/slrw-gen.rs`,
`tests/fixtures/media/**`. Don't edit `session.rs` until W7 has merged; the
lead tells you, then you wire `media::input` in.

- [ ] `mp4.rs`: MP4/MOV demuxer over `Read + Seek` (ftyp, moov/trak/mdia/
      minf/stbl: stsd avc1/avcC, hvc1/hvcC, mp4a/esds, Opus/dOps, alac;
      stts, ctts, stss, stsz, stsc, stco/co64; edit lists for the start
      offset; fragmented MP4 moof/traf/trun). → `EncodedFrame` +
      `VideoInfo`/`AudioInfo`. Never panics; size limits. Fixtures made
      with ffmpeg; compare against `ffprobe -show_packets`.
- [ ] Compatibility check: decide from `VideoInfo`/`AudioInfo` + SPS
      (`max_num_reorder_frames`, pts==dts) + all sinks' `Capabilities`
      whether a file can be sent unchanged; return a precise reason when
      not (so the log says why ffmpeg is used).
- [ ] `input.rs`: a source abstraction yielding stream info + frames, with
      implementations: ffmpeg (existing `media::source`), passthrough file
      (MKV via `mkv.rs`, MP4 via `mp4.rs`, paced in real time with
      `media::clock`, optional loop), SLRW (raw → ffmpeg as today; encoded
      → direct).
- [ ] SLRW: new record kind for encoded H.264 (AVCC or Annex B, keyframe
      flag, optional avcC/SPS+PPS record); a back channel so the game can
      learn "keyframe requested" (design it: e.g. a byte on a localhost TCP
      port or on stdout; keep it std-only), documented in a decision note
      amending 0002. Update `slrw-gen` with an `--encoded` mode (it may use
      ffmpeg to produce H.264 for the example only).
- [ ] Experiments on the receivers, as `#[ignore]` tests or scripts plus a
      short write-up in `docs/ffmpeg-optional.md`: H.264 with B-frames sent
      in decode order (openscreen receiver; Apple TV once W7 merges), and
      AAC-LC passthrough.
