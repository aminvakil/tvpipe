//! CAVLC residual block coding (residual_block_cavlc).
//!
//! Sources: ITU-T H.264 §7.3.5.3.2 (syntax), §9.2 (parsing process:
//! coeff_token §9.2.1, levels §9.2.2, total_zeros/run_before §9.2.3).

#[cfg(test)]
use super::bits::BitWriter;
use super::bits::{BitCounter, BitSink};
use super::tables::{nc_class, COEFF_TOKEN, RUN_BEFORE, TOTAL_ZEROS, TOTAL_ZEROS_CHROMA_DC};

/// Writes one residual block. `coeffs` are levels in scan order, of length
/// maxNumCoeff (16, 15 or 4); `nc` is the coeff_token context (−1 for
/// chroma DC). Returns TotalCoeff.
pub fn write_block<W: BitSink>(w: &mut W, coeffs: &[i32], nc: i32) -> u32 {
    let max = coeffs.len();
    // Non-zero levels from the highest frequency down, with the zero runs
    // that precede each.
    let mut levels = [0i32; 16];
    let mut runs = [0u32; 16];
    let mut total = 0usize;
    let mut last = None;
    for i in (0..max).rev() {
        if coeffs[i] != 0 {
            last.get_or_insert(i);
            levels[total] = coeffs[i];
            total += 1;
        }
    }
    let class = nc_class(nc);
    let Some(last) = last else {
        let (code, len) = COEFF_TOKEN[class][0][0];
        w.put(len as u32, code as u32);
        return 0;
    };
    // run_before of each coefficient (zeros between it and the next lower
    // non-zero one).
    {
        let mut k = 0;
        let mut run = 0u32;
        for i in (0..=last).rev() {
            if coeffs[i] != 0 {
                if k > 0 {
                    runs[k - 1] = run;
                }
                k += 1;
                run = 0;
            } else {
                run += 1;
            }
        }
        runs[k - 1] = run; // zeros below the lowest coefficient (unused)
    }
    let total_zeros = (last + 1 - total) as u32;
    let mut t1 = 0usize;
    while t1 < 3 && t1 < total && levels[t1].abs() == 1 {
        t1 += 1;
    }
    let (code, len) = COEFF_TOKEN[class][total][t1];
    w.put(len as u32, code as u32);
    for &l in &levels[..t1] {
        w.flag(l < 0);
    }
    let mut suffix_len: u32 = if total > 10 && t1 < 3 { 1 } else { 0 };
    for (i, &level) in levels[t1..total].iter().enumerate() {
        let mut code = if level > 0 {
            2 * level as u32 - 2
        } else {
            (-2 * level - 1) as u32
        };
        if i == 0 && t1 < 3 {
            code -= 2;
        }
        write_level(w, code, suffix_len);
        if suffix_len == 0 {
            suffix_len = 1;
        }
        if level.unsigned_abs() > (3 << (suffix_len - 1)) && suffix_len < 6 {
            suffix_len += 1;
        }
    }
    if total < max {
        let (code, len) = if max == 4 {
            TOTAL_ZEROS_CHROMA_DC[total][total_zeros as usize]
        } else {
            TOTAL_ZEROS[total][total_zeros as usize]
        };
        w.put(len as u32, code as u32);
    }
    let mut zeros_left = total_zeros;
    for &run in &runs[..total - 1] {
        if zeros_left == 0 {
            break;
        }
        let (code, len) = RUN_BEFORE[zeros_left.min(7) as usize][run as usize];
        w.put(len as u32, code as u32);
        zeros_left -= run;
    }
    total as u32
}

/// level_prefix / level_suffix for a levelCode (§9.2.2.1, inverted).
/// Callers keep |level| ≤ 2047 so the 12-bit escape always suffices.
#[inline]
fn write_level<W: BitSink>(w: &mut W, code: u32, suffix_len: u32) {
    if suffix_len == 0 {
        if code < 14 {
            w.put(code + 1, 1);
        } else if code < 30 {
            w.put(15, 1);
            w.put(4, code - 14);
        } else {
            w.put(16, 1);
            w.put(12, code - 30);
        }
    } else if code < (15 << suffix_len) {
        w.put((code >> suffix_len) + 1, 1);
        w.put(suffix_len, code & ((1 << suffix_len) - 1));
    } else {
        w.put(16, 1);
        w.put(12, code - (15 << suffix_len));
    }
}

/// Bits [`write_block`] would produce (for mode decisions).
pub fn block_bits(coeffs: &[i32], nc: i32) -> usize {
    let mut w = BitCounter::default();
    write_block(&mut w, coeffs, nc);
    w.bits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(w: &BitWriter) -> String {
        let mut s = String::new();
        let mut c = w.clone();
        let n = c.bit_len();
        c.align_zero();
        for b in c.bytes() {
            s.push_str(&format!("{b:08b}"));
        }
        s.truncate(n);
        s
    }

    #[test]
    fn richardson_example_1() {
        // Richardson, "H.264 and MPEG-4 Video Compression", §6.4.13.2,
        // example 1: block 0,3,-1,0 / 0,-1,1,0 / 1,0,0,0 / 0,0,0,0 with
        // nC = 0. Zig-zag: 0,3,0,1,-1,-1,0,1,0,...
        let mut c = [0i32; 16];
        c[..8].copy_from_slice(&[0, 3, 0, 1, -1, -1, 0, 1]);
        let mut w = BitWriter::new();
        assert_eq!(write_block(&mut w, &c, 0), 5);
        // coeff_token(5,3)=0000100, signs 011, levels +1 → 1, +3 → 0010,
        // total_zeros(5)=3 → 111, runs: 1 (zl 3) → 10, 0 (zl 2) → 1,
        // 0 (zl 2) → 1, 1 (zl 2) → 01.
        assert_eq!(bits(&w), "000010001110010111101101");
    }

    #[test]
    fn empty_and_chroma_dc() {
        let mut w = BitWriter::new();
        write_block(&mut w, &[0; 16], 0);
        assert_eq!(bits(&w), "1");
        let mut w = BitWriter::new();
        write_block(&mut w, &[0; 4], -1);
        assert_eq!(bits(&w), "01");
        // Chroma DC [1,0,0,0]: coeff_token(1,1)=1, sign 0, total_zeros 0
        // (TotalCoeff 1) → 1.
        let mut w = BitWriter::new();
        write_block(&mut w, &[1, 0, 0, 0], -1);
        assert_eq!(bits(&w), "101");
    }

    #[test]
    fn large_levels_use_escape() {
        let mut c = [0i32; 16];
        c[0] = 2047;
        c[1] = -2047;
        c[2] = 30;
        let mut w = BitWriter::new();
        write_block(&mut w, &c, 0);
        assert!(w.bit_len() > 40);
    }
}
