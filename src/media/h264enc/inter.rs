//! Reference pictures, luma/chroma sample interpolation and motion search.
//!
//! The reference keeps its full-sample planes edge-extended plus three
//! precomputed half-sample planes (b, h, j of Figure 8-4), so that any
//! quarter-sample position is a copy or a rounded average of two planes.
//! Motion vectors whose block would read outside the padding use the
//! direct per-sample formulas with clamped coordinates instead; both give
//! identical results (tested).
//!
//! Sources: ITU-T H.264 §8.4.2.2.1 (luma sample interpolation, Eqs.
//! 8-241..8-261, Table 8-12), §8.4.2.2.2 (chroma, Eq. 8-266), §8.4.1.3
//! (motion vector prediction, used by the caller).

use super::picture::Plane;

pub const LUMA_PAD: usize = 32;
pub const CHROMA_PAD: usize = 16;

#[inline]
fn clip(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

#[inline]
fn tap(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32) -> i32 {
    a - 5 * b + 20 * c + 20 * d - 5 * e + f
}

/// A plane and an offset into it.
type Src<'a> = (&'a [u8], usize);

/// A decoded (deblocked) picture used for inter prediction.
#[derive(Debug, Clone)]
pub struct RefPic {
    pub y: Plane,
    pub u: Plane,
    pub v: Plane,
    /// b: half sample right of (x, y).
    pub hb: Plane,
    /// h: half sample below (x, y).
    pub hh: Plane,
    /// j: half sample right and below (x, y).
    pub hj: Plane,
}

impl RefPic {
    pub fn new(w: usize, h: usize) -> RefPic {
        RefPic {
            y: Plane::new(w, h, LUMA_PAD),
            u: Plane::new(w / 2, h / 2, CHROMA_PAD),
            v: Plane::new(w / 2, h / 2, CHROMA_PAD),
            hb: Plane::new(w, h, LUMA_PAD),
            hh: Plane::new(w, h, LUMA_PAD),
            hj: Plane::new(w, h, LUMA_PAD),
        }
    }

    /// Rows (in padded coordinates, i.e. 0..h+2·pad) for which the half
    /// planes are computed.
    pub fn half_rows(&self) -> std::ops::Range<usize> {
        3..self.y.h + 2 * LUMA_PAD - 3
    }

    /// Computes the half-sample planes for padded rows `rows` into the
    /// matching row chunks of `hb`, `hh`, `hj` (each `rows.len() · stride`
    /// long). Needs `y` with extended edges.
    pub fn half_rows_into(
        y: &Plane,
        rows: std::ops::Range<usize>,
        hb: &mut [u8],
        hh: &mut [u8],
        hj: &mut [u8],
    ) {
        let s = y.stride;
        let d = &y.data;
        let mut h1 = vec![0i32; s];
        for (k, py) in rows.enumerate() {
            if py < 3 || py + 3 >= y.h + 2 * y.pad {
                continue;
            }
            let o = k * s;
            // Vertical 6-tap (unclipped) for every column.
            for x in 0..s {
                let c = |dy: isize| d[((py as isize + dy) as usize) * s + x] as i32;
                h1[x] = tap(c(-2), c(-1), c(0), c(1), c(2), c(3));
            }
            let row = &d[py * s..py * s + s];
            for x in 2..s - 3 {
                hb[o + x] = clip(
                    (tap(
                        row[x - 2] as i32,
                        row[x - 1] as i32,
                        row[x] as i32,
                        row[x + 1] as i32,
                        row[x + 2] as i32,
                        row[x + 3] as i32,
                    ) + 16)
                        >> 5,
                );
                hj[o + x] = clip(
                    (tap(h1[x - 2], h1[x - 1], h1[x], h1[x + 1], h1[x + 2], h1[x + 3]) + 512) >> 10,
                );
            }
            for x in 0..s {
                hh[o + x] = clip((h1[x] + 16) >> 5);
            }
        }
    }

    /// Single-threaded half-plane build.
    pub fn build_half(&mut self) {
        let rows = 0..self.y.h + 2 * LUMA_PAD;
        RefPic::half_rows_into(
            &self.y,
            rows,
            &mut self.hb.data,
            &mut self.hh.data,
            &mut self.hj.data,
        );
    }

