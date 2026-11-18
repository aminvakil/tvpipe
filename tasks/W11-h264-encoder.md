# W11 Built-in H.264 encoder (option C, video)

Status: core done on branch `w11-h264enc` (not merged; lead integrates).
Owns `src/media/h264enc/**`, `src/media/convert.rs` (new),
`tests/vectors/h264enc/**`.

API (leaf module):
`h264enc::Encoder::new(Config { width, height, fps, bitrate_kbps, gop,
threads }) -> Result`, `encode(&mut self, frame: &Yuv420Frame, force_idr:
bool) -> Result<Encoded { data: Vec<u8> /* AVCC */, keyframe: bool }>`,
`avcc() -> Vec<u8>`, `request_keyframe()`. `convert.rs`: RGBA/BGRA/RGB24/
NV12 → I420 (BT.709 limited range, configurable), and a bilinear scaler.

As built: `Config` also has `color` (VUI colour description, default
BT.709 limited), `qp: Option<u8>` (fixed QP instead of rate control) and
`inline_parameter_sets` (SPS/PPS in front of each IDR); use
`..Config::default()`. `Encoded` also carries the frame's `qp`. Extra:
`parameter_sets()`, `set_bitrate()`, `recon()` (cropped reconstruction),
`last_stats()`. `media::convert::{Yuv420Frame, PixelFormat, ColorSpec,
Matrix, to_i420, scale}`; `PixelFormat::from_name` takes ffmpeg names.
`threads: 0` = automatic (≤ 8, ≥ 4 MB rows per slice).

- [x] Constrained Baseline profile (works on both Apple TV and Cast), CAVLC,
      one or more slices per frame (slices = threads, std::thread), SPS/PPS
      with VUI (timing, colour), IDR + P frames, no B-frames.
      (VUI also: bitstream_restriction with max_num_reorder_frames 0,
      max_dec_frame_buffering 1; POC type 2; level picked from Table A-1.)
- [x] Intra: I16x16 (4 modes) and I4x4 (9 modes) with SAD/SATD decision;
      chroma intra (4 modes). Integer transform, quantization (deadzone),
      dequant/reconstruction for the reference.
- [x] Inter: P16x16 (and optionally 16x8/8x16/8x8) with a small motion
      search (diamond/hex + predicted MV), quarter-pel optional (start
      full-pel; half/quarter-pel interpolation per the standard if time
      allows), P_Skip, deblocking filter (needed for quality; must match
      the decoder bit-exactly).
      (P16x16 only; diamond search from mvp/0/skip/neighbour candidates,
      half then quarter-sample refinement with SATD; intra MBs in P
      frames; 16x8/8x16/8x8 not done.)
- [x] Rate control: per-frame QP from a simple model targeting
      bitrate_kbps with a small VBV so frames stay ≤ ~2× average (low
      latency); `force_idr` anytime.
      (P frames over 2× the average are re-encoded at a higher QP, up to
      twice; IDRs get 2.5× with a 4× cap.)
- [x] Tests: decode with ffmpeg (oracle) and require bit-exact
      reconstruction (our recon == ffmpeg's decoded frames) plus PSNR
      targets on synthetic and natural-ish content; ffprobe accepts the
      stream; bitrate within ±15%.
      (Oracle tests skip without ffmpeg; heavy ones are `#[ignore]`:
      `cargo test --release -- --ignored --nocapture bench_` and
      `oracle_1080p`.)
- [x] Speed: report fps at 720p and 1080p in release; goal 720p60 /
      1080p30 real time on this machine with threads.

Results (M5 Max, release, machine shared with other jobs, synthetic
moving texture): 720p ≈ 95 fps on 1 thread, 430 fps on 8; 1080p ≈ 33–40
fps on 1 thread, 140–180 fps on 8 (≈ 6 ms per frame). Quality (lavfi
720p60, rate control, PSNR-Y): testsrc2 36.9 / 39.9 / 45.4 dB and
mandelbrot 31.7 / 34.1 / 37.5 dB at 2 / 4 / 8 Mbit/s, bitrate within 1%.
For reference x264 `-profile baseline -tune zerolatency` on mandelbrot:
ultrafast 28.8 / 31.2 dB, veryfast 30.6 / 33.3 dB at 2 / 4 Mbit/s; at
fixed QP 30 we match veryfast on mandelbrot and trail it by ~0.3 dB on
testsrc2. Decisions: SATD pre-selection, then SSD + λ·bits between
inter, P_Skip, Intra_16x16 and Intra_4x4 when close.

Left: P16x8/8x16/8x8 partitions (x264 shows little gain from p8x8 on
this content); SIMD-friendlier deblocking and SATD (no `unsafe`/SIMD
yet, autovectorisation only); slices are not emitted before the whole
picture is done; hardware tests on Apple TV and Chromecast; wiring into
`session.rs`/SLRW (lead).

Clean room: ITU-T H.264, public papers/textbooks; openh264 (BSD-2) may be
read with attribution; no x264 or ffmpeg encoder source (decision 0006).
(openh264 was read only for the CAVLC/cbp/deblocking tables.)
