//! Built-in H.264 encoder (Constrained Baseline, CAVLC) for raw game
//! frames: no ffmpeg, keyframes on demand, one picture of latency.
//!
//! Tools: IDR and P pictures (one reference, no B), Intra_16x16 /
//! Intra_4x4 / chroma intra, P_L0_16x16 with quarter-sample motion search
//! and P_Skip, deblocking, per-frame rate control with a small VBV, and one
//! slice per thread (horizontal bands of macroblock rows).
//!
//! Output is AVCC (4-byte lengths), parameter sets via [`Encoder::avcc`].
//!
//! Sources: ITU-T H.264 (08/2021), see the submodules; I. Richardson, "The
//! H.264 Advanced Video Compression Standard" (2nd ed.). Clean room: no
//! x264/ffmpeg encoder source was consulted (decision 0006); tables were
//! cross-checked with openh264 (BSD-2-Clause).

pub mod bits;
pub mod cavlc;
pub mod deblock;
pub mod headers;
pub mod inter;
pub mod intra;
pub mod mb;
pub mod picture;
pub mod rc;
pub mod tables;
pub mod transform;

#[cfg(test)]
mod tests;

/// ffmpeg for oracle tests elsewhere in `media`.
#[cfg(test)]
pub(crate) fn test_ffmpeg() -> Option<std::path::PathBuf> {
    tests::ffmpeg()
}

use crate::error::{Error, Result};
use crate::media::convert::Yuv420Frame;
use crate::media::h264::AvcConfig;
use crate::util::ratio::Ratio;

pub use headers::ColorInfo;
use headers::{pick_level, SeqParams, SliceHeader, LOG2_MAX_FRAME_NUM};
use inter::RefPic;
use mb::{FrameIn, SliceEnc, SrcPic, Tools};
use picture::Plane;

/// Per-macroblock state kept for neighbours and the deblocking filter.
#[derive(Debug, Clone, Copy)]
pub struct MbInfo {
    pub intra: bool,
    /// TotalCoeff of each luma 4x4 block (raster by block position).
    pub nz: [u8; 16],
    /// TotalCoeff of the chroma AC blocks.
    pub nzc: [[u8; 4]; 2],
    /// Intra4x4PredMode per block (2 = DC for non-Intra_4x4 macroblocks).
    pub i4: [u8; 16],
    /// Quarter-sample motion vector (16x16 partition).
    pub mv: [i16; 2],
    /// Reference index, −1 for intra.
    pub refi: i8,
    /// QPY for deblocking (0 for I_PCM).
    pub qp: u8,
}

impl Default for MbInfo {
    fn default() -> MbInfo {
        MbInfo {
            intra: true,
            nz: [0; 16],
            nzc: [[0; 4]; 2],
            i4: [intra::I4_DC; 16],
            mv: [0; 2],
            refi: -1,
            qp: 26,
        }
    }
}

/// Encoder settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Picture size; must be even.
    pub width: u32,
    pub height: u32,
    pub fps: Ratio,
    /// Target bitrate (ignored with a fixed `qp`).
    pub bitrate_kbps: u32,
    /// IDR interval in frames; 0 = only the first frame and on request.
    pub gop: u32,
    /// Worker threads = slices per picture (clamped to the macroblock
    /// rows); 0 = available parallelism.
    pub threads: u32,
    pub color: ColorInfo,
    /// Fixed QP instead of rate control.
    pub qp: Option<u8>,
    /// Repeat SPS/PPS in front of every IDR access unit.
    pub inline_parameter_sets: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            width: 1280,
            height: 720,
            fps: Ratio::integer(60),
            bitrate_kbps: 10_000,
            gop: 0,
            threads: 0,
            color: ColorInfo::default(),
            qp: None,
            inline_parameter_sets: false,
        }
    }
}

/// One encoded access unit.
#[derive(Debug, Clone)]
pub struct Encoded {
    /// AVCC: 4-byte big-endian length-prefixed NAL units.
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// QP the picture was coded with.
    pub qp: u8,
}

pub struct Encoder {
    cfg: Config,
    sp: SeqParams,
    sps: Vec<u8>,
    pps: Vec<u8>,
    slices: Vec<SliceEnc>,
    src: SrcPic,
    refp: Option<RefPic>,
    spare: Option<RefPic>,
    frame_num: u32,
    idr_pic_id: u32,
    since_idr: u32,
    want_idr: bool,
    rc: rc::RateControl,
    tools: Tools,
    threads: usize,
}

