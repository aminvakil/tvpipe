//! Slice encoding: macroblock mode decision, residual coding with
//! reconstruction, and macroblock layer syntax (CAVLC).
//!
//! One [`SliceEnc`] encodes one slice: a band of whole macroblock rows.
//! Neighbours outside the band are unavailable (they belong to another
//! slice), so bands are independent and run on separate threads.
//!
//! Sources: ITU-T H.264 §7.3.4 (slice_data, mb_skip_run), §7.3.5
//! (macroblock_layer, mb_pred, residual), Table 7-11 (I mb_type), Table
//! 7-13 (P mb_type), §7.4.5 (mb_qp_delta), §8.3.1.1 (Intra4x4PredMode
//! derivation), §8.4.1.1 (P_Skip vectors), §8.4.1.3 (vector prediction),
//! §9.2.1 (nC), §6.4.11 (neighbouring locations).

use super::bits::{se_len, BitWriter};
use super::cavlc::write_block;
use super::headers::SliceHeader;
use super::inter::{sad16, satd16, RefPic};
use super::intra::{self, Edges, Edges4};
use super::picture::Plane;
use super::tables::{BLK_XY, CBP_INTER_CODE, CBP_INTRA_CODE, CHROMA_QP, ZIGZAG};
use super::transform::{
    chroma_dc_dequant, chroma_dc_quant, dequant4, fdct4, idct4, luma_dc_dequant, luma_dc_quant,
    quant4, satd4, Deadzone,
};
use super::MbInfo;

/// Source picture, macroblock-aligned (planes without padding).
#[derive(Debug, Clone, Default)]
pub struct SrcPic {
    pub y: Plane,
    pub u: Plane,
    pub v: Plane,
}

/// Encoding tools that can be switched off (tests, speed).
#[derive(Debug, Clone, Copy)]
pub struct Tools {
    pub i4x4: bool,
    pub subpel: bool,
    /// Full-sample search steps.
    pub search_iters: u32,
    /// Encode every macroblock as I_PCM (sanity tests only).
    pub pcm: bool,
}

impl Default for Tools {
    fn default() -> Tools {
        Tools {
            i4x4: true,
            subpel: true,
            search_iters: 16,
            pcm: false,
        }
    }
}

/// Per-frame inputs shared by all slices.
pub struct FrameIn<'a> {
    pub src: &'a SrcPic,
    pub refp: Option<&'a RefPic>,
    pub hdr: SliceHeader,
    pub tools: Tools,
    /// Largest |mv| component (quarter samples), [x, y].
    pub mv_limit: [i32; 2],
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SliceStats {
    pub intra: u32,
    pub inter: u32,
    pub skip: u32,
}

/// Encoder state for one band of macroblock rows.
pub struct SliceEnc {
    pub row0: usize,
    pub rows: usize,
    pub mb_w: usize,
    pub rec: [Plane; 3],
    pub info: Vec<MbInfo>,
    /// `info` of the previous picture (temporal motion candidates).
    prev: Vec<MbInfo>,
    pub bw: BitWriter,
    pub stats: SliceStats,
}

/// Lagrangian multiplier for SAD/SATD costs: ≈ 0.85^½ · 2^((qp − 12) / 6)
/// (the usual H.264 reference-encoder choice), in 1/16 units.
fn lambda16(qp: u8) -> u32 {
    let v = 0.92 * 16.0 * 2f64.powf((qp.min(51) as f64 - 12.0) / 6.0);
    (v.round() as u32).max(1)
}

/// λ for SSD costs, 0.85 · 2^((qp − 12) / 3) (the reference encoder's
/// mode-decision λ), in 1/16 units.
fn lambda_ssd16(qp: u8) -> u64 {
    let v = 0.85 * 16.0 * 2f64.powf((qp.min(51) as f64 - 12.0) / 3.0);
    (v.round() as u64).max(1)
}

/// Copies the chroma part of `from` into `to`.
fn merge_chroma(to: &mut Coded, from: &Coded) {
    to.chroma_mode = from.chroma_mode;
    to.cdc = from.cdc;
    to.cac = from.cac;
    to.nzc = from.nzc;
    to.cbp_chroma = from.cbp_chroma;
}

#[inline]
fn chroma_qp(qp: u8) -> u8 {
    CHROMA_QP[qp.min(51) as usize]
}

/// Everything needed to write one coded (non-skipped) macroblock.
#[derive(Clone)]
struct Coded {
    kind: Kind,
    /// Intra_4x4 modes, raster by block position.
    i4: [u8; 16],
    chroma_mode: u8,
    /// Luma levels in scan order per block (raster by block position).
    luma: [[i32; 16]; 16],
    luma_dc: [i32; 16],
    cdc: [[i32; 4]; 2],
    cac: [[[i32; 16]; 4]; 2],
    nz: [u8; 16],
    nzc: [[u8; 4]; 2],
    cbp_luma: u8,
    cbp_chroma: u8,
    mv: [i32; 2],
    mvd: [i32; 2],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    I16(u8),
    I4,
    Inter,
    Pcm,
}

impl Coded {
    fn new(kind: Kind) -> Coded {
        Coded {
            kind,
            i4: [intra::I4_DC; 16],
            chroma_mode: intra::C_DC,
            luma: [[0; 16]; 16],
            luma_dc: [0; 16],
            cdc: [[0; 4]; 2],
            cac: [[[0; 16]; 4]; 2],
            nz: [0; 16],
            nzc: [[0; 4]; 2],
            cbp_luma: 0,
            cbp_chroma: 0,
            mv: [0; 2],
            mvd: [0; 2],
        }
    }
}

fn median(a: i32, b: i32, c: i32) -> i32 {
    a.max(b).min(a.min(b).max(c))
}

