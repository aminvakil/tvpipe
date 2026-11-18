//! Raw picture formats for the built-in encoder: the planar 4:2:0 frame it
//! takes, conversions from packed RGB/BGR and NV12, and a bilinear scaler.

use crate::error::{Error, Result};

/// A planar 8-bit 4:2:0 picture (I420). Planes are tightly packed: luma
/// stride = width, chroma stride = ceil(width / 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Yuv420Frame {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl Yuv420Frame {
    /// A black (limited range) frame.
    pub fn new(width: u32, height: u32) -> Yuv420Frame {
        let (cw, ch) = chroma_size(width, height);
        Yuv420Frame {
            width,
            height,
            y: vec![16; width as usize * height as usize],
            u: vec![128; cw * ch],
            v: vec![128; cw * ch],
        }
    }

    /// Wraps planes after checking their sizes.
    pub fn from_planes(
        width: u32,
        height: u32,
        y: Vec<u8>,
        u: Vec<u8>,
        v: Vec<u8>,
    ) -> Result<Yuv420Frame> {
        let (cw, ch) = chroma_size(width, height);
        if y.len() != width as usize * height as usize || u.len() != cw * ch || v.len() != cw * ch {
            return Err(Error::malformed("yuv420: plane sizes do not match"));
        }
        Ok(Yuv420Frame {
            width,
            height,
            y,
            u,
            v,
        })
    }

    /// Parses a packed I420 buffer (Y, then U, then V).
    pub fn from_i420(width: u32, height: u32, data: &[u8]) -> Result<Yuv420Frame> {
        let ys = width as usize * height as usize;
        let (cw, ch) = chroma_size(width, height);
        let cs = cw * ch;
        if data.len() != ys + 2 * cs {
            return Err(Error::malformed("yuv420: buffer size does not match"));
        }
        Ok(Yuv420Frame {
            width,
            height,
            y: data[..ys].to_vec(),
            u: data[ys..ys + cs].to_vec(),
            v: data[ys + cs..].to_vec(),
        })
    }

    pub fn chroma_width(&self) -> usize {
        chroma_size(self.width, self.height).0
    }

    pub fn chroma_height(&self) -> usize {
        chroma_size(self.width, self.height).1
    }

    /// Packed I420 bytes.
    pub fn to_i420(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.y.len() + 2 * self.u.len());
        out.extend_from_slice(&self.y);
        out.extend_from_slice(&self.u);
        out.extend_from_slice(&self.v);
        out
    }
}

fn chroma_size(width: u32, height: u32) -> (usize, usize) {
    (width.div_ceil(2) as usize, height.div_ceil(2) as usize)
}
