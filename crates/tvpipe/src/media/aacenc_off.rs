//! `media::aacenc` without the `aac` feature: the same surface, and an
//! encoder that cannot be made, so PCM never becomes AAC (Cast receivers
//! then get no in-process audio).

use crate::error::{Error, Result};

/// Samples per channel in one AAC-LC frame.
pub const FRAME_LEN: usize = 1024;

/// Never constructed.
pub enum Encoder {}

impl Encoder {
    pub fn new(_sample_rate: u32, _channels: u8, _bitrate_bps: u32) -> Result<Encoder> {
        Err(Error::feature("aac", "AAC encoding"))
    }

    pub fn encode(&mut self, _pcm: &[i16]) -> Result<Vec<u8>> {
        match *self {}
    }

    pub fn flush(&mut self) -> Vec<Vec<u8>> {
        match *self {}
    }

    pub fn delay_samples(&self) -> usize {
        match *self {}
    }

    pub fn audio_specific_config(&self) -> Vec<u8> {
        match *self {}
    }
}