/// Raster 4x4 residual of block (bx, by) of an NxN block.
#[inline]
fn residual4<const N: usize>(src: &[u8], pred: &[u8], bx: usize, by: usize) -> [i32; 16] {
    let mut d = [0i32; 16];
    for y in 0..4 {
        for x in 0..4 {
            let i = (by * 4 + y) * N + bx * 4 + x;
            d[y * 4 + x] = src[i] as i32 - pred[i] as i32;
        }
    }
    d
}

#[inline]
fn add4<const N: usize>(out: &mut [u8], pred: &[u8], r: &[i32; 16], bx: usize, by: usize) {
    for y in 0..4 {
        for x in 0..4 {
            let i = (by * 4 + y) * N + bx * 4 + x;
            out[i] = (pred[i] as i32 + r[y * 4 + x]).clamp(0, 255) as u8;
        }
    }
}

fn to_scan(raster: &[i32; 16]) -> [i32; 16] {
    core::array::from_fn(|i| raster[ZIGZAG[i]])
}

fn satd_n<const N: usize>(src: &[u8], pred: &[u8]) -> u32 {
    let mut s = 0;
    for by in 0..N / 4 {
        for bx in 0..N / 4 {
            s += satd4(&residual4::<N>(src, pred, bx, by));
        }
    }
    s
}

impl SliceEnc {
    pub fn new(row0: usize, rows: usize, mb_w: usize) -> SliceEnc {
        SliceEnc {
            row0,
            rows,
            mb_w,
            rec: [
                Plane::new(mb_w * 16, rows * 16, 0),
                Plane::new(mb_w * 8, rows * 8, 0),
                Plane::new(mb_w * 8, rows * 8, 0),
            ],
            info: vec![MbInfo::default(); mb_w * rows],
            prev: vec![MbInfo::default(); mb_w * rows],
            bw: BitWriter::with_capacity(1 << 16),
            stats: SliceStats::default(),
        }
    }

    /// Call once per picture before [`encode`](SliceEnc::encode) (which may
    /// run several times when rate control re-encodes).
    pub fn begin_picture(&mut self) {
        self.prev.clone_from(&self.info);
    }

    /// Encodes the slice; the RBSP (header + data + trailing bits) ends up
    /// in `self.bw`.
    pub fn encode(&mut self, f: &FrameIn<'_>) {
        self.bw.clear();
        self.stats = SliceStats::default();
        let mut hdr = f.hdr;
        hdr.first_mb = (self.row0 * self.mb_w) as u32;
        hdr.write(&mut self.bw);
        let qp = hdr.qp;
        let lambda = lambda16(qp);
        let mut skip_run = 0u32;
        let mut qp_pred = qp;
        for my in 0..self.rows {
            for mx in 0..self.mb_w {
                let coded = match (f.refp, hdr.p) {
                    (Some(r), true) => self.decide_p(f, r, mx, my, qp, lambda),
                    _ => Some(self.decide_intra(f, mx, my, qp, lambda)),
                };
                match coded {
                    None => {
                        skip_run += 1;
                        self.stats.skip += 1;
                    }
                    Some(c) => {
                        if hdr.p {
                            self.bw.ue(skip_run);
                            skip_run = 0;
                        }
                        if c.kind == Kind::Inter {
                            self.stats.inter += 1;
                        } else {
                            self.stats.intra += 1;
                        }
                        self.write_mb(&c, mx, my, hdr.p, qp, &mut qp_pred);
                    }
                }
            }
        }
        if skip_run > 0 {
            self.bw.ue(skip_run);
        }
        self.bw.trailing();
    }

    fn src_luma(&self, src: &SrcPic, mx: usize, my: usize) -> [u8; 256] {
        let mut out = [0u8; 256];
        let y0 = (self.row0 + my) * 16;
        for r in 0..16 {
            out[r * 16..r * 16 + 16].copy_from_slice(&src.y.row(y0 + r)[mx * 16..mx * 16 + 16]);
        }
        out
    }

    fn src_chroma(&self, src: &SrcPic, mx: usize, my: usize) -> [[u8; 64]; 2] {
        let mut out = [[0u8; 64]; 2];
        let y0 = (self.row0 + my) * 8;
        for (o, p) in out.iter_mut().zip([&src.u, &src.v]) {
            for r in 0..8 {
                o[r * 8..r * 8 + 8].copy_from_slice(&p.row(y0 + r)[mx * 8..mx * 8 + 8]);
            }
        }
        out
    }

    fn store<const N: usize>(plane: &mut Plane, mx: usize, my: usize, px: &[u8]) {
        for r in 0..N {
            plane.row_mut(my * N + r)[mx * N..mx * N + N].copy_from_slice(&px[r * N..r * N + N]);
        }
    }

    fn edges<const N: usize>(plane: &Plane, mx: usize, my: usize) -> Edges<N> {
        let (x0, y0) = (mx * N, my * N);
        let top = (my > 0).then(|| {
            let mut t = [0u8; N];
            t.copy_from_slice(&plane.row(y0 - 1)[x0..x0 + N]);
            t
        });
        let left = (mx > 0).then(|| core::array::from_fn(|i| plane.at(x0 - 1, y0 + i)));
        let top_left = if mx > 0 && my > 0 {
            plane.at(x0 - 1, y0 - 1)
        } else {
            0
        };
        Edges {
            top,
            left,
            top_left,
        }
    }

    fn nb(&self, mx: isize, my: isize) -> Option<&MbInfo> {
        if mx < 0 || my < 0 || mx >= self.mb_w as isize || my >= self.rows as isize {
            return None;
        }
        Some(&self.info[my as usize * self.mb_w + mx as usize])
    }

    // ---- intra ----------------------------------------------------------