    /// Whether a 16x16 luma block at (x0, y0) with quarter-sample vector
    /// `mv` stays within the precomputed planes.
    #[inline]
    pub fn luma_safe(&self, x0: usize, y0: usize, mv: [i32; 2]) -> bool {
        let lo = -(LUMA_PAD as isize) + 3;
        let x = x0 as isize + (mv[0] >> 2) as isize;
        let y = y0 as isize + (mv[1] >> 2) as isize;
        x >= lo
            && y >= lo
            && x + 17 <= (self.y.w + LUMA_PAD) as isize - 4
            && y + 17 <= (self.y.h + LUMA_PAD) as isize - 4
    }

    /// 16x16 luma prediction (raster) for a block at (x0, y0).
    pub fn luma_pred(&self, x0: usize, y0: usize, mv: [i32; 2], out: &mut [u8; 256]) {
        if self.luma_safe(x0, y0, mv) {
            self.luma_pred_fast(x0, y0, mv, out);
        } else {
            luma_pred_slow(&self.y, x0, y0, mv, out);
        }
    }

    /// Plane-based prediction; `luma_safe` must hold.
    pub fn luma_pred_fast(&self, x0: usize, y0: usize, mv: [i32; 2], out: &mut [u8; 256]) {
        let x = x0 as isize + (mv[0] >> 2) as isize;
        let y = y0 as isize + (mv[1] >> 2) as isize;
        let (fx, fy) = (mv[0] & 3, mv[1] & 3);
        let o = self.y.idx(x, y);
        let s = self.y.stride;
        let (g, b, h, j) = (&self.y.data, &self.hb.data, &self.hh.data, &self.hj.data);
        // (plane, offset) of the one or two contributing samples.
        let (a, b2): (Src<'_>, Option<Src<'_>>) = match (fx, fy) {
            (0, 0) => ((g, o), None),
            (1, 0) => ((g, o), Some((b, o))),
            (2, 0) => ((b, o), None),
            (3, 0) => ((g, o + 1), Some((b, o))),
            (0, 1) => ((g, o), Some((h, o))),
            (0, 2) => ((h, o), None),
            (0, 3) => ((g, o + s), Some((h, o))),
            (1, 1) => ((b, o), Some((h, o))),
            (3, 1) => ((b, o), Some((h, o + 1))),
            (1, 3) => ((h, o), Some((b, o + s))),
            (3, 3) => ((h, o + 1), Some((b, o + s))),
            (2, 1) => ((b, o), Some((j, o))),
            (2, 3) => ((j, o), Some((b, o + s))),
            (1, 2) => ((h, o), Some((j, o))),
            (3, 2) => ((j, o), Some((h, o + 1))),
            _ => ((j, o), None),
        };
        match b2 {
            None => {
                for r in 0..16 {
                    let src = &a.0[a.1 + r * s..a.1 + r * s + 16];
                    out[r * 16..r * 16 + 16].copy_from_slice(src);
                }
            }
            Some(b2) => {
                for r in 0..16 {
                    let p = &a.0[a.1 + r * s..a.1 + r * s + 16];
                    let q = &b2.0[b2.1 + r * s..b2.1 + r * s + 16];
                    let dst = &mut out[r * 16..r * 16 + 16];
                    for i in 0..16 {
                        dst[i] = ((p[i] as u16 + q[i] as u16 + 1) >> 1) as u8;
                    }
                }
            }
        }
    }

    /// 8x8 chroma predictions (Cb, Cr) for the macroblock at luma (x0, y0).
    pub fn chroma_pred(&self, x0: usize, y0: usize, mv: [i32; 2], out: &mut [[u8; 64]; 2]) {
        chroma_pred_plane(&self.u, x0 / 2, y0 / 2, mv, &mut out[0]);
        chroma_pred_plane(&self.v, x0 / 2, y0 / 2, mv, &mut out[1]);
    }
}

