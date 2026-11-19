//! Per-frame rate control with a small virtual buffer.
//!
//! Model: bits(qp) ≈ K · 2^(−qp/6) per picture type (the quantiser step
//! doubles every 6 QP), with K re-estimated after every frame. The buffer
//! term pulls the per-frame budget back towards the average so that the
//! stream converges on the target bitrate while single frames stay within
//! a small multiple of the average (low latency). Frames that overshoot
//! their cap are re-encoded at a higher QP by the caller.
//!
//! Sources: textbook rate-quantiser modelling (Richardson, "The H.264
//! Advanced Video Compression Standard", §10.5); the parameters are ours.

use super::mb::SrcPic;
use super::Config;

pub const QP_MIN: u8 = 10;
pub const QP_MAX: u8 = 51;

pub struct RateControl {
    /// Average bits per frame.
    target: f64,
    fps: f64,
    k_i: Option<f64>,
    k_p: Option<f64>,
    /// Bits sent above the nominal budget so far (VBV fullness).
    buffer: f64,
    last_p_qp: Option<f64>,
    last_qp: f64,
    /// Complexity at which `k_p` was estimated, and the current one.
    prev_luma: Vec<u8>,
    cplx_p: Option<f64>,
    cplx_now: f64,
}

/// IDR budget, in average frames.
const I_RATIO: f64 = 2.5;
/// Hard caps (multiples of the average frame) that trigger a re-encode.
const I_CAP: f64 = 4.0;
const P_CAP: f64 = 2.0;
/// Buffer size in average frames; fullness beyond it is clipped.
const VBV_FRAMES: f64 = 8.0;

impl RateControl {
    pub fn new(cfg: &Config) -> RateControl {
        let fps = cfg.fps.as_f64().max(1.0);
        let target = (cfg.bitrate_kbps.max(1) as f64 * 1000.0) / fps;
        let pixels = cfg.width as f64 * cfg.height as f64;
        // Initial guess: about QP 26 at 0.1 bit/pixel for P frames.
        let bpp = target / pixels;
        let qp0 = (26.0 - 6.0 * (bpp / 0.1).log2()).clamp(QP_MIN as f64 + 8.0, 44.0);
        RateControl {
            target,
            fps,
            k_i: None,
            k_p: None,
            buffer: 0.0,
            last_p_qp: None,
            last_qp: qp0,
            prev_luma: Vec::new(),
            cplx_p: None,
            cplx_now: 1.0,
        }
    }

    pub fn set_bitrate(&mut self, kbps: u32) {
        self.target = kbps.max(1) as f64 * 1000.0 / self.fps;
        self.buffer = 0.0;
    }

    fn qp_for(k: f64, bits: f64) -> f64 {
        6.0 * (k / bits.max(1.0)).log2()
    }

    /// Mean absolute difference to the previous source on a sparse grid
    /// (a cheap temporal complexity measure).
    fn temporal_complexity(&mut self, src: &SrcPic) -> f64 {
        let p = &src.y;
        let mut sample = Vec::with_capacity((p.w / 4) * (p.h / 4));
        for y in (0..p.h).step_by(4) {
            let row = p.row(y);
            for x in (0..p.w).step_by(4) {
                sample.push(row[x]);
            }
        }
        let c = if self.prev_luma.len() == sample.len() {
            let s: u64 = sample
                .iter()
                .zip(&self.prev_luma)
                .map(|(a, b)| a.abs_diff(*b) as u64)
                .sum();
            (s as f64 / sample.len().max(1) as f64).max(0.5)
        } else {
            8.0
        };
        self.prev_luma = sample;
        c
    }

    /// QP for the next frame.
    pub fn frame_qp(&mut self, idr: bool, src: &SrcPic) -> u8 {
        self.cplx_now = self.temporal_complexity(src);
        let buf = self
            .buffer
            .clamp(-VBV_FRAMES * self.target, VBV_FRAMES * self.target);
        let qp = if idr {
            let t = (I_RATIO * self.target - buf / 4.0).max(self.target);
            match self.k_i {
                Some(k) => Self::qp_for(k, t),
                None => self.last_p_qp.unwrap_or(self.last_qp) + 4.0,
            }
        } else {
            let t = (self.target - buf / 4.0).clamp(self.target / 4.0, 1.5 * self.target);
            match (self.k_p, self.cplx_p) {
                (Some(k), c) => {
                    // Scale the model with the change in temporal activity,
                    // damped (bits do not grow linearly with it).
                    let k = match c {
                        Some(c) => k * (self.cplx_now / c).powf(0.5).clamp(0.25, 4.0),
                        None => k,
                    };
                    let q = Self::qp_for(k, t);
                    match self.last_p_qp {
                        Some(last) => q.clamp(last - 2.0, last + 4.0),
                        None => q,
                    }
                }
                // First P frame: a little coarser than the IDR before it.
                (None, _) => self.last_qp + 2.0,
            }
        };
        let q = qp.round().clamp(QP_MIN as f64, QP_MAX as f64);
        q as u8
    }

    /// If a frame of `bits` at `qp` is over its cap, the QP to retry with.
    pub fn check_overshoot(&self, idr: bool, qp: u8, bits: usize) -> Option<u8> {
        let cap = if idr { I_CAP } else { P_CAP } * self.target;
        let bits = bits as f64;
        if bits <= cap || qp >= QP_MAX {
            return None;
        }
        let delta = (6.0 * (bits / (0.9 * cap)).log2()).ceil().clamp(1.0, 8.0);
        Some((qp as f64 + delta).min(QP_MAX as f64) as u8)
    }

    /// Feeds back the size of the frame that was sent.
    pub fn update(&mut self, idr: bool, qp: u8, bits: usize) {
        let bits = bits as f64;
        let k = bits * 2f64.powf(qp as f64 / 6.0);
        if idr {
            self.k_i = Some(match self.k_i {
                Some(old) => 0.5 * old + 0.5 * k,
                None => k,
            });
        } else {
            // Normalise K to the complexity it was measured at.
            self.k_p = Some(match (self.k_p, self.cplx_p) {
                (Some(old), Some(c)) => {
                    let old_now = old * (self.cplx_now / c).powf(0.5).clamp(0.25, 4.0);
                    0.6 * k + 0.4 * old_now
                }
                _ => k,
            });
            self.cplx_p = Some(self.cplx_now);
            self.last_p_qp = Some(qp as f64);
        }
        self.last_qp = qp as f64;
        self.buffer = (self.buffer + bits - self.target)
            .clamp(-VBV_FRAMES * self.target, VBV_FRAMES * self.target);
    }
}