impl Encoder {
    pub fn new(cfg: Config) -> Result<Encoder> {
        if cfg.width == 0 || cfg.height == 0 || cfg.width % 2 != 0 || cfg.height % 2 != 0 {
            return Err(Error::unsupported(
                "h264enc: width and height must be even and non-zero",
            ));
        }
        if cfg.width > 8192 || cfg.height > 8192 {
            return Err(Error::unsupported("h264enc: picture too large"));
        }
        if cfg.fps.num == 0 || cfg.fps.den == 0 {
            return Err(Error::usage("h264enc: invalid frame rate"));
        }
        if let Some(qp) = cfg.qp {
            if qp > 51 {
                return Err(Error::usage("h264enc: qp must be 0..=51"));
            }
        }
        let mb_w = cfg.width.div_ceil(16);
        let mb_h = cfg.height.div_ceil(16);
        let sp = SeqParams {
            width: cfg.width,
            height: cfg.height,
            mb_w,
            mb_h,
            fps_num: cfg.fps.num,
            fps_den: cfg.fps.den,
            level_idc: pick_level(mb_w, mb_h, cfg.fps.num, cfg.fps.den, cfg.bitrate_kbps),
            color: cfg.color,
        };
        let sps = bits::nal_unit(3, 7, &sp.sps_rbsp());
        let pps = bits::nal_unit(3, 8, &headers::pps_rbsp());
        let threads = match cfg.threads {
            0 => std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
                .min(16),
            n => n as usize,
        }
        .clamp(1, mb_h as usize);
        let (w, h) = (mb_w as usize * 16, mb_h as usize * 16);
        let src = SrcPic {
            y: Plane::new(w, h, 0),
            u: Plane::new(w / 2, h / 2, 0),
            v: Plane::new(w / 2, h / 2, 0),
        };
        let mut slices = Vec::with_capacity(threads);
        let rows = mb_h as usize;
        for i in 0..threads {
            let r0 = rows * i / threads;
            let r1 = rows * (i + 1) / threads;
            slices.push(SliceEnc::new(r0, r1 - r0, mb_w as usize));
        }
        let rc = rc::RateControl::new(&cfg);
        Ok(Encoder {
            cfg,
            sp,
            sps,
            pps,
            slices,
            src,
            refp: None,
            spare: None,
            frame_num: 0,
            idr_pic_id: 0,
            since_idr: 0,
            want_idr: true,
            rc,
            tools: Tools::default(),
            threads,
        })
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// The `avcC` record (SPS + PPS).
    pub fn avcc(&self) -> Vec<u8> {
        AvcConfig::from_parameter_sets(vec![self.sps.clone()], vec![self.pps.clone()])
            .and_then(|c| c.to_bytes())
            .unwrap_or_default()
    }

    /// SPS and PPS NAL units (no start codes / lengths).
    pub fn parameter_sets(&self) -> (&[u8], &[u8]) {
        (&self.sps, &self.pps)
    }

    /// Makes the next encoded frame an IDR picture.
    pub fn request_keyframe(&mut self) {
        self.want_idr = true;
    }

    /// Changes the target bitrate (takes effect with the next frame).
    pub fn set_bitrate(&mut self, kbps: u32) {
        self.cfg.bitrate_kbps = kbps;
        self.rc.set_bitrate(kbps);
    }

    pub(crate) fn set_tools(&mut self, tools: Tools) {
        self.tools = tools;
    }

    /// Encodes one frame.
    pub fn encode(&mut self, frame: &Yuv420Frame, force_idr: bool) -> Result<Encoded> {
        if frame.width != self.cfg.width || frame.height != self.cfg.height {
            return Err(Error::usage(format!(
                "h264enc: frame is {}x{}, encoder expects {}x{}",
                frame.width, frame.height, self.cfg.width, self.cfg.height
            )));
        }
        let (cw, ch) = (frame.chroma_width(), frame.chroma_height());
        let ys = frame.width as usize * frame.height as usize;
        if frame.y.len() != ys || frame.u.len() != cw * ch || frame.v.len() != cw * ch {
            return Err(Error::malformed("h264enc: plane sizes do not match"));
        }
        load_plane(
            &mut self.src.y,
            &frame.y,
            frame.width as usize,
            frame.height as usize,
        );
        load_plane(&mut self.src.u, &frame.u, cw, ch);
        load_plane(&mut self.src.v, &frame.v, cw, ch);

        let gop_due = self.cfg.gop > 0 && self.since_idr >= self.cfg.gop;
        let idr = force_idr || self.want_idr || self.refp.is_none() || gop_due;
        if idr {
            self.frame_num = 0;
        }
        let mut qp = match self.cfg.qp {
            Some(q) => q,
            None => self.rc.frame_qp(idr, &self.src),
        };
        let mut tries = 0;
        let data = loop {
            let data = self.encode_pass(idr, qp);
            if self.cfg.qp.is_some() {
                break data;
            }
            match self.rc.check_overshoot(idr, qp, data.len() * 8) {
                Some(q) if tries < 2 => {
                    qp = q;
                    tries += 1;
                }
                _ => break data,
            }
        };
        if self.cfg.qp.is_none() {
            self.rc.update(idr, qp, data.len() * 8);
        }
        self.finish_reference();
        if idr {
            self.idr_pic_id = (self.idr_pic_id + 1) % 65536;
            self.since_idr = 0;
            self.want_idr = false;
        }
        self.since_idr += 1;
        self.frame_num = (self.frame_num + 1) % (1 << LOG2_MAX_FRAME_NUM);
        Ok(Encoded {
            data,
            keyframe: idr,
            qp,
        })
    }

    /// Runs the slices and returns the AVCC access unit.
    fn encode_pass(&mut self, idr: bool, qp: u8) -> Vec<u8> {
        let hdr = SliceHeader {
            first_mb: 0,
            idr,
            p: !idr,
            frame_num: if idr { 0 } else { self.frame_num },
            idr_pic_id: self.idr_pic_id,
            qp,
            deblock_idc: if self.slices.len() > 1 { 2 } else { 0 },
        };
        let fin = FrameIn {
            src: &self.src,
            refp: if idr { None } else { self.refp.as_ref() },
            hdr,
            tools: self.tools,
        };
        let mb_w = self.sp.mb_w as usize;
        let run = |s: &mut SliceEnc| {
            s.encode(&fin);
            deblock::deblock_band(&mut s.rec, &s.info, mb_w);
        };
        if self.slices.len() == 1 {
            run(&mut self.slices[0]);
        } else {
            std::thread::scope(|scope| {
                let mut it = self.slices.iter_mut();
                let first = it.next();
                for s in it {
                    scope.spawn(|| run(s));
                }
                if let Some(s) = first {
                    run(s);
                }
            });
        }
        let mut out = Vec::with_capacity(self.slices.iter().map(|s| s.bw.bytes().len() + 8).sum());
        if idr && self.cfg.inline_parameter_sets {
            for ps in [&self.sps, &self.pps] {
                out.extend_from_slice(&(ps.len() as u32).to_be_bytes());
                out.extend_from_slice(ps);
            }
        }
        let (ref_idc, nut) = if idr { (3, 5) } else { (2, 1) };
        for s in &self.slices {
            bits::put_nal_avcc(&mut out, ref_idc, nut, s.bw.bytes());
        }
        out
    }

    /// Assembles the slices' reconstructions into the next reference.
    fn finish_reference(&mut self) {
        let (w, h) = (self.sp.mb_w as usize * 16, self.sp.mb_h as usize * 16);
        let mut r = self.spare.take().unwrap_or_else(|| RefPic::new(w, h));
        for s in &self.slices {
            r.y.copy_rows_from(&s.rec[0], s.row0 * 16);
            r.u.copy_rows_from(&s.rec[1], s.row0 * 8);
            r.v.copy_rows_from(&s.rec[2], s.row0 * 8);
        }
        r.y.extend_edges();
        r.u.extend_edges();
        r.v.extend_edges();
        build_half_parallel(&mut r, self.threads);
        self.spare = self.refp.replace(r);
    }

    /// The last reconstructed picture, cropped (for tests and PSNR).
    pub fn recon(&self) -> Option<Yuv420Frame> {
        let r = self.refp.as_ref()?;
        let (w, h) = (self.cfg.width as usize, self.cfg.height as usize);
        let mut out = Yuv420Frame::new(self.cfg.width, self.cfg.height);
        for y in 0..h {
            out.y[y * w..(y + 1) * w].copy_from_slice(&r.y.row(y)[..w]);
        }
        let (cw, ch) = (w / 2, h / 2);
        for y in 0..ch {
            out.u[y * cw..(y + 1) * cw].copy_from_slice(&r.u.row(y)[..cw]);
            out.v[y * cw..(y + 1) * cw].copy_from_slice(&r.v.row(y)[..cw]);
        }
        Some(out)
    }

    /// Macroblock statistics of the last frame: (intra, inter, skip).
    pub fn last_stats(&self) -> (u32, u32, u32) {
        self.slices.iter().fold((0, 0, 0), |a, s| {
            (a.0 + s.stats.intra, a.1 + s.stats.inter, a.2 + s.stats.skip)
        })
    }
}

/// Copies a plane into the macroblock-aligned source, replicating the
/// right and bottom edges.
fn load_plane(dst: &mut Plane, src: &[u8], w: usize, h: usize) {
    for y in 0..dst.h {
        let sy = y.min(h - 1);
        let row = &src[sy * w..sy * w + w];
        let d = dst.row_mut(y);
        d[..w].copy_from_slice(row);
        let last = row[w - 1];
        d[w..].fill(last);
    }
}

fn build_half_parallel(r: &mut RefPic, threads: usize) {
    let total = r.y.h + 2 * inter::LUMA_PAD;
    let s = r.y.stride;
    let n = threads.max(1);
    if n == 1 {
        r.build_half();
        return;
    }
    let per = total.div_ceil(n);
    let y = &r.y;
    std::thread::scope(|scope| {
        let chunks =
            r.hb.data
                .chunks_mut(per * s)
                .zip(r.hh.data.chunks_mut(per * s))
                .zip(r.hj.data.chunks_mut(per * s))
                .enumerate();
        for (i, ((b, h), j)) in chunks {
            let rows = i * per..((i + 1) * per).min(total);
            scope.spawn(move || RefPic::half_rows_into(y, rows, b, h, j));
        }
    });
}