/// Chroma interpolation (Eq. 8-266); the vector is in 1/8 chroma samples.
fn chroma_pred_plane(p: &Plane, cx: usize, cy: usize, mv: [i32; 2], out: &mut [u8; 64]) {
    let x = cx as isize + (mv[0] >> 3) as isize;
    let y = cy as isize + (mv[1] >> 3) as isize;
    let (fx, fy) = (mv[0] & 7, mv[1] & 7);
    let w00 = (8 - fx) * (8 - fy);
    let w10 = fx * (8 - fy);
    let w01 = (8 - fx) * fy;
    let w11 = fx * fy;
    let pad = p.pad as isize;
    let inside =
        x >= -pad && y >= -pad && x + 9 <= p.w as isize + pad && y + 9 <= p.h as isize + pad;
    if inside {
        let s = p.stride;
        let o = p.idx(x, y);
        let d = &p.data;
        for r in 0..8 {
            let r0 = &d[o + r * s..o + r * s + 9];
            let r1 = &d[o + (r + 1) * s..o + (r + 1) * s + 9];
            for c in 0..8 {
                let v = w00 * r0[c] as i32
                    + w10 * r0[c + 1] as i32
                    + w01 * r1[c] as i32
                    + w11 * r1[c + 1] as i32;
                out[r * 8 + c] = ((v + 32) >> 6) as u8;
            }
        }
    } else {
        for r in 0..8isize {
            for c in 0..8isize {
                let s = |dx: isize, dy: isize| p.clamped(x + c + dx, y + r + dy) as i32;
                let v = w00 * s(0, 0) + w10 * s(1, 0) + w01 * s(0, 1) + w11 * s(1, 1);
                out[(r * 8 + c) as usize] = ((v + 32) >> 6) as u8;
            }
        }
    }
}

/// Per-sample luma interpolation with clamped reference coordinates,
/// straight from the equations (reference implementation and fallback).
pub fn luma_pred_slow(p: &Plane, x0: usize, y0: usize, mv: [i32; 2], out: &mut [u8; 256]) {
    let (fx, fy) = (mv[0] & 3, mv[1] & 3);
    for r in 0..16 {
        for c in 0..16 {
            let xi = x0 as isize + c as isize + (mv[0] >> 2) as isize;
            let yi = y0 as isize + r as isize + (mv[1] >> 2) as isize;
            out[r * 16 + c] = luma_sample(p, xi, yi, fx, fy);
        }
    }
}

fn luma_sample(p: &Plane, x: isize, y: isize, fx: i32, fy: i32) -> u8 {
    let g = |dx: isize, dy: isize| p.clamped(x + dx, y + dy) as i32;
    let b1 = |dx: isize, dy: isize| {
        tap(
            g(dx - 2, dy),
            g(dx - 1, dy),
            g(dx, dy),
            g(dx + 1, dy),
            g(dx + 2, dy),
            g(dx + 3, dy),
        )
    };
    let h1 = |dx: isize, dy: isize| {
        tap(
            g(dx, dy - 2),
            g(dx, dy - 1),
            g(dx, dy),
            g(dx, dy + 1),
            g(dx, dy + 2),
            g(dx, dy + 3),
        )
    };
    let b = |dx, dy| clip((b1(dx, dy) + 16) >> 5) as i32;
    let h = |dx, dy| clip((h1(dx, dy) + 16) >> 5) as i32;
    let j = || {
        let j1 = tap(b1(0, -2), b1(0, -1), b1(0, 0), b1(0, 1), b1(0, 2), b1(0, 3));
        clip((j1 + 512) >> 10) as i32
    };
    let avg = |a: i32, b: i32| ((a + b + 1) >> 1) as u8;
    match (fx, fy) {
        (0, 0) => g(0, 0) as u8,
        (1, 0) => avg(g(0, 0), b(0, 0)),
        (2, 0) => b(0, 0) as u8,
        (3, 0) => avg(g(1, 0), b(0, 0)),
        (0, 1) => avg(g(0, 0), h(0, 0)),
        (0, 2) => h(0, 0) as u8,
        (0, 3) => avg(g(0, 1), h(0, 0)),
        (1, 1) => avg(b(0, 0), h(0, 0)),
        (3, 1) => avg(b(0, 0), h(1, 0)),
        (1, 3) => avg(h(0, 0), b(0, 1)),
        (3, 3) => avg(h(1, 0), b(0, 1)),
        (2, 1) => avg(b(0, 0), j()),
        (2, 3) => avg(j(), b(0, 1)),
        (1, 2) => avg(h(0, 0), j()),
        (3, 2) => avg(j(), h(1, 0)),
        _ => j() as u8,
    }
}

/// Sum of absolute differences between a 16x16 source block and the
/// reference at `off`.
#[inline]
pub fn sad16(src: &[u8; 256], r: &[u8], off: usize, stride: usize) -> u32 {
    let mut s = 0u32;
    for row in 0..16 {
        let a = &src[row * 16..row * 16 + 16];
        let b = &r[off + row * stride..off + row * stride + 16];
        let mut acc = 0u16;
        for i in 0..16 {
            acc += a[i].abs_diff(b[i]) as u16;
        }
        s += acc as u32;
    }
    s
}

