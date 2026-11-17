//! Media pipeline: ffmpeg subprocess, Matroska demux, H.264, raw game
//! input, media clock, fanout.
//!
//! Owner: W4. Planned files: `ffmpeg.rs`, `mkv.rs`, `h264.rs`, `rawin.rs`,
//! `clock.rs`, `fanout.rs` (PLAN §3, §5).

pub mod h264;
pub mod mkv;
