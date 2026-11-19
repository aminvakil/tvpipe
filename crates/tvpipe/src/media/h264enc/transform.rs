//! 4x4 integer transform, Hadamard DC transforms, quantisation and the
//! decoder's scaling + inverse transform (which the encoder must mirror
//! bit-exactly for its reference pictures).
//!
//! All 4x4 arrays are raster order: index = row * 4 + column, row = vertical
//! frequency / y.
//!
//! Sources: ITU-T H.264 §8.5.12.1 (scaling, flat weights), §8.5.12.2
//! (inverse transform), §8.5.10 (Intra16x16 DC), §8.5.11.1/§8.5.11.2
//! (chroma DC, 4:2:0); I. Richardson, "The H.264 Advanced Video Compression
//! Standard", 2nd ed., ch. 7 (forward transform and quantiser design).

use super::tables::{DEQUANT_V, POS_CLASS, QUANT_MF};

/// Largest level magnitude we emit; always representable with CAVLC
/// level_prefix ≤ 15 (Baseline).
pub const MAX_LEVEL: i32 = 2047;

/// Forward core transform W = Cf · X · Cfᵀ.
#[inline]
pub fn fdct4(x: &[i32; 16]) -> [i32; 16] {
    let mut t = [0i32; 16];
    for i in 0..4 {
        let r = &x[i * 4..i * 4 + 4];
        let s0 = r[0] + r[3];
        let s1 = r[1] + r[2];
        let d0 = r[0] - r[3];
        let d1 = r[1] - r[2];
        t[i * 4] = s0 + s1;
        t[i * 4 + 1] = 2 * d0 + d1;
        t[i * 4 + 2] = s0 - s1;
        t[i * 4 + 3] = d0 - 2 * d1;
    }
    let mut w = [0i32; 16];
    for j in 0..4 {
        let s0 = t[j] + t[12 + j];
        let s1 = t[4 + j] + t[8 + j];
        let d0 = t[j] - t[12 + j];
        let d1 = t[4 + j] - t[8 + j];
        w[j] = s0 + s1;
        w[4 + j] = 2 * d0 + d1;
        w[8 + j] = s0 - s1;
        w[12 + j] = d0 - 2 * d1;
    }
    w
}

/// Inverse transform of scaled coefficients `d` (§8.5.12.2), including the
/// final `(x + 32) >> 6`: rows first, then columns.
#[inline]
pub fn idct4(d: &[i32; 16]) -> [i32; 16] {
    let mut f = [0i32; 16];
    for i in 0..4 {
        let r = &d[i * 4..i * 4 + 4];
        let e0 = r[0] + r[2];
        let e1 = r[0] - r[2];
        let e2 = (r[1] >> 1) - r[3];
        let e3 = r[1] + (r[3] >> 1);
        f[i * 4] = e0 + e3;
        f[i * 4 + 1] = e1 + e2;
        f[i * 4 + 2] = e1 - e2;
        f[i * 4 + 3] = e0 - e3;
    }
    let mut out = [0i32; 16];
    for j in 0..4 {
        let g0 = f[j] + f[8 + j];
        let g1 = f[j] - f[8 + j];
        let g2 = (f[4 + j] >> 1) - f[12 + j];
        let g3 = f[4 + j] + (f[12 + j] >> 1);
        out[j] = (g0 + g3 + 32) >> 6;
        out[4 + j] = (g1 + g2 + 32) >> 6;
        out[8 + j] = (g1 - g2 + 32) >> 6;
        out[12 + j] = (g0 - g3 + 32) >> 6;
    }
    out
}

/// Deadzone rounding offsets, as a fraction of the quantiser step: 1/3 for
/// intra, 1/6 for inter blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deadzone {
    Intra,
    Inter,
}

#[inline]
fn round_offset(qbits: u32, dz: Deadzone) -> i32 {
    match dz {
        Deadzone::Intra => ((1i64 << qbits) / 3) as i32,
        Deadzone::Inter => ((1i64 << qbits) / 6) as i32,
    }
}

