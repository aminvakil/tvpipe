//! In-loop deblocking filter for one slice (a band of whole macroblock
//! rows). Edges with macroblocks outside the band are not filtered, which
//! matches disable_deblocking_filter_idc 0 for a single-slice picture and
//! idc 2 for several slices.
//!
//! Sources: ITU-T H.264 §8.7 (order of edges), §8.7.2.1 (bS derivation,
//! frame macroblocks), §8.7.2.2 (thresholds, qPav for chroma), §8.7.2.3
//! (bS < 4), §8.7.2.4 (bS = 4), Tables 8-16/8-17.

use super::picture::Plane;
use super::tables::{ALPHA, BETA, CHROMA_QP, TC0};
use super::MbInfo;

#[inline]
fn clip1(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Boundary strengths for the four 4-sample segments of an edge.
type Bs = [u8; 4];

fn bs_between(p: &MbInfo, pb: usize, q: &MbInfo, qb: usize, mb_edge: bool) -> u8 {
    if p.intra || q.intra {
        return if mb_edge { 4 } else { 3 };
    }
    if p.nz[pb] != 0 || q.nz[qb] != 0 {
        return 2;
    }
    if p.refi != q.refi
        || (p.mv[0] as i32 - q.mv[0] as i32).abs() >= 4
        || (p.mv[1] as i32 - q.mv[1] as i32).abs() >= 4
    {
        return 1;
    }
    0
}

struct Thresh {
    alpha: i32,
    beta: i32,
    tc0: [i32; 3],
}

fn thresh(qp_av: i32) -> Thresh {
    // FilterOffsetA/B are 0 (slice_alpha_c0_offset_div2 = 0).
    let ia = qp_av.clamp(0, 51) as usize;
    let ib = qp_av.clamp(0, 51) as usize;
    Thresh {
        alpha: ALPHA[ia] as i32,
        beta: BETA[ib] as i32,
        tc0: [TC0[ia][0] as i32, TC0[ia][1] as i32, TC0[ia][2] as i32],
    }
}

/// Filters one line of samples across an edge: `q0` is the index of the
/// first q sample, `step` the distance between successive samples across
/// the edge.
#[inline]
fn filter_line(d: &mut [u8], q0: usize, step: usize, bs: u8, t: &Thresh, chroma: bool) {
    let idx = |i: isize| (q0 as isize + i * step as isize) as usize;
    let (p0, q0v, p1, q1) = (
        d[idx(-1)] as i32,
        d[idx(0)] as i32,
        d[idx(-2)] as i32,
        d[idx(1)] as i32,
    );
    if (p0 - q0v).abs() >= t.alpha || (p1 - p0).abs() >= t.beta || (q1 - q0v).abs() >= t.beta {
        return;
    }
    if chroma {
        if bs < 4 {
            let tc = t.tc0[bs as usize - 1] + 1;
            let delta = ((((q0v - p0) << 2) + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
            d[idx(-1)] = clip1(p0 + delta);
            d[idx(0)] = clip1(q0v - delta);
        } else {
            d[idx(-1)] = ((2 * p1 + p0 + q1 + 2) >> 2) as u8;
            d[idx(0)] = ((2 * q1 + q0v + p1 + 2) >> 2) as u8;
        }
        return;
    }
    let (p2, q2) = (d[idx(-3)] as i32, d[idx(2)] as i32);
    let ap = (p2 - p0).abs();
    let aq = (q2 - q0v).abs();
    if bs < 4 {
        let tc0 = t.tc0[bs as usize - 1];
        let tc = tc0 + (ap < t.beta) as i32 + (aq < t.beta) as i32;
        let delta = ((((q0v - p0) << 2) + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
        d[idx(-1)] = clip1(p0 + delta);
        d[idx(0)] = clip1(q0v - delta);
        let avg = (p0 + q0v + 1) >> 1;
        if ap < t.beta {
            d[idx(-2)] = (p1 + ((p2 + avg - (p1 << 1)) >> 1).clamp(-tc0, tc0)) as u8;
        }
        if aq < t.beta {
            d[idx(1)] = (q1 + ((q2 + avg - (q1 << 1)) >> 1).clamp(-tc0, tc0)) as u8;
        }
    } else {
        let strong = (p0 - q0v).abs() < ((t.alpha >> 2) + 2);
        if ap < t.beta && strong {
            let p3 = d[idx(-4)] as i32;
            d[idx(-1)] = ((p2 + 2 * p1 + 2 * p0 + 2 * q0v + q1 + 4) >> 3) as u8;
            d[idx(-2)] = ((p2 + p1 + p0 + q0v + 2) >> 2) as u8;
            d[idx(-3)] = ((2 * p3 + 3 * p2 + p1 + p0 + q0v + 4) >> 3) as u8;
        } else {
            d[idx(-1)] = ((2 * p1 + p0 + q1 + 2) >> 2) as u8;
        }
        if aq < t.beta && strong {
            let q3 = d[idx(3)] as i32;
            d[idx(0)] = ((p1 + 2 * p0 + 2 * q0v + 2 * q1 + q2 + 4) >> 3) as u8;
            d[idx(1)] = ((p0 + q0v + q1 + q2 + 2) >> 2) as u8;
            d[idx(2)] = ((2 * q3 + 3 * q2 + q1 + q0v + p0 + 4) >> 3) as u8;
        } else {
            d[idx(0)] = ((2 * q1 + q0v + p1 + 2) >> 2) as u8;
        }
    }
}

/// Deblocks a band: `rec` = [Y, Cb, Cr] planes of the band (pad 0),
/// `info` its macroblocks (row-major, `mb_w` per row).
pub fn deblock_band(rec: &mut [Plane; 3], info: &[MbInfo], mb_w: usize) {
    let rows = info.len() / mb_w.max(1);
    for my in 0..rows {
        for mx in 0..mb_w {
            let q = &info[my * mb_w + mx];
            let left = (mx > 0).then(|| &info[my * mb_w + mx - 1]);
            let top = (my > 0).then(|| &info[(my - 1) * mb_w + mx]);
            // bS for vertical edges (e = 0..3) and horizontal edges.
            let mut bsv = [[0u8; 4]; 4];
            let mut bsh = [[0u8; 4]; 4];
            for e in 0..4 {
                for k in 0..4 {
                    bsv[e][k] = if e == 0 {
                        match left {
                            Some(p) => bs_between(p, k * 4 + 3, q, k * 4, true),
                            None => 0,
                        }
                    } else {
                        bs_between(q, k * 4 + e - 1, q, k * 4 + e, false)
                    };
                    bsh[e][k] = if e == 0 {
                        match top {
                            Some(p) => bs_between(p, 12 + k, q, k, true),
                            None => 0,
                        }
                    } else {
                        bs_between(q, (e - 1) * 4 + k, q, e * 4 + k, false)
                    };
                }
            }
            let qp_l = left.map(|p| p.qp as i32).unwrap_or(0);
            let qp_t = top.map(|p| p.qp as i32).unwrap_or(0);
            let qq = q.qp as i32;
            // Luma.
            {
                let y = &mut rec[0];
                let s = y.stride;
                let base = (my * 16) * s + mx * 16;
                for (e, bs) in bsv.iter().enumerate() {
                    if bs.iter().all(|&b| b == 0) {
                        continue;
                    }
                    let qp_p = if e == 0 { qp_l } else { qq };
                    let t = thresh((qp_p + qq + 1) >> 1);
                    for r in 0..16 {
                        let b = bs[r / 4];
                        if b != 0 {
                            filter_line(&mut y.data, base + r * s + e * 4, 1, b, &t, false);
                        }
                    }
                }
                for (e, bs) in bsh.iter().enumerate() {
                    if bs.iter().all(|&b| b == 0) {
                        continue;
                    }
                    let qp_p = if e == 0 { qp_t } else { qq };
                    let t = thresh((qp_p + qq + 1) >> 1);
                    for c in 0..16 {
                        let b = bs[c / 4];
                        if b != 0 {
                            filter_line(&mut y.data, base + e * 4 * s + c, s, b, &t, false);
                        }
                    }
                }
            }
            // Chroma: edges 0 and 2 of luma map to chroma edges 0 and 4.
            let qc = |qp: i32| CHROMA_QP[qp.clamp(0, 51) as usize] as i32;
            for plane in rec[1..].iter_mut() {
                let s = plane.stride;
                let base = (my * 8) * s + mx * 8;
                for (ce, e) in [(0usize, 0usize), (1, 2)] {
                    let bs: Bs = bsv[e];
                    if bs.iter().all(|&b| b == 0) {
                        continue;
                    }
                    let qp_p = if e == 0 { qp_l } else { qq };
                    let t = thresh((qc(qp_p) + qc(qq) + 1) >> 1);
                    for r in 0..8 {
                        let b = bs[r / 2];
                        if b != 0 {
                            filter_line(&mut plane.data, base + r * s + ce * 4, 1, b, &t, true);
                        }
                    }
                }
                for (ce, e) in [(0usize, 0usize), (1, 2)] {
                    let bs: Bs = bsh[e];
                    if bs.iter().all(|&b| b == 0) {
                        continue;
                    }
                    let qp_p = if e == 0 { qp_t } else { qq };
                    let t = thresh((qc(qp_p) + qc(qq) + 1) >> 1);
                    for c in 0..8 {
                        let b = bs[c / 2];
                        if b != 0 {
                            filter_line(&mut plane.data, base + ce * 4 * s + c, s, b, &t, true);
                        }
                    }
                }
            }
        }
    }
}
