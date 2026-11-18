# 0006: Built-in encoders and ffmpeg-free sources

Status: accepted by the human (2026-11-18: "A+B makes sense", and C).
See `docs/ffmpeg-optional.md`.

- A: passthrough of compatible inputs with our own demuxers (MKV exists,
  MP4 new); ffmpeg remains the fallback.
- B: SLRW gains a record kind for already-encoded H.264 from the game.
- C: hand-rolled encoders: AAC-LC (audio for Cast) and a baseline H.264
  encoder (raw game frames without ffmpeg, on-demand IDR).

Interface change (lead-approved): `types::Encoder` gains a `Builtin`
variant (our H.264 encoder) once W11 lands; until then the encoders are
leaf modules with their own APIs (below) and are wired by the lead.
The source side gets a `media::input` abstraction (W9) so `session.rs`
can take frames from ffmpeg, a passthrough demuxer, or SLRW + built-in
encoders alike.

Landed (builtin-wiring): `types::Encoder::Builtin` (the only change to
the frozen types; `ffmpeg.rs` treats it like `Auto`, i.e. never hands it
to ffmpeg) and `--encoder builtin`. Policy: raw SLRW input (`-` with a
pixel format) and `test-pattern` use `media::builtin` (convert + scale +
`h264enc`, keyframes on demand; audio as in 0008: PCM for AirPlay,
`aacenc` for Cast) with `--encoder builtin`, and with `--encoder auto`
when ffmpeg cannot be found (the test pattern also when ffmpeg lacks
`drawtext`); otherwise ffmpeg as before. Files, URLs and capture with
`--encoder builtin` are a usage error. The built-in test pattern burns
in the UTC wallclock in milliseconds and a frame counter, with a 1 kHz
beep and a white flash for the first 100 ms of every second.

Clean room for C: ITU-T H.264 and ISO/IEC 14496-3 (and public papers,
textbooks, Wikipedia) are fine; permissive code (openh264 BSD-2) may be
read with attribution. Do **not** read copyleft encoders: x264 (GPL),
ffmpeg's encoders (LGPL/GPL), FAAC (LGPL), fdk-aac (restrictive). ffmpeg
and ffprobe may be *run* as decoding oracles.