/// Quantises a transformed block in place into levels (raster). With
/// `ac_only` the DC position is left for the separate DC transform and
/// set to 0. Returns the number of non-zero levels.
#[inline]
pub fn quant4(w: &mut [i32; 16], qp: u8, dz: Deadzone, ac_only: bool) -> u32 {
    let qbits = 15 + (qp / 6) as u32;
    let f = round_offset(qbits, dz);
    let mf = &QUANT_MF[(qp % 6) as usize];
    let mut nnz = 0;
    for (i, c) in w.iter_mut().enumerate() {
        if ac_only && i == 0 {
            *c = 0;
            continue;
        }
        let a = c.abs();
        let l = (((a as i64) * mf[POS_CLASS[i]] as i64 + f as i64) >> qbits) as i32;
        let l = l.min(MAX_LEVEL);
        *c = if *c < 0 { -l } else { l };
        nnz += (l != 0) as u32;
    }
    nnz
}

/// Scales levels (raster) in place for the inverse transform (§8.5.12.1
/// with flat weights: `c · v << (qp / 6)`). With `skip_dc` position 0 is
/// left untouched (it already holds the scaled DC).
#[inline]
pub fn dequant4(c: &mut [i32; 16], qp: u8, skip_dc: bool) {
    let v = &DEQUANT_V[(qp % 6) as usize];
    let shift = (qp / 6) as u32;
    for (i, x) in c.iter_mut().enumerate() {
        if skip_dc && i == 0 {
            continue;
        }
        *x = (*x * v[POS_CLASS[i]]) << shift;
    }
}

/// 4x4 Hadamard H · c · H with H = [[1,1,1,1],[1,1,-1,-1],[1,-1,-1,1],
/// [1,-1,1,-1]] (§8.5.10).
#[inline]
pub fn hadamard4(c: &[i32; 16]) -> [i32; 16] {
    let mut t = [0i32; 16];
    for i in 0..4 {
        let r = &c[i * 4..i * 4 + 4];
        let s01 = r[0] + r[1];
        let d01 = r[0] - r[1];
        let s23 = r[2] + r[3];
        let d23 = r[2] - r[3];
        t[i * 4] = s01 + s23;
        t[i * 4 + 1] = s01 - s23;
        t[i * 4 + 2] = d01 - d23;
        t[i * 4 + 3] = d01 + d23;
    }
    let mut f = [0i32; 16];
    for j in 0..4 {
        let s01 = t[j] + t[4 + j];
        let d01 = t[j] - t[4 + j];
        let s23 = t[8 + j] + t[12 + j];
        let d23 = t[8 + j] - t[12 + j];
        f[j] = s01 + s23;
        f[4 + j] = s01 - s23;
        f[8 + j] = d01 - d23;
        f[12 + j] = d01 + d23;
    }
    f
}

/// Forward Intra16x16 DC: `dc` holds W₀₀ of the 16 blocks (raster by block
/// position); returns quantised levels (raster) and their count.
pub fn luma_dc_quant(dc: &[i32; 16], qp: u8) -> ([i32; 16], u32) {
    let y = hadamard4(dc);
    let qbits = 16 + (qp / 6) as u32;
    let f = 2 * round_offset(qbits - 1, Deadzone::Intra);
    let mf = QUANT_MF[(qp % 6) as usize][0] as i64;
    let mut out = [0i32; 16];
    let mut nnz = 0;
    for i in 0..16 {
        let a = (y[i] / 2).abs() as i64;
        let l = (((a * mf + f as i64) >> qbits) as i32).min(MAX_LEVEL);
        out[i] = if y[i] < 0 { -l } else { l };
        nnz += (l != 0) as u32;
    }
    (out, nnz)
}

/// Decoder side of Intra16x16 DC (§8.5.10): levels (raster) → dcY (raster
/// by block position), ready to be placed at position 0 of each block.
pub fn luma_dc_dequant(levels: &[i32; 16], qp: u8) -> [i32; 16] {
    let f = hadamard4(levels);
    let ls = 16 * DEQUANT_V[(qp % 6) as usize][0];
    let q6 = (qp / 6) as u32;
    let mut out = [0i32; 16];
    for i in 0..16 {
        out[i] = if q6 >= 6 {
            (f[i] * ls) << (q6 - 6)
        } else {
            (f[i] * ls + (1 << (5 - q6))) >> (6 - q6)
        };
    }
    out
}

/// 2x2 Hadamard [[1,1],[1,-1]] · c · [[1,1],[1,-1]] (raster).
#[inline]
pub fn hadamard2(c: &[i32; 4]) -> [i32; 4] {
    let a = c[0] + c[1];
    let b = c[0] - c[1];
    let d = c[2] + c[3];
    let e = c[2] - c[3];
    [a + d, b + e, a - d, b - e]
}

