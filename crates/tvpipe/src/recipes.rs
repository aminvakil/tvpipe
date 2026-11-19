//! Recipes: getting frames and sound out of common engines and libraries.
//!
//! None of these crates is a dependency of tvpipe; the snippets show the
//! shape of the code, written against the versions current when this was
//! written (they are not compiled here).
//!
//! # CPU framebuffers
//!
//! - **pixels**: the frame is RGBA8: `Options::video(w, h, fps,
//!   PixelFormat::Rgba)`, then `stream.write_video(pixels.frame(), pts)`.
//! - **softbuffer** and **minifb**: `u32` pixels `0x00RRGGBB`, whose
//!   little-endian bytes are B, G, R, 0: [`PixelFormat::Bgr0`] and
//!   [`Stream::write_video_u32`] (no `unsafe`, no `bytemuck`).
//!
//! # wgpu (and engines built on it)
//!
//! Render to (or copy the surface to) a texture with `COPY_SRC` at the
//! stream size (downscale in a pass: cheaper than encoding a 4K frame),
//! copy it into one of two or three `MAP_READ` staging buffers, and hand
//! tvpipe the mapped buffer from two frames ago, padded rows and all:
//!
//! ```ignore
//! const W: u32 = 1280;
//! const H: u32 = 720;
//! // Rows of a texture copy are padded to 256 bytes.
//! let padded = (W * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
//!     * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
//! let staging: Vec<wgpu::Buffer> = (0..3)
//!     .map(|_| device.create_buffer(&wgpu::BufferDescriptor {
//!         label: Some("tvpipe readback"),
//!         size: (padded * H) as u64,
//!         usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
//!         mapped_at_creation: false,
//!     }))
//!     .collect();
//!
//! // Every frame, after rendering into `target` (Rgba8Unorm, COPY_SRC):
//! if stream.wants_frame() {
//!     let buf = &staging[frame % 3];
//!     encoder.copy_texture_to_buffer(
//!         target.as_image_copy(),
//!         wgpu::TexelCopyBufferInfo {
//!             buffer: buf,
//!             layout: wgpu::TexelCopyBufferLayout {
//!                 offset: 0,
//!                 bytes_per_row: Some(padded),
//!                 rows_per_image: Some(H),
//!             },
//!         },
//!         wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
//!     );
//!     buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
//!     pending.push_back((frame, stream.elapsed()));
//! }
//! queue.submit([encoder.finish()]);
//! device.poll(wgpu::PollType::Poll)?;
//! // Two frames later the mapping is done: no stall on the GPU.
//! if let Some(&(old, pts)) = pending.front().filter(|(f, _)| frame >= f + 2) {
//!     let buf = &staging[old % 3];
//!     {
//!         let data = buf.slice(..).get_mapped_range();
//!         let f = tvpipe::Frame::new(&data, padded as usize);
//!         let _ = stream.write_video_frame(f, pts)?;
//!     }
//!     buf.unmap();
//!     pending.pop_front();
//! }
//! ```
//!
//! Use [`PixelFormat::Bgra`] for a `Bgra8Unorm` surface. Checking
//! [`Stream::wants_frame`] first skips the copy for frames the encoder
//! would drop. An RGB→NV12 compute pass that writes a 1.5-byte-per-pixel
//! buffer cuts the readback by 2.7× and the conversion on the CPU:
//! [`PixelFormat::Nv12`] with [`Frame::planes`].
//!
//! # Bevy and macroquad
//!
//! - **Bevy**: a camera rendering to an `Image` with `COPY_SRC`, a render
//!   graph node copying it to a staging buffer and sending the mapped
//!   bytes over a channel to the main world (the shape of Bevy's
//!   `headless_renderer` example), then as above. The screenshot API is
//!   too slow per frame.
//! - **macroquad / miniquad** (OpenGL): render to a `RenderTarget`, read
//!   it with `get_texture_data()`; GL rows are bottom-up:
//!   `Frame::new(&bytes, w * 4).bottom_up(true)`.
//!
//! # Audio from a sound card callback (cpal)
//!
//! ```ignore
//! let audio = stream.audio_writer(); // Send + Clone
//! device.build_output_stream(&config, move |out: &mut [f32], _| {
//!     mixer.fill(out);
//!     audio.write_f32(out); // never blocks for long
//! }, |e| eprintln!("{e}"), None)?;
//! ```
//!
//! Use the device's sample rate in `Options::audio`. The TV plays about
//! 0.1–0.3 s after the local speakers; games usually mute one.
//!
//! # A hardware encoder
//!
//! With NVENC, VideoToolbox, Media Foundation or VA-API through the
//! program's own bindings: `Options::h264(w, h, fps)`, no B-frames, a
//! low-latency preset, an IDR when [`Stream::take_keyframe_request`] says
//! so (a long GOP is fine), then [`Stream::write_h264`].
//!
//! # FairPlay and crash isolation: the `tvpipe` command
//!
//! A program that wants FairPlay without linking LGPL code, or wants a
//! crash in tvpipe not to take it down, runs `tvpipe --machine` as a child
//! (like the Node SDK): SLRW records on its stdin ([`slrw`](crate::slrw)),
//! one JSON event per line on its stdout ([`Event::from_json`]); see
//! docs/machine.md. With `default-features = false` the library is then
//! only those two formats.
//!
//! # Shipping
//!
//! - **macOS**: the program makes the network connections, so macOS's
//!   Local Network prompt names it; its `Info.plist` needs
//!   `NSLocalNetworkUsageDescription`.
//! - **Windows**: the browser player listens on 127.0.0.1 only (no
//!   firewall prompt).
//! - **Debug builds**: the built-in H.264 encoder is too slow unoptimized;
//!   add `[profile.dev.package.tvpipe] opt-level = 3` (and the same for
//!   `ring`).

#[allow(unused_imports)]
use crate::{Event, Frame, PixelFormat, Stream};
