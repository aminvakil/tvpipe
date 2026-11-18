# W9 Passthrough sources + encoded SLRW (options A + B)

Status: done; Apple TV B-frame experiment waits for the human
(`STREAMLIB_EXPERIMENT_BFRAMES`). Owns `src/media/{mp4,input,passthrough}.rs` (new),
`src/media/{slrw,rawin}.rs` (extend), `examples/slrw-gen.rs`,
`tests/fixtures/media/**`. Don't edit `session.rs` until W7 has merged; the
lead tells you, then you wire `media::input` in.

- [x] `mp4.rs`: MP4/MOV demuxer over `Read + Seek` (ftyp, moov/trak/mdia/
      minf/stbl: stsd avc1/avcC, hvc1/hvcC, mp4a/esds, Opus/dOps, alac;
      stts, ctts, stss, stsz, stsc, stco/co64; edit lists for the start
      offset; fragmented MP4 moof/traf/trun). → `EncodedFrame` +
      `VideoInfo`/`AudioInfo`. Never panics; size limits. Fixtures made
      with ffmpeg; compare against `ffprobe -show_packets`.
      (`scripts/gen-mp4-fixtures.sh`, `tests/vectors/mp4/`; also stz2,
      sowt/twos/lpcm, QuickTime `wave`, tkhd matrix.)
- [x] Compatibility check: decide from `VideoInfo`/`AudioInfo` + SPS
      (`max_num_reorder_frames`, pts==dts) + all sinks' `Capabilities`
      whether a file can be sent unchanged; return a precise reason when
      not (so the log says why ffmpeg is used).
      (`passthrough::check`; `h264::Sps::max_num_reorder_frames`, lead-
      approved; also 4:2:0/8-bit/progressive, rotation, cuts, keyframe
      interval.)
- [x] `input.rs`: a source abstraction yielding stream info + frames, with
      implementations: ffmpeg (existing `media::source`), passthrough file
      (MKV via `mkv.rs`, MP4 via `mp4.rs`, paced in real time with
      `media::clock`, optional loop), SLRW (raw → ffmpeg as today; encoded
      → direct).
- [x] SLRW: new record kind for encoded H.264 (AVCC or Annex B, keyframe
      flag, optional avcC/SPS+PPS record); a back channel so the game can
      learn "keyframe requested" (design it: e.g. a byte on a localhost TCP
      port or on stdout; keep it std-only), documented in a decision note
      amending 0002. Update `slrw-gen` with an `--encoded` mode (it may use
      ffmpeg to produce H.264 for the example only).
      (decision 0008: kinds 3-5, `keyframe\n` on stdout;
      `Fanout::keyframe_wanted()`, lead-approved.)
- [~] Experiments on the receivers, as `#[ignore]` tests or scripts plus a
      short write-up in `docs/ffmpeg-optional.md`: H.264 with B-frames sent
      in decode order (openscreen receiver; Apple TV once W7 merges), and
      AAC-LC passthrough. (openscreen done; Apple TV pending W7.)
- [x] Wire `media::input` into `session.rs` (plan below, as done).
- [x] Encoded SLRW → Cast audio with `media::aacenc`; Cast sink sends
      ADTS AAC and can offer AAC only (`CastOptions::audio_codecs`).
- [x] ffmpeg located lazily: passthrough and encoded SLRW run without it
      (session tests with a bogus `--ffmpeg`).

## Session wiring plan (for after W7)

1. Before connecting: if `input::passthrough_candidate(&opts)` is a file,
   run `passthrough::probe(path)` and keep the `Result<FileInfo>`. When it
   succeeded, fill `InputInfo` from it (size, fps, audio, duration) instead
   of ffprobe, and let `cast_options` offer the file's own size and at
   least its bitrate (`video_kbps`, or `peak_video_kbps`), so a 4K/8 Mbit/s
   file is not refused only because we offered less.
2. After `connect_all`: `input::decide_file(&probed, &opts, &sinks)` →
   `Passthrough { info, plan }` → `PassthroughInput::open(path, &plan,
   realtime, looped)`; else `info!("{why}")` and the ffmpeg path.
3. `--raw-video ...:h264`: `encoded_audio_families(raw_audio, &sinks)`
   (log its warnings), then `SlrwEncodedInput::open(stdin, EncodedOptions
   { back_channel: Some(Box::new(std::io::stdout())), .. })`. No ffmpeg,
   no `plan()` (and no ffmpeg lookup: `ffmpeg::locate` becomes lazy).
4. Everything else: `FfmpegInput::start(bin, probe, &source, &plan, raw)`
   (`raw` = `Some((RawInput, Box::new(stdin)))` for raw SLRW).
5. `stream()` takes a `Box<dyn Input>`: `Fanout::new(sinks, input.info())`,
   loop `next_frame()` → `fanout.push`, `if fanout.keyframe_wanted() {
   input.request_keyframe(); }`, then `input.finish(end == Eof &&
   no error)`. The `Watcher`, `ffmpeg_error` and forwarder code in
   `session.rs` go away (they live in `FfmpegInput`).