/// Forward chroma DC (4:2:0): W₀₀ of the four 4x4 blocks → levels.
pub fn chroma_dc_quant(dc: &[i32; 4], qpc: u8, dz: Deadzone) -> ([i32; 4], u32) {
    let y = hadamard2(dc);
    let qbits = 16 + (qpc / 6) as u32;
    let f = 2 * round_offset(qbits - 1, dz);
    let mf = QUANT_MF[(qpc % 6) as usize][0] as i64;
    let mut out = [0i32; 4];
    let mut nnz = 0;
    for i in 0..4 {
        let a = y[i].abs() as i64;
        let l = (((a * mf + f as i64) >> qbits) as i32).min(MAX_LEVEL);
        out[i] = if y[i] < 0 { -l } else { l };
        nnz += (l != 0) as u32;
    }
    (out, nnz)
}

/// Decoder side of chroma DC (§8.5.11.2, 4:2:0).
pub fn chroma_dc_dequant(levels: &[i32; 4], qpc: u8) -> [i32; 4] {
    let f = hadamard2(levels);
    let ls = 16 * DEQUANT_V[(qpc % 6) as usize][0];
    let q6 = (qpc / 6) as u32;
    let mut out = [0i32; 4];
    for i in 0..4 {
        out[i] = ((f[i] * ls) << q6) >> 5;
    }
    out
}

/// Sum of absolute Hadamard-transformed differences of a 4x4 block, halved.
#[inline]
pub fn satd4(d: &[i32; 16]) -> u32 {
    let h = hadamard4(d);
    let s: u32 = h.iter().map(|v| v.unsigned_abs()).sum();
    s.div_ceil(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_roundtrip_is_close() {
        // With qp 0-ish scaling the quantise/rescale/inverse chain must
        // reproduce the residual within ±1.
        let x: [i32; 16] = [5, -3, 0, 12, 7, 7, -8, 1, 0, 0, 3, 4, -20, 15, 9, -1];
        let mut w = fdct4(&x);
        quant4(&mut w, 4, Deadzone::Intra, false);
        dequant4(&mut w, 4, false);
        let r = idct4(&w);
        for i in 0..16 {
            assert!((r[i] - x[i]).abs() <= 1, "{i}: {} vs {}", r[i], x[i]);
        }
    }

    #[test]
    fn idct_known_answer() {
        // A lone DC level of 1 at qp 28 (v=16, shift 4): d00 = 256,
        // (256 + 32) >> 6 = 4 everywhere.
        let mut c = [0i32; 16];
        c[0] = 1;
        dequant4(&mut c, 28, false);
        assert_eq!(c[0], 256);
        assert_eq!(idct4(&c), [4; 16]);
        // d01 = 64: rows: e0 = 0, e2 = 32, e3 = 64 → f = [64, 32, -32, -64]
        // then (x + 32) >> 6 per column.
        let mut d = [0i32; 16];
        d[1] = 64;
        let r = idct4(&d);
        assert_eq!(&r[0..4], &[1, 1, 0, -1]);
        assert_eq!(&r[12..16], &[1, 1, 0, -1]);
    }

    #[test]
    fn dc_transforms_invert() {
        let dc: [i32; 16] = core::array::from_fn(|i| (i as i32 * 37 % 23 - 11) * 16);
        let (lv, _) = luma_dc_quant(&dc, 10);
        let back = luma_dc_dequant(&lv, 10);
        // back ≈ dc · 64 / 16 scale of the 4x4 DC path: compare by idct.
        for i in 0..16 {
            let mut blk = [0i32; 16];
            blk[0] = back[i];
            let r = idct4(&blk)[0];
            let want = (dc[i] as f64 / 16.0).round() as i32;
            assert!((r - want).abs() <= 1, "{i}: {r} vs {want}");
        }
        let cdc = [160, -48, 32, 0];
        let (lv, _) = chroma_dc_quant(&cdc, 12, Deadzone::Intra);
        let back = chroma_dc_dequant(&lv, 12);
        for i in 0..4 {
            let mut blk = [0i32; 16];
            blk[0] = back[i];
            let r = idct4(&blk)[0];
            let want = (cdc[i] as f64 / 16.0).round() as i32;
            assert!((r - want).abs() <= 1, "{i}: {r} vs {want}");
        }
    }
}