    fn decide_intra(
        &mut self,
        f: &FrameIn<'_>,
        mx: usize,
        my: usize,
        qp: u8,
        lambda: u32,
    ) -> Coded {
        let src = self.src_luma(f.src, mx, my);
        let srcc = self.src_chroma(f.src, mx, my);
        if f.tools.pcm {
            return self.code_pcm(&src, &srcc, mx, my);
        }
        let (mode16, cost16) = self.best_i16(&src, mx, my);
        self.code_intra(
            &src,
            &srcc,
            mx,
            my,
            qp,
            lambda,
            mode16,
            cost16,
            f.tools.i4x4,
            f.hdr.p,
        )
    }

    fn best_i16(&self, src: &[u8; 256], mx: usize, my: usize) -> (u8, u32) {
        let e = Self::edges::<16>(&self.rec[0], mx, my);
        let mut pred = [0u8; 256];
        let mut best = (intra::I16_DC, u32::MAX);
        for mode in 0..4 {
            if !intra::i16_available(mode, &e) {
                continue;
            }
            intra::pred16(mode, &e, &mut pred);
            let c = satd16(src, &pred);
            if c < best.1 {
                best = (mode, c);
            }
        }
        best
    }

    /// Intra macroblock: chroma first (independent of the luma mode), then
    /// Intra_16x16 or Intra_4x4, chosen by SSD + λ · bits when their SATD
    /// costs are close.
    #[allow(clippy::too_many_arguments)]
    fn code_intra(
        &mut self,
        src: &[u8; 256],
        srcc: &[[u8; 64]; 2],
        mx: usize,
        my: usize,
        qp: u8,
        lambda: u32,
        mode16: u8,
        cost16: u32,
        try_i4: bool,
        p: bool,
    ) -> Coded {
        let mut ch = Coded::new(Kind::I16(mode16));
        self.code_intra_chroma(srcc, mx, my, qp, &mut ch);
        let mut c = if try_i4 {
            let i16_cost = cost16 + (lambda * 6) / 16;
            let (mut c4, cost4) = self.code_i4(src, mx, my, qp, lambda, i16_cost + i16_cost / 4);
            if cost4 == u32::MAX {
                self.code_i16(src, mx, my, qp, mode16)
            } else {
                let ls = lambda_ssd16(qp);
                merge_chroma(&mut c4, &ch);
                let j4 =
                    self.ssd_luma(src, mx, my) * 16 + ls * self.mb_bits(&c4, mx, my, p, qp) as u64;
                let px4 = self.save_luma(mx, my);
                let mut c16 = self.code_i16(src, mx, my, qp, mode16);
                merge_chroma(&mut c16, &ch);
                let j16 =
                    self.ssd_luma(src, mx, my) * 16 + ls * self.mb_bits(&c16, mx, my, p, qp) as u64;
                if j4 < j16 {
                    Self::store::<16>(&mut self.rec[0], mx, my, &px4);
                    c4
                } else {
                    c16
                }
            }
        } else {
            self.code_i16(src, mx, my, qp, mode16)
        };
        merge_chroma(&mut c, &ch);
        let info = &mut self.info[my * self.mb_w + mx];
        *info = MbInfo {
            intra: true,
            nz: c.nz,
            nzc: c.nzc,
            i4: c.i4,
            mv: [0, 0],
            refi: -1,
            qp,
        };
        c
    }

    fn save_luma(&self, mx: usize, my: usize) -> [u8; 256] {
        let mut out = [0u8; 256];
        for r in 0..16 {
            out[r * 16..r * 16 + 16]
                .copy_from_slice(&self.rec[0].row(my * 16 + r)[mx * 16..mx * 16 + 16]);
        }
        out
    }

    fn ssd_luma(&self, src: &[u8; 256], mx: usize, my: usize) -> u64 {
        let mut s = 0u64;
        for r in 0..16 {
            let rec = &self.rec[0].row(my * 16 + r)[mx * 16..mx * 16 + 16];
            for (a, b) in src[r * 16..r * 16 + 16].iter().zip(rec) {
                let d = *a as i32 - *b as i32;
                s += (d * d) as u64;
            }
        }
        s
    }

    fn ssd_chroma(&self, srcc: &[[u8; 64]; 2], mx: usize, my: usize) -> u64 {
        let mut s = 0u64;
        for (src, p) in srcc.iter().zip(&self.rec[1..]) {
            for r in 0..8 {
                let rec = &p.row(my * 8 + r)[mx * 8..mx * 8 + 8];
                for (a, b) in src[r * 8..r * 8 + 8].iter().zip(rec) {
                    let d = *a as i32 - *b as i32;
                    s += (d * d) as u64;
                }
            }
        }
        s
    }

    fn code_pcm(&mut self, src: &[u8; 256], srcc: &[[u8; 64]; 2], mx: usize, my: usize) -> Coded {
        Self::store::<16>(&mut self.rec[0], mx, my, src);
        Self::store::<8>(&mut self.rec[1], mx, my, &srcc[0]);
        Self::store::<8>(&mut self.rec[2], mx, my, &srcc[1]);
        self.info[my * self.mb_w + mx] = MbInfo {
            intra: true,
            nz: [16; 16],
            nzc: [[16; 4]; 2],
            i4: [intra::I4_DC; 16],
            mv: [0, 0],
            refi: -1,
            qp: 0,
        };
        Coded::new(Kind::Pcm)
    }

