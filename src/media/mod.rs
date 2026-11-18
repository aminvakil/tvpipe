//! Media pipeline: ffmpeg subprocess, Matroska demux, H.264, raw game
//! input, media clock, fanout.
//!
//! Owner: W4 (PLAN §3, §5).

pub mod aacenc;
pub mod clock;
pub mod debug;
pub mod fanout;
pub mod ffmpeg;
pub mod h264;
pub mod mkv;
pub mod mp4;
pub mod rawin;
pub mod slrw;
pub mod source;