/// SATD of a 16x16 block against a prediction: the sum over its 4x4
/// blocks of the absolute 4x4 Hadamard coefficients, halved.
///
/// Rows are transformed first (groups of four columns), then the vertical
/// stage uses |a + b| + |a − b| = 2 · max(|a|, |b|), which also absorbs
/// the halving; that stage runs over all 16 columns at once. Values stay
/// within ±4080, so 16-bit lanes suffice.
#[inline]
pub fn satd16(src: &[u8; 256], pred: &[u8; 256]) -> u32 {
    let mut sum = 0u32;
    for strip in 0..4 {
        let mut t = [[0i16; 16]; 4];
        for (r, tr) in t.iter_mut().enumerate() {
            let o = (strip * 4 + r) * 16;
            let a = &src[o..o + 16];
            let b = &pred[o..o + 16];
            let mut d = [0i16; 16];
            for x in 0..16 {
                d[x] = a[x] as i16 - b[x] as i16;
            }
            for g in 0..4 {
                let p = &d[g * 4..g * 4 + 4];
                let s01 = p[0] + p[1];
                let d01 = p[0] - p[1];
                let s23 = p[2] + p[3];
                let d23 = p[2] - p[3];
                tr[g * 4] = s01 + s23;
                tr[g * 4 + 1] = s01 - s23;
                tr[g * 4 + 2] = d01 - d23;
                tr[g * 4 + 3] = d01 + d23;
            }
        }
        let mut acc = [0u16; 16];
        for x in 0..16 {
            let s01 = t[0][x] + t[1][x];
            let d01 = t[0][x] - t[1][x];
            let s23 = t[2][x] + t[3][x];
            let d23 = t[2][x] - t[3][x];
            acc[x] = s01.unsigned_abs().max(s23.unsigned_abs())
                + d01.unsigned_abs().max(d23.unsigned_abs());
        }
        sum += acc.iter().map(|&v| v as u32).sum::<u32>();
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise_ref(w: usize, h: usize) -> RefPic {
        let mut r = RefPic::new(w, h);
        let mut s = 12345u32;
        for y in 0..h {
            for x in 0..w {
                s = s.wrapping_mul(1103515245).wrapping_add(12345);
                let v = ((x * 7 + y * 3) as u32 + (s >> 24)) as u8;
                r.y.row_mut(y)[x] = v;
            }
        }
        r.y.extend_edges();
        r.build_half();
        r
    }

    #[test]
    fn satd16_matches_satd4() {
        let src: [u8; 256] = core::array::from_fn(|i| (i * 37 % 251) as u8);
        let pred: [u8; 256] = core::array::from_fn(|i| (i * 11 % 241) as u8);
        let mut want = 0;
        for by in 0..4 {
            for bx in 0..4 {
                let d: [i32; 16] = core::array::from_fn(|k| {
                    let i = (by * 4 + k / 4) * 16 + bx * 4 + k % 4;
                    src[i] as i32 - pred[i] as i32
                });
                let h = super::super::transform::hadamard4(&d);
                want += h.iter().map(|v| v.unsigned_abs()).sum::<u32>();
            }
        }
        assert_eq!(satd16(&src, &pred), want / 2);
    }

    #[test]
    fn fast_matches_slow() {
        let r = noise_ref(64, 48);
        let mut a = [0u8; 256];
        let mut b = [0u8; 256];
        for mvy in -12..12 {
            for mvx in -12..12 {
                for (x0, y0) in [(0, 0), (16, 16), (48, 32)] {
                    let mv = [mvx * 3 + (mvx & 3), mvy * 5 - (mvy & 1)];
                    if !r.luma_safe(x0, y0, mv) {
                        continue;
                    }
                    r.luma_pred_fast(x0, y0, mv, &mut a);
                    luma_pred_slow(&r.y, x0, y0, mv, &mut b);
                    assert_eq!(a, b, "{x0},{y0} {mv:?}");
                }
            }
        }
        // Far outside: the slow path is used and clamps.
        r.luma_pred(0, 0, [-4000, 77], &mut a);
        luma_pred_slow(&r.y, 0, 0, [-4000, 77], &mut b);
        assert_eq!(a, b);
    }
}