    fn code_i16(&mut self, src: &[u8; 256], mx: usize, my: usize, qp: u8, mode: u8) -> Coded {
        let e = Self::edges::<16>(&self.rec[0], mx, my);
        let mut pred = [0u8; 256];
        intra::pred16(mode, &e, &mut pred);
        let mut c = Coded::new(Kind::I16(mode));
        let mut ac = [[0i32; 16]; 16];
        let mut dc = [0i32; 16];
        let mut any_ac = false;
        for by in 0..4 {
            for bx in 0..4 {
                let b = by * 4 + bx;
                let mut w = fdct4(&residual4::<16>(src, &pred, bx, by));
                dc[b] = w[0];
                let n = quant4(&mut w, qp, Deadzone::Intra, true);
                any_ac |= n > 0;
                c.nz[b] = n as u8;
                ac[b] = w;
            }
        }
        let (dcl, _) = luma_dc_quant(&dc, qp);
        c.luma_dc = to_scan(&dcl);
        let dcy = luma_dc_dequant(&dcl, qp);
        c.cbp_luma = if any_ac { 15 } else { 0 };
        let mut rec = [0u8; 256];
        for by in 0..4 {
            for bx in 0..4 {
                let b = by * 4 + bx;
                let mut d = if any_ac { ac[b] } else { [0; 16] };
                c.luma[b] = to_scan(&d);
                dequant4(&mut d, qp, true);
                d[0] = dcy[b];
                add4::<16>(&mut rec, &pred, &idct4(&d), bx, by);
            }
        }
        if !any_ac {
            c.nz = [0; 16];
        }
        Self::store::<16>(&mut self.rec[0], mx, my, &rec);
        c
    }

    /// Neighbouring samples of 4x4 block (bx, by) from the reconstruction.
    fn edges4(&self, mx: usize, my: usize, bx: usize, by: usize) -> Edges4 {
        let p = &self.rec[0];
        let x0 = mx * 16 + bx * 4;
        let y0 = my * 16 + by * 4;
        let has_top = by > 0 || my > 0;
        let has_left = bx > 0 || mx > 0;
        // Top-right: inside the macroblock only for blocks decoded after
        // their top-right neighbour; above the macroblock it needs the
        // top (or top-right) macroblock.
        let has_tr = if by == 0 {
            if bx < 3 {
                my > 0
            } else {
                my > 0 && mx + 1 < self.mb_w
            }
        } else {
            matches!(
                (bx, by),
                (0, 1) | (2, 1) | (0, 2) | (1, 2) | (0, 3) | (2, 2) | (2, 3)
            )
        };
        let top = has_top.then(|| {
            let r = p.row(y0 - 1);
            let mut t = [0u8; 8];
            t[..4].copy_from_slice(&r[x0..x0 + 4]);
            if has_tr {
                t[4..].copy_from_slice(&r[x0 + 4..x0 + 8]);
            } else {
                let v = t[3];
                t[4..].fill(v);
            }
            t
        });
        let left = has_left.then(|| core::array::from_fn(|i| p.at(x0 - 1, y0 + i)));
        let top_left = (has_top && has_left).then(|| p.at(x0 - 1, y0 - 1));
        Edges4 {
            top,
            left,
            top_left,
        }
    }

    /// Predicted Intra4x4PredMode of block (bx, by) given the modes chosen
    /// so far in this macroblock.
    fn pred_i4_mode(&self, mx: usize, my: usize, cur: &[u8; 16], bx: usize, by: usize) -> u8 {
        let a = if bx > 0 {
            Some(cur[by * 4 + bx - 1])
        } else {
            self.nb(mx as isize - 1, my as isize)
                .map(|n| n.i4[by * 4 + 3])
        };
        let b = if by > 0 {
            Some(cur[(by - 1) * 4 + bx])
        } else {
            self.nb(mx as isize, my as isize - 1).map(|n| n.i4[12 + bx])
        };
        match (a, b) {
            (Some(a), Some(b)) => a.min(b),
            _ => intra::I4_DC,
        }
    }

    /// Intra_4x4 coding with reconstruction into `rec`. Stops early (with
    /// cost `u32::MAX`) once the cost exceeds `limit`.
    fn code_i4(
        &mut self,
        src: &[u8; 256],
        mx: usize,
        my: usize,
        qp: u8,
        lambda: u32,
        limit: u32,
    ) -> (Coded, u32) {
        let mut c = Coded::new(Kind::I4);
        let mut cost = (lambda * 24) / 16;
        let mut pred = [0u8; 16];
        let mut best_pred = [0u8; 16];
        for (blk, &(bx, by)) in BLK_XY.iter().enumerate() {
            let _ = blk;
            let e = self.edges4(mx, my, bx, by);
            let pm = self.pred_i4_mode(mx, my, &c.i4, bx, by);
            let mut s = [0u8; 16];
            for y in 0..4 {
                s[y * 4..y * 4 + 4].copy_from_slice(&src[(by * 4 + y) * 16 + bx * 4..][..4]);
            }
            let mut best = (intra::I4_DC, u32::MAX);
            for mode in 0..9 {
                if !intra::i4_available(mode, &e) {
                    continue;
                }
                intra::pred4(mode, &e, &mut pred);
                let bits = if mode == pm { 1 } else { 4 };
                let j = satd4(&residual4::<4>(&s, &pred, 0, 0)) + (lambda * bits) / 16;
                if j < best.1 {
                    best = (mode, j);
                    best_pred = pred;
                }
            }
            cost += best.1;
            if cost >= limit {
                return (c, u32::MAX);
            }
            c.i4[by * 4 + bx] = best.0;
            let mut w = fdct4(&residual4::<4>(&s, &best_pred, 0, 0));
            let n = quant4(&mut w, qp, Deadzone::Intra, false);
            c.nz[by * 4 + bx] = n as u8;
            c.luma[by * 4 + bx] = to_scan(&w);
            let mut out = [0u8; 16];
            if n > 0 {
                dequant4(&mut w, qp, false);
                add4::<4>(&mut out, &best_pred, &idct4(&w), 0, 0);
            } else {
                out = best_pred;
            }
            let p = &mut self.rec[0];
            for y in 0..4 {
                let x0 = mx * 16 + bx * 4;
                p.row_mut(my * 16 + by * 4 + y)[x0..x0 + 4].copy_from_slice(&out[y * 4..y * 4 + 4]);
            }
            if n > 0 {
                c.cbp_luma |= 1 << ((by / 2) * 2 + bx / 2);
            }
        }
        (c, cost)
    }

