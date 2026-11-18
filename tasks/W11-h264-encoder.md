# W11 Built-in H.264 encoder (option C, video)

Status: in progress. Owns `src/media/h264enc/**`, `src/media/convert.rs`
(new), `tests/vectors/h264enc/**`.

API (leaf module):
`h264enc::Encoder::new(Config { width, height, fps, bitrate_kbps, gop,
threads }) -> Result`, `encode(&mut self, frame: &Yuv420Frame, force_idr:
bool) -> Result<Encoded { data: Vec<u8> /* AVCC */, keyframe: bool }>`,
`avcc() -> Vec<u8>`, `request_keyframe()`. `convert.rs`: RGBA/BGRA/RGB24/
NV12 → I420 (BT.709 limited range, configurable), and a bilinear scaler.

- [ ] Constrained Baseline profile (works on both Apple TV and Cast), CAVLC,
      one or more slices per frame (slices = threads, std::thread), SPS/PPS
      with VUI (timing, colour), IDR + P frames, no B-frames.
- [ ] Intra: I16x16 (4 modes) and I4x4 (9 modes) with SAD/SATD decision;
      chroma intra (4 modes). Integer transform, quantization (deadzone),
      dequant/reconstruction for the reference.
- [ ] Inter: P16x16 (and optionally 16x8/8x16/8x8) with a small motion
      search (diamond/hex + predicted MV), quarter-pel optional (start
      full-pel; half/quarter-pel interpolation per the standard if time
      allows), P_Skip, deblocking filter (needed for quality; must match
      the decoder bit-exactly).
- [ ] Rate control: per-frame QP from a simple model targeting
      bitrate_kbps with a small VBV so frames stay ≤ ~2× average (low
      latency); `force_idr` anytime.
- [ ] Tests: decode with ffmpeg (oracle) and require bit-exact
      reconstruction (our recon == ffmpeg's decoded frames) plus PSNR
      targets on synthetic and natural-ish content; ffprobe accepts the
      stream; bitrate within ±15%.
- [ ] Speed: report fps at 720p and 1080p in release; goal 720p60 /
      1080p30 real time on this machine with threads.

Clean room: ITU-T H.264, public papers/textbooks; openh264 (BSD-2) may be
read with attribution; no x264 or ffmpeg encoder source (decision 0006).
