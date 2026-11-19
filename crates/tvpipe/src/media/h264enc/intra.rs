//! Intra prediction: Intra_16x16 (4 modes), Intra_4x4 (9 modes) and chroma
//! (4 modes, 4:2:0).
//!
//! Sources: ITU-T H.264 §8.3.1.2 (Intra_4x4 modes, Eqs. 8-39..8-73),
//! §8.3.3 (Intra_16x16), §8.3.4 (chroma).

#[inline]
fn clip(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Neighbouring samples of an NxN block (N = 16 or 8).
#[derive(Clone, Copy)]
pub struct Edges<const N: usize> {
    pub top: Option<[u8; N]>,
    pub left: Option<[u8; N]>,
    /// p[-1, -1]; only used when both top and left are available.
    pub top_left: u8,
}

/// Intra_16x16 modes.
pub const I16_V: u8 = 0;
pub const I16_H: u8 = 1;
pub const I16_DC: u8 = 2;
pub const I16_PLANE: u8 = 3;

/// Chroma modes (note the different numbering).
pub const C_DC: u8 = 0;
pub const C_H: u8 = 1;
pub const C_V: u8 = 2;
pub const C_PLANE: u8 = 3;

/// Whether an Intra_16x16 mode can be used with these neighbours.
pub fn i16_available(mode: u8, e: &Edges<16>) -> bool {
    match mode {
        I16_V => e.top.is_some(),
        I16_H => e.left.is_some(),
        I16_DC => true,
        _ => e.top.is_some() && e.left.is_some(),
    }
}

/// Intra_16x16 prediction (raster 16x16).
pub fn pred16(mode: u8, e: &Edges<16>, out: &mut [u8; 256]) {
    match (mode, e.top, e.left) {
        (I16_V, Some(t), _) => {
            for row in out.chunks_exact_mut(16) {
                row.copy_from_slice(&t);
            }
        }
        (I16_H, _, Some(l)) => {
            for (row, &v) in out.chunks_exact_mut(16).zip(l.iter()) {
                row.fill(v);
            }
        }
        (I16_PLANE, Some(t), Some(l)) => {
            let at = |i: i32| -> i32 {
                if i < 0 {
                    e.top_left as i32
                } else {
                    t[i as usize] as i32
                }
            };
            let al = |i: i32| -> i32 {
                if i < 0 {
                    e.top_left as i32
                } else {
                    l[i as usize] as i32
                }
            };
            let mut h = 0;
            let mut v = 0;
            for k in 0..8 {
                h += (k + 1) * (at(8 + k) - at(6 - k));
                v += (k + 1) * (al(8 + k) - al(6 - k));
            }
            let a = 16 * (l[15] as i32 + t[15] as i32);
            let b = (5 * h + 32) >> 6;
            let c = (5 * v + 32) >> 6;
            for y in 0..16 {
                for x in 0..16 {
                    out[y * 16 + x] = clip((a + b * (x as i32 - 7) + c * (y as i32 - 7) + 16) >> 5);
                }
            }
        }
        _ => {
            let dc = match (e.top, e.left) {
                (Some(t), Some(l)) => {
                    let s: u32 = t.iter().chain(l.iter()).map(|&v| v as u32).sum();
                    (s + 16) >> 5
                }
                (Some(t), None) => (t.iter().map(|&v| v as u32).sum::<u32>() + 8) >> 4,
                (None, Some(l)) => (l.iter().map(|&v| v as u32).sum::<u32>() + 8) >> 4,
                (None, None) => 128,
            };
            out.fill(dc as u8);
        }
    }
}

pub fn chroma_available(mode: u8, e: &Edges<8>) -> bool {
    match mode {
        C_DC => true,
        C_H => e.left.is_some(),
        C_V => e.top.is_some(),
        _ => e.top.is_some() && e.left.is_some(),
    }
}

/// Chroma prediction for one 8x8 component (raster 8x8).
pub fn pred_chroma(mode: u8, e: &Edges<8>, out: &mut [u8; 64]) {
    match (mode, e.top, e.left) {
        (C_V, Some(t), _) => {
            for row in out.chunks_exact_mut(8) {
                row.copy_from_slice(&t);
            }
        }
        (C_H, _, Some(l)) => {
            for (row, &v) in out.chunks_exact_mut(8).zip(l.iter()) {
                row.fill(v);
            }
        }
        (C_PLANE, Some(t), Some(l)) => {
            let at = |i: i32| -> i32 {
                if i < 0 {
                    e.top_left as i32
                } else {
                    t[i as usize] as i32
                }
            };
            let al = |i: i32| -> i32 {
                if i < 0 {
                    e.top_left as i32
                } else {
                    l[i as usize] as i32
                }
            };
            let mut h = 0;
            let mut v = 0;
            for k in 0..4 {
                h += (k + 1) * (at(4 + k) - at(2 - k));
                v += (k + 1) * (al(4 + k) - al(2 - k));
            }
            let a = 16 * (l[7] as i32 + t[7] as i32);
            let b = (34 * h + 32) >> 6;
            let c = (34 * v + 32) >> 6;
            for y in 0..8 {
                for x in 0..8 {
                    out[y * 8 + x] = clip((a + b * (x as i32 - 3) + c * (y as i32 - 3) + 16) >> 5);
                }
            }
        }
        _ => {
            // DC per 4x4 chroma block (§8.3.4.1..3).
            let sum = |s: &[u8]| s.iter().map(|&v| v as u32).sum::<u32>();
            for by in 0..2 {
                for bx in 0..2 {
                    let top = e.top.map(|t| sum(&t[bx * 4..bx * 4 + 4]));
                    let left = e.left.map(|l| sum(&l[by * 4..by * 4 + 4]));
                    let dc = if bx == by {
                        match (top, left) {
                            (Some(t), Some(l)) => (t + l + 4) >> 3,
                            (Some(t), None) => (t + 2) >> 2,
                            (None, Some(l)) => (l + 2) >> 2,
                            (None, None) => 128,
                        }
                    } else if bx == 1 {
                        // xO > 0, yO == 0: prefer the top.
                        match (top, left) {
                            (Some(t), _) => (t + 2) >> 2,
                            (None, Some(l)) => (l + 2) >> 2,
                            (None, None) => 128,
                        }
                    } else {
                        // xO == 0, yO > 0: prefer the left.
                        match (top, left) {
                            (_, Some(l)) => (l + 2) >> 2,
                            (Some(t), None) => (t + 2) >> 2,
                            (None, None) => 128,
                        }
                    };
                    for y in 0..4 {
                        let r = (by * 4 + y) * 8 + bx * 4;
                        out[r..r + 4].fill(dc as u8);
                    }
                }
            }
        }
    }
}

/// Neighbours of a 4x4 block: `top` holds p[0..7, -1] (top-right already
/// substituted by p[3, -1] when unavailable), `left` p[-1, 0..3].
#[derive(Clone, Copy)]
pub struct Edges4 {
    pub top: Option<[u8; 8]>,
    pub left: Option<[u8; 4]>,
    pub top_left: Option<u8>,
}

pub const I4_V: u8 = 0;
pub const I4_H: u8 = 1;
pub const I4_DC: u8 = 2;
pub const I4_DDL: u8 = 3;
pub const I4_DDR: u8 = 4;
pub const I4_VR: u8 = 5;
pub const I4_HD: u8 = 6;
pub const I4_VL: u8 = 7;
pub const I4_HU: u8 = 8;

pub fn i4_available(mode: u8, e: &Edges4) -> bool {
    match mode {
        I4_V | I4_DDL | I4_VL => e.top.is_some(),
        I4_H | I4_HU => e.left.is_some(),
        I4_DC => true,
        _ => e.top.is_some() && e.left.is_some() && e.top_left.is_some(),
    }
}

/// Intra_4x4 prediction (raster 4x4). The mode must be available.
pub fn pred4(mode: u8, e: &Edges4, out: &mut [u8; 16]) {
    let t = e.top.unwrap_or([128; 8]);
    let l = e.left.unwrap_or([128; 4]);
    let tl = e.top_left.unwrap_or(128) as i32;
    // p[x, -1] for x in -1..8 and p[-1, y] for y in -1..4.
    let pt = |x: i32| -> i32 {
        if x < 0 {
            tl
        } else {
            t[x as usize] as i32
        }
    };
    let pl = |y: i32| -> i32 {
        if y < 0 {
            tl
        } else {
            l[y as usize] as i32
        }
    };
    let mut set = |x: i32, y: i32, v: i32| out[(y * 4 + x) as usize] = v as u8;
    match mode {
        I4_V => {
            for y in 0..4 {
                for x in 0..4 {
                    set(x, y, pt(x));
                }
            }
        }
        I4_H => {
            for y in 0..4 {
                for x in 0..4 {
                    set(x, y, pl(y));
                }
            }
        }
        I4_DDL => {
            for y in 0..4 {
                for x in 0..4 {
                    let v = if x == 3 && y == 3 {
                        (pt(6) + 3 * pt(7) + 2) >> 2
                    } else {
                        (pt(x + y) + 2 * pt(x + y + 1) + pt(x + y + 2) + 2) >> 2
                    };
                    set(x, y, v);
                }
            }
        }
        I4_DDR => {
            for y in 0..4 {
                for x in 0..4 {
                    let v = if x > y {
                        (pt(x - y - 2) + 2 * pt(x - y - 1) + pt(x - y) + 2) >> 2
                    } else if x < y {
                        (pl(y - x - 2) + 2 * pl(y - x - 1) + pl(y - x) + 2) >> 2
                    } else {
                        (pt(0) + 2 * tl + pl(0) + 2) >> 2
                    };
                    set(x, y, v);
                }
            }
        }
        I4_VR => {
            for y in 0..4 {
                for x in 0..4 {
                    let z = 2 * x - y;
                    let v = if z >= 0 && z % 2 == 0 {
                        (pt(x - (y >> 1) - 1) + pt(x - (y >> 1)) + 1) >> 1
                    } else if z >= 0 {
                        (pt(x - (y >> 1) - 2) + 2 * pt(x - (y >> 1) - 1) + pt(x - (y >> 1)) + 2)
                            >> 2
                    } else if z == -1 {
                        (pl(0) + 2 * tl + pt(0) + 2) >> 2
                    } else {
                        (pl(y - 1) + 2 * pl(y - 2) + pl(y - 3) + 2) >> 2
                    };
                    set(x, y, v);
                }
            }
        }
        I4_HD => {
            for y in 0..4 {
                for x in 0..4 {
                    let z = 2 * y - x;
                    let v = if z >= 0 && z % 2 == 0 {
                        (pl(y - (x >> 1) - 1) + pl(y - (x >> 1)) + 1) >> 1
                    } else if z >= 0 {
                        (pl(y - (x >> 1) - 2) + 2 * pl(y - (x >> 1) - 1) + pl(y - (x >> 1)) + 2)
                            >> 2
                    } else if z == -1 {
                        (pl(0) + 2 * tl + pt(0) + 2) >> 2
                    } else {
                        (pt(x - 1) + 2 * pt(x - 2) + pt(x - 3) + 2) >> 2
                    };
                    set(x, y, v);
                }
            }
        }
        I4_VL => {
            for y in 0..4 {
                for x in 0..4 {
                    let i = x + (y >> 1);
                    let v = if y % 2 == 0 {
                        (pt(i) + pt(i + 1) + 1) >> 1
                    } else {
                        (pt(i) + 2 * pt(i + 1) + pt(i + 2) + 2) >> 2
                    };
                    set(x, y, v);
                }
            }
        }
        I4_HU => {
            for y in 0..4 {
                for x in 0..4 {
                    let z = x + 2 * y;
                    let i = y + (x >> 1);
                    let v = match z {
                        0 | 2 | 4 => (pl(i) + pl(i + 1) + 1) >> 1,
                        1 | 3 => (pl(i) + 2 * pl(i + 1) + pl(i + 2) + 2) >> 2,
                        5 => (pl(2) + 3 * pl(3) + 2) >> 2,
                        _ => pl(3),
                    };
                    set(x, y, v);
                }
            }
        }
        _ => {
            let st: i32 = (0..4).map(pt).sum();
            let sl: i32 = (0..4).map(pl).sum();
            let dc = match (e.top.is_some(), e.left.is_some()) {
                (true, true) => (st + sl + 4) >> 3,
                (true, false) => (st + 2) >> 2,
                (false, true) => (sl + 2) >> 2,
                (false, false) => 128,
            };
            out.fill(dc as u8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i16_dc_and_plane() {
        let mut out = [0u8; 256];
        let e = Edges::<16> {
            top: None,
            left: None,
            top_left: 0,
        };
        pred16(I16_DC, &e, &mut out);
        assert!(out.iter().all(|&v| v == 128));
        // A linear ramp is reproduced by the plane mode.
        let t: [u8; 16] = core::array::from_fn(|i| 40 + 4 * (i as u8 + 1));
        let l: [u8; 16] = core::array::from_fn(|i| 40 + 2 * (i as u8 + 1));
        let e = Edges {
            top: Some(t),
            left: Some(l),
            top_left: 40,
        };
        pred16(I16_PLANE, &e, &mut out);
        // b = (5*H+32)>>6 with H = Σ(k+1)*8(k+1)... just check monotonicity
        assert!(out[15] > out[0] && out[255] > out[15]);
    }

    #[test]
    fn i4_modes_known_answers() {
        let e = Edges4 {
            top: Some([10, 20, 30, 40, 50, 60, 70, 80]),
            left: Some([15, 25, 35, 45]),
            top_left: Some(5),
        };
        let mut o = [0u8; 16];
        pred4(I4_DDL, &e, &mut o);
        // (10 + 40 + 30 + 2) >> 2 = 20
        assert_eq!(o[0], 20);
        assert_eq!(o[15], ((70 + 3 * 80 + 2) >> 2) as u8);
        pred4(I4_DDR, &e, &mut o);
        assert_eq!(o[0], ((10 + 10 + 15 + 2) >> 2) as u8);
        pred4(I4_HU, &e, &mut o);
        assert_eq!(o[15], 45);
        assert_eq!(o[0], 20);
        pred4(I4_DC, &e, &mut o);
        assert_eq!(o[0], ((100 + 120 + 4) >> 3) as u8);
    }
}