    fn code_intra_chroma(
        &mut self,
        srcc: &[[u8; 64]; 2],
        mx: usize,
        my: usize,
        qp: u8,
        c: &mut Coded,
    ) {
        let eu = Self::edges::<8>(&self.rec[1], mx, my);
        let ev = Self::edges::<8>(&self.rec[2], mx, my);
        let mut pu = [0u8; 64];
        let mut pv = [0u8; 64];
        let mut best = (intra::C_DC, u32::MAX, [0u8; 64], [0u8; 64]);
        for mode in 0..4 {
            if !intra::chroma_available(mode, &eu) {
                continue;
            }
            intra::pred_chroma(mode, &eu, &mut pu);
            intra::pred_chroma(mode, &ev, &mut pv);
            let cost = satd_n::<8>(&srcc[0], &pu) + satd_n::<8>(&srcc[1], &pv);
            if cost < best.1 {
                best = (mode, cost, pu, pv);
            }
        }
        c.chroma_mode = best.0;
        let preds = [best.2, best.3];
        self.code_chroma_residual(srcc, &preds, mx, my, qp, Deadzone::Intra, c);
    }

    #[allow(clippy::too_many_arguments)]
    fn code_chroma_residual(
        &mut self,
        srcc: &[[u8; 64]; 2],
        preds: &[[u8; 64]; 2],
        mx: usize,
        my: usize,
        qp: u8,
        dz: Deadzone,
        c: &mut Coded,
    ) {
        let qpc = chroma_qp(qp);
        let mut ac = [[[0i32; 16]; 4]; 2];
        let mut any_dc = false;
        let mut any_ac = false;
        for comp in 0..2 {
            let mut dc = [0i32; 4];
            for b in 0..4 {
                let (bx, by) = (b % 2, b / 2);
                let mut w = fdct4(&residual4::<8>(&srcc[comp], &preds[comp], bx, by));
                dc[b] = w[0];
                let n = quant4(&mut w, qpc, dz, true);
                any_ac |= n > 0;
                c.nzc[comp][b] = n as u8;
                ac[comp][b] = w;
            }
            let (l, n) = chroma_dc_quant(&dc, qpc, dz);
            any_dc |= n > 0;
            c.cdc[comp] = l;
        }
        c.cbp_chroma = if any_ac {
            2
        } else if any_dc {
            1
        } else {
            0
        };
        if !any_ac {
            c.nzc = [[0; 4]; 2];
        }
        for comp in 0..2 {
            let dcc = chroma_dc_dequant(&c.cdc[comp], qpc);
            let mut rec = [0u8; 64];
            for b in 0..4 {
                let (bx, by) = (b % 2, b / 2);
                let mut d = if any_ac { ac[comp][b] } else { [0; 16] };
                c.cac[comp][b] = to_scan(&d);
                dequant4(&mut d, qpc, true);
                d[0] = dcc[b];
                add4::<8>(&mut rec, &preds[comp], &idct4(&d), bx, by);
            }
            Self::store::<8>(&mut self.rec[1 + comp], mx, my, &rec);
        }
    }

    // ---- inter ----------------------------------------------------------

    /// (refIdx, mv) of a neighbour for prediction; `None` = unavailable.
    fn nb_motion(&self, mx: isize, my: isize) -> Option<(i32, [i32; 2])> {
        self.nb(mx, my).map(|n| {
            if n.intra {
                (-1, [0, 0])
            } else {
                (n.refi as i32, [n.mv[0] as i32, n.mv[1] as i32])
            }
        })
    }

    /// 16x16 motion vector predictor (§8.4.1.3) and the P_Skip vector
    /// (§8.4.1.1).
    fn mv_pred(&self, mx: usize, my: usize) -> ([i32; 2], [i32; 2]) {
        let (x, y) = (mx as isize, my as isize);
        let a = self.nb_motion(x - 1, y);
        let b = self.nb_motion(x, y - 1);
        let c = self
            .nb_motion(x + 1, y - 1)
            .or_else(|| self.nb_motion(x - 1, y - 1));
        let un = (-1, [0, 0]);
        let (ra, ma) = a.unwrap_or(un);
        let (mut rb, mut mb) = b.unwrap_or(un);
        let (mut rc, mut mc) = c.unwrap_or(un);
        if b.is_none() && c.is_none() && a.is_some() {
            (rb, mb) = (ra, ma);
            (rc, mc) = (ra, ma);
        }
        let matches = (ra == 0) as u32 + (rb == 0) as u32 + (rc == 0) as u32;
        let mvp = if matches == 1 {
            if ra == 0 {
                ma
            } else if rb == 0 {
                mb
            } else {
                mc
            }
        } else {
            [median(ma[0], mb[0], mc[0]), median(ma[1], mb[1], mc[1])]
        };
        let skip = match (a, b) {
            (Some((ra, ma)), Some((rb, mb))) => {
                if (ra == 0 && ma == [0, 0]) || (rb == 0 && mb == [0, 0]) {
                    [0, 0]
                } else {
                    mvp
                }
            }
            _ => [0, 0],
        };
        (mvp, skip)
    }

    #[inline]
    fn mv_bits(mv: [i32; 2], mvp: [i32; 2]) -> u32 {
        se_len(mv[0] - mvp[0]) + se_len(mv[1] - mvp[1])
    }

    /// Integer then fractional motion search around a few predictors.
    #[allow(clippy::too_many_arguments)]
    fn motion_search(
        &self,
        r: &RefPic,
        src: &[u8; 256],
        x0: usize,
        y0: usize,
        mvp: [i32; 2],
        cands: &[[i32; 2]],
        lambda: u32,
        tools: &Tools,
        mv_limit: [i32; 2],
    ) -> ([i32; 2], u32) {
        let stride = r.y.stride;
        let in_range = |m: [i32; 2]| m[0].abs() <= mv_limit[0] && m[1].abs() <= mv_limit[1];
        let cost_full = |m: [i32; 2]| -> u32 {
            let mq = [m[0] * 4, m[1] * 4];
            if !in_range(mq) || !r.luma_safe(x0, y0, mq) {
                return u32::MAX;
            }
            let off =
                r.y.idx(x0 as isize + m[0] as isize, y0 as isize + m[1] as isize);
            sad16(src, &r.y.data, off, stride) + (lambda * Self::mv_bits(mq, mvp)) / 16
        };
        let mut best = ([0i32, 0], u32::MAX);
        for &c in cands {
            let m = [(c[0] + 2) >> 2, (c[1] + 2) >> 2];
            let j = cost_full(m);
            if j < best.1 {
                best = (m, j);
            }
        }
        // Large diamond (step 2), then small diamond (step 1).
        for (step, iters) in [(2, tools.search_iters), (1, tools.search_iters)] {
            for _ in 0..iters {
                let center = best;
                for d in [[step, 0], [-step, 0], [0, step], [0, -step]] {
                    let m = [center.0[0] + d[0], center.0[1] + d[1]];
                    let j = cost_full(m);
                    if j < best.1 {
                        best = (m, j);
                    }
                }
                if best.0 == center.0 {
                    break;
                }
            }
        }
        let mut bq = [best.0[0] * 4, best.0[1] * 4];
        let mut pred = [0u8; 256];
        let cost_sub = |m: [i32; 2], pred: &mut [u8; 256]| -> u32 {
            if !in_range(m) || !r.luma_safe(x0, y0, m) {
                return u32::MAX;
            }
            r.luma_pred_fast(x0, y0, m, pred);
            satd16(src, pred) + (lambda * Self::mv_bits(m, mvp)) / 16
        };
        let mut bj = cost_sub(bq, &mut pred);
        if tools.subpel {
            for step in [2, 1] {
                let center = bq;
                for d in [
                    [-step, -step],
                    [0, -step],
                    [step, -step],
                    [-step, 0],
                    [step, 0],
                    [-step, step],
                    [0, step],
                    [step, step],
                ] {
                    let m = [center[0] + d[0], center[1] + d[1]];
                    let j = cost_sub(m, &mut pred);
                    if j < bj {
                        bj = j;
                        bq = m;
                    }
                }
            }
        }
        (bq, bj)
    }

    /// Codes the luma residual of an inter macroblock; returns the
    /// reconstruction.
    fn code_inter_luma(
        &self,
        src: &[u8; 256],
        pred: &[u8; 256],
        qp: u8,
        c: &mut Coded,
    ) -> [u8; 256] {
        let mut lv = [[0i32; 16]; 16];
        for by in 0..4 {
            for bx in 0..4 {
                let b = by * 4 + bx;
                let mut w = fdct4(&residual4::<16>(src, pred, bx, by));
                let n = quant4(&mut w, qp, Deadzone::Inter, false);
                c.nz[b] = n as u8;
                lv[b] = w;
            }
        }
        for b8 in 0..4 {
            let (ox, oy) = ((b8 % 2) * 2, (b8 / 2) * 2);
            let coded = [
                oy * 4 + ox,
                oy * 4 + ox + 1,
                (oy + 1) * 4 + ox,
                (oy + 1) * 4 + ox + 1,
            ]
            .iter()
            .any(|&b| c.nz[b] > 0);
            if coded {
                c.cbp_luma |= 1 << b8;
            }
        }
        let mut rec = *pred;
        for by in 0..4 {
            for bx in 0..4 {
                let b = by * 4 + bx;
                c.luma[b] = to_scan(&lv[b]);
                if c.nz[b] > 0 {
                    let mut d = lv[b];
                    dequant4(&mut d, qp, false);
                    add4::<16>(&mut rec, pred, &idct4(&d), bx, by);
                }
            }
        }
        rec
    }

    /// P-slice macroblock decision. `None` means P_Skip.
    fn decide_p(
        &mut self,
        f: &FrameIn<'_>,
        r: &RefPic,
        mx: usize,
        my: usize,
        qp: u8,
        lambda: u32,
    ) -> Option<Coded> {
        let src = self.src_luma(f.src, mx, my);
        let srcc = self.src_chroma(f.src, mx, my);
        let (x0, y0) = (mx * 16, (self.row0 + my) * 16);
        let (mvp, skip_mv) = self.mv_pred(mx, my);
        let mut pred = [0u8; 256];
        let mut predc = [[0u8; 64]; 2];

        // P_Skip if the skip prediction leaves nothing worth coding.
        r.luma_pred(x0, y0, skip_mv, &mut pred);
        let skip_pred = pred;
        let skip_sad = sad16(&src, &pred, 0, 16);
        let qstep = 0.625 * 2f64.powf(qp as f64 / 6.0);
        if (skip_sad as f64) < 256.0 * qstep * 0.5 {
            let mut c = Coded::new(Kind::Inter);
            let rec = self.code_inter_luma(&src, &pred, qp, &mut c);
            if c.cbp_luma == 0 {
                r.chroma_pred(x0, y0, skip_mv, &mut predc);
                let mut cc = c.clone();
                let saved = self.save_chroma(mx, my);
                self.code_chroma_residual(&srcc, &predc, mx, my, qp, Deadzone::Inter, &mut cc);
                if cc.cbp_chroma == 0 {
                    Self::store::<16>(&mut self.rec[0], mx, my, &rec);
                    self.info[my * self.mb_w + mx] = MbInfo {
                        intra: false,
                        nz: [0; 16],
                        nzc: [[0; 4]; 2],
                        i4: [intra::I4_DC; 16],
                        mv: [skip_mv[0] as i16, skip_mv[1] as i16],
                        refi: 0,
                        qp,
                    };
                    return None;
                }
                self.restore_chroma(mx, my, &saved);
            }
        }

        // Motion search.
        let mut cands = vec![mvp, [0, 0], skip_mv];
        for (dx, dy) in [(-1isize, 0isize), (0, -1), (1, -1)] {
            if let Some((0, m)) = self.nb_motion(mx as isize + dx, my as isize + dy) {
                cands.push(m);
            }
        }
        // Temporal candidates: the co-located macroblock and its right and
        // lower neighbours in the previous picture.
        for (dx, dy) in [(0usize, 0usize), (1, 0), (0, 1)] {
            let (x, y) = (mx + dx, my + dy);
            if x < self.mb_w && y < self.rows {
                let p = &self.prev[y * self.mb_w + x];
                if p.refi == 0 {
                    cands.push([p.mv[0] as i32, p.mv[1] as i32]);
                }
            }
        }
        let (mv, inter_cost) =
            self.motion_search(r, &src, x0, y0, mvp, &cands, lambda, &f.tools, f.mv_limit);

        // Inter coding.
        r.luma_pred(x0, y0, mv, &mut pred);
        r.chroma_pred(x0, y0, mv, &mut predc);
        let mut c = Coded::new(Kind::Inter);
        c.mv = mv;
        c.mvd = [mv[0] - mvp[0], mv[1] - mvp[1]];
        let rec = self.code_inter_luma(&src, &pred, qp, &mut c);
        Self::store::<16>(&mut self.rec[0], mx, my, &rec);
        self.code_chroma_residual(&srcc, &predc, mx, my, qp, Deadzone::Inter, &mut c);
        let skippable = mv == skip_mv && c.cbp_luma == 0 && c.cbp_chroma == 0;
        let idx = my * self.mb_w + mx;
        self.info[idx] = MbInfo {
            intra: false,
            nz: c.nz,
            nzc: c.nzc,
            i4: [intra::I4_DC; 16],
            mv: [mv[0] as i16, mv[1] as i16],
            refi: 0,
            qp,
        };

        // Rate-distortion checks against P_Skip (when its prediction is
        // close) and intra (when its SATD cost is in reach).
        let ls = lambda_ssd16(qp);
        let (mode16, cost16) = self.best_i16(&src, mx, my);
        let intra_bias = (lambda * 8) / 16;
        let try_intra = (cost16.saturating_add(intra_bias) as u64) * 2 < inter_cost as u64 * 3;
        let try_skip = !skippable && (skip_sad as f64) < 256.0 * qstep;
        if !try_intra && !try_skip {
            if skippable {
                self.info[idx].nz = [0; 16];
                return None;
            }
            return Some(c);
        }
        let bits = if skippable {
            1
        } else {
            self.mb_bits(&c, mx, my, true, qp)
        };
        let mut j_best =
            (self.ssd_luma(&src, mx, my) + self.ssd_chroma(&srcc, mx, my)) * 16 + ls * bits as u64;
        let mut use_skip = false;
        let mut skip_predc = [[0u8; 64]; 2];
        if try_skip {
            r.chroma_pred(x0, y0, skip_mv, &mut skip_predc);
            let mut ssd = 0u64;
            for (a, b) in src.iter().zip(&skip_pred) {
                let d = *a as i32 - *b as i32;
                ssd += (d * d) as u64;
            }
            for (sc, pc) in srcc.iter().zip(&skip_predc) {
                for (a, b) in sc.iter().zip(pc) {
                    let d = *a as i32 - *b as i32;
                    ssd += (d * d) as u64;
                }
            }
            let j_skip = ssd * 16 + ls;
            if j_skip < j_best {
                j_best = j_skip;
                use_skip = true;
            }
        }
        if try_intra {
            let inter_px = (self.save_luma(mx, my), self.save_chroma(mx, my));
            let inter_info = self.info[idx];
            let ci = self.code_intra(
                &src,
                &srcc,
                mx,
                my,
                qp,
                lambda,
                mode16,
                cost16,
                f.tools.i4x4,
                true,
            );
            let j_intra = (self.ssd_luma(&src, mx, my) + self.ssd_chroma(&srcc, mx, my)) * 16
                + ls * self.mb_bits(&ci, mx, my, true, qp) as u64;
            if j_intra < j_best {
                return Some(ci);
            }
            Self::store::<16>(&mut self.rec[0], mx, my, &inter_px.0);
            self.restore_chroma(mx, my, &inter_px.1);
            self.info[idx] = inter_info;
        }
        if use_skip {
            Self::store::<16>(&mut self.rec[0], mx, my, &skip_pred);
            self.restore_chroma(mx, my, &skip_predc);
            self.info[idx] = MbInfo {
                intra: false,
                nz: [0; 16],
                nzc: [[0; 4]; 2],
                i4: [intra::I4_DC; 16],
                mv: [skip_mv[0] as i16, skip_mv[1] as i16],
                refi: 0,
                qp,
            };
            return None;
        }
        if skippable {
            self.info[idx].nz = [0; 16];
            return None;
        }
        Some(c)
    }

    fn save_chroma(&self, mx: usize, my: usize) -> [[u8; 64]; 2] {
        let mut out = [[0u8; 64]; 2];
        for (o, p) in out.iter_mut().zip(&self.rec[1..]) {
            for r in 0..8 {
                o[r * 8..r * 8 + 8].copy_from_slice(&p.row(my * 8 + r)[mx * 8..mx * 8 + 8]);
            }
        }
        out
    }

    fn restore_chroma(&mut self, mx: usize, my: usize, s: &[[u8; 64]; 2]) {
        Self::store::<8>(&mut self.rec[1], mx, my, &s[0]);
        Self::store::<8>(&mut self.rec[2], mx, my, &s[1]);
    }

    // ---- syntax ---------------------------------------------------------

    fn nc_luma(&self, mx: usize, my: usize, nz: &[u8; 16], bx: usize, by: usize) -> i32 {
        let a = if bx > 0 {
            Some(nz[by * 4 + bx - 1])
        } else {
            self.nb(mx as isize - 1, my as isize)
                .map(|n| n.nz[by * 4 + 3])
        };
        let b = if by > 0 {
            Some(nz[(by - 1) * 4 + bx])
        } else {
            self.nb(mx as isize, my as isize - 1).map(|n| n.nz[12 + bx])
        };
        nc_of(a, b)
    }

    fn nc_chroma(&self, mx: usize, my: usize, comp: usize, nz: &[u8; 4], b: usize) -> i32 {
        let (bx, by) = (b % 2, b / 2);
        let a = if bx > 0 {
            Some(nz[by * 2])
        } else {
            self.nb(mx as isize - 1, my as isize)
                .map(|n| n.nzc[comp][by * 2 + 1])
        };
        let t = if by > 0 {
            Some(nz[bx])
        } else {
            self.nb(mx as isize, my as isize - 1)
                .map(|n| n.nzc[comp][2 + bx])
        };
        nc_of(a, t)
    }

    fn write_mb(&mut self, c: &Coded, mx: usize, my: usize, p: bool, qp: u8, qp_pred: &mut u8) {
        let mut bw = std::mem::take(&mut self.bw);
        self.write_mb_to(&mut bw, c, mx, my, p, qp, qp_pred);
        self.bw = bw;
    }

    /// Bits of a macroblock's layer (for rate-distortion decisions).
    fn mb_bits(&self, c: &Coded, mx: usize, my: usize, p: bool, qp: u8) -> u32 {
        let mut bw = BitWriter::with_capacity(512);
        let mut qp_pred = qp;
        self.write_mb_to(&mut bw, c, mx, my, p, qp, &mut qp_pred);
        bw.bit_len() as u32
    }

    #[allow(clippy::too_many_arguments)]
    fn write_mb_to(
        &self,
        bw: &mut BitWriter,
        c: &Coded,
        mx: usize,
        my: usize,
        p: bool,
        qp: u8,
        qp_pred: &mut u8,
    ) {
        let intra_off = if p { 5 } else { 0 };
        match c.kind {
            Kind::Pcm => {
                bw.ue(intra_off + 25);
                bw.align_zero();
                let y0 = my * 16;
                for r in 0..16 {
                    for &v in &self.rec[0].row(y0 + r)[mx * 16..mx * 16 + 16] {
                        bw.put(8, v as u32);
                    }
                }
                for pl in &self.rec[1..] {
                    for r in 0..8 {
                        for &v in &pl.row(my * 8 + r)[mx * 8..mx * 8 + 8] {
                            bw.put(8, v as u32);
                        }
                    }
                }
                return;
            }
            Kind::I16(mode) => {
                let t = 1
                    + mode as u32
                    + 4 * c.cbp_chroma as u32
                    + if c.cbp_luma != 0 { 12 } else { 0 };
                bw.ue(intra_off + t);
                bw.ue(c.chroma_mode as u32);
            }
            Kind::I4 => {
                bw.ue(intra_off);
                let mut modes = [intra::I4_DC; 16];
                for &(bx, by) in &BLK_XY {
                    let pm = self.pred_i4_mode(mx, my, &modes, bx, by);
                    let m = c.i4[by * 4 + bx];
                    modes[by * 4 + bx] = m;
                    if m == pm {
                        bw.flag(true);
                    } else {
                        bw.flag(false);
                        bw.put(3, if m < pm { m } else { m - 1 } as u32);
                    }
                }
                bw.ue(c.chroma_mode as u32);
            }
            Kind::Inter => {
                bw.ue(0); // P_L0_16x16
                bw.se(c.mvd[0]);
                bw.se(c.mvd[1]);
            }
        }
        let cbp = c.cbp_luma | (c.cbp_chroma << 4);
        match c.kind {
            Kind::I4 => bw.ue(CBP_INTRA_CODE[cbp as usize] as u32),
            Kind::Inter => bw.ue(CBP_INTER_CODE[cbp as usize] as u32),
            _ => {}
        }
        let is16 = matches!(c.kind, Kind::I16(_));
        if cbp != 0 || is16 {
            bw.se(qp as i32 - *qp_pred as i32);
            *qp_pred = qp;
        }
        // residual_luma
        if is16 {
            let nc = self.nc_luma(mx, my, &c.nz, 0, 0);
            write_block(bw, &c.luma_dc, nc);
        }
        for (blk, &(bx, by)) in BLK_XY.iter().enumerate() {
            if c.cbp_luma & (1 << (blk / 4)) == 0 {
                continue;
            }
            let nc = self.nc_luma(mx, my, &c.nz, bx, by);
            let b = by * 4 + bx;
            if is16 {
                write_block(bw, &c.luma[b][1..], nc);
            } else {
                write_block(bw, &c.luma[b], nc);
            }
        }
        if c.cbp_chroma != 0 {
            for comp in 0..2 {
                write_block(bw, &c.cdc[comp], -1);
            }
        }
        if c.cbp_chroma == 2 {
            for comp in 0..2 {
                for b in 0..4 {
                    let nc = self.nc_chroma(mx, my, comp, &c.nzc[comp], b);
                    write_block(bw, &c.cac[comp][b][1..], nc);
                }
            }
        }
    }
}

fn nc_of(a: Option<u8>, b: Option<u8>) -> i32 {
    match (a, b) {
        (Some(a), Some(b)) => (a as i32 + b as i32 + 1) >> 1,
        (Some(a), None) => a as i32,
        (None, Some(b)) => b as i32,
        (None, None) => 0,
    }
}
