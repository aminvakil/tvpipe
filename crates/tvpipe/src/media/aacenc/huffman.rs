//! Noiseless coding: spectral n-tuples, escape sequences and scalefactor
//! differences (ISO/IEC 13818-7 clause 9, Tables 58-59, Annex A).

use super::bits::BitSink;
use super::tables::{Code, SCALEFACTOR, SPECTRUM};

/// Codebook parameters (Table 59): (unsigned, dimension, LAV).
pub const CB_PARAMS: [(bool, usize, i32); 12] = [
    (false, 0, 0),
    (false, 4, 1),
    (false, 4, 1),
    (true, 4, 2),
    (true, 4, 2),
    (false, 2, 4),
    (false, 2, 4),
    (true, 2, 7),
    (true, 2, 7),
    (true, 2, 12),
    (true, 2, 12),
    (true, 2, 16),
];

/// The escape codebook.
pub const ESC_HCB: usize = 11;

/// Largest quantized magnitude a codebook can carry (escape: 8191).
pub fn max_value(cb: usize) -> i32 {
    if cb == ESC_HCB {
        8191
    } else {
        CB_PARAMS[cb].2
    }
}

/// The smallest codebook number able to code magnitude `maxq`.
pub fn first_codebook(maxq: i32) -> usize {
    match maxq {
        0 => 0,
        1 => 1,
        2 => 3,
        3..=4 => 5,
        5..=7 => 7,
        8..=12 => 9,
        _ => ESC_HCB,
    }
}

#[inline]
fn tuple_index(cb: usize, vals: &[i32]) -> usize {
    let (unsigned, _, lav) = CB_PARAMS[cb];
    let mut idx = 0usize;
    if unsigned {
        let m = (lav + 1) as usize;
        for &v in vals {
            idx = idx * m + (v.unsigned_abs() as usize).min(16);
        }
    } else {
        let m = (2 * lav + 1) as usize;
        for &v in vals {
            idx = idx * m + (v + lav) as usize;
        }
    }
    idx
}

#[inline]
fn esc_len(v: u32) -> u32 {
    // escape_prefix N ones, separator, N+4 bit word: value 2^(N+4)+word.
    let n = 31 - v.leading_zeros() - 4;
    2 * n + 5
}

/// Bits for one n-tuple (codeword + sign bits + escapes). Values must be
/// within the codebook's range.
#[inline]
pub fn tuple_bits(cb: usize, vals: &[i32]) -> u32 {
    let code: Code = SPECTRUM[cb][tuple_index(cb, vals)];
    let mut bits = code.0 as u32;
    if CB_PARAMS[cb].0 {
        for &v in vals {
            if v != 0 {
                bits += 1;
            }
            if cb == ESC_HCB && v.unsigned_abs() >= 16 {
                bits += esc_len(v.unsigned_abs());
            }
        }
    }
    bits
}

/// Bits for a run of quantized values (length a multiple of the
/// codebook's dimension) in codebook `cb` (1..=11).
pub fn run_bits(cb: usize, q: &[i32]) -> u32 {
    let dim = CB_PARAMS[cb].1;
    q.chunks_exact(dim).map(|t| tuple_bits(cb, t)).sum()
}

/// Writes a run of quantized values with codebook `cb` (1..=11).
pub fn write_run(out: &mut impl BitSink, cb: usize, q: &[i32]) {
    let (unsigned, dim, _) = CB_PARAMS[cb];
    for t in q.chunks_exact(dim) {
        let (len, code) = SPECTRUM[cb][tuple_index(cb, t)];
        out.put(code, len as u32);
        if unsigned {
            for &v in t {
                if v != 0 {
                    out.put((v < 0) as u32, 1);
                }
            }
            if cb == ESC_HCB {
                for &v in t {
                    let a = v.unsigned_abs();
                    if a >= 16 {
                        let n = 31 - a.leading_zeros() - 4;
                        out.put((1 << n) - 1, n);
                        out.put(0, 1);
                        out.put(a - (1 << (n + 4)), n + 4);
                    }
                }
            }
        }
    }
}

/// Bits of a scalefactor difference (-60..=60).
#[inline]
pub fn sf_bits(diff: i32) -> u32 {
    SCALEFACTOR[(diff + 60) as usize].0 as u32
}

pub fn write_sf(out: &mut impl BitSink, diff: i32) {
    let (len, code) = SCALEFACTOR[(diff + 60) as usize];
    out.put(code, len as u32);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::aacenc::bits::{BitCounter, BitWriter};
    use crate::media::aacenc::tables::SCALEFACTOR;

    fn check_prefix_code(codes: &[Code]) {
        let kraft: f64 = codes.iter().map(|c| 0.5f64.powi(c.0 as i32)).sum();
        assert!((kraft - 1.0).abs() < 1e-12, "kraft {kraft}");
        let mut v: Vec<String> = codes
            .iter()
            .map(|&(l, c)| format!("{:0width$b}", c, width = l as usize))
            .collect();
        v.sort();
        for w in v.windows(2) {
            assert!(!w[1].starts_with(&w[0]), "{} prefixes {}", w[0], w[1]);
        }
    }

    #[test]
    fn tables_are_complete_prefix_codes() {
        check_prefix_code(&SCALEFACTOR);
        assert_eq!(SCALEFACTOR[60], (1, 0));
        let sizes = [0, 81, 81, 81, 81, 81, 81, 64, 64, 169, 169, 289];
        for cb in 1..=11 {
            assert_eq!(SPECTRUM[cb].len(), sizes[cb]);
            check_prefix_code(SPECTRUM[cb]);
        }
    }

    /// Escape coding examples from §9.3: 16 -> "00000", 31 -> "01111",
    /// 32 -> "1000000", 63 -> "1011111".
    #[test]
    fn escape_sequences() {
        for (v, bits, len) in [
            (16u32, 0b00000u32, 5u32),
            (31, 0b01111, 5),
            (32, 0b1000000, 7),
            (63, 0b1011111, 7),
            (8191, 0b1_1111_1110_1111_1111_1111, 21),
        ] {
            assert_eq!(esc_len(v), len);
            let mut w = BitWriter::new();
            // Codebook 11, pair (16+, 0): codeword, one sign bit, escape.
            write_run(&mut w, ESC_HCB, &[v as i32, 0]);
            let (cl, cc) = SPECTRUM[11][16 * 17];
            let mut want = BitWriter::new();
            want.put(cc, cl as u32);
            want.put(0, 1);
            want.put(bits, len);
            assert_eq!(w.bits(), want.bits());
            assert_eq!(w.finish(), want.finish());
        }
    }

    #[test]
    fn counted_bits_match_written() {
        let mut seed = 1u32;
        for (cb, &(unsigned, _, cb_lav)) in CB_PARAMS.iter().enumerate().skip(1) {
            let lav = max_value(cb).min(40);
            let q: Vec<i32> = (0..64)
                .map(|_| {
                    seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                    let r = (seed >> 16) as i32 % (2 * lav + 1) - lav;
                    if unsigned || r.abs() <= cb_lav {
                        r
                    } else {
                        0
                    }
                })
                .collect();
            let mut c = BitCounter::default();
            write_run(&mut c, cb, &q);
            assert_eq!(c.0 as u32, run_bits(cb, &q), "cb {cb}");
        }
    }

    #[test]
    fn index_mapping_matches_decoder_formula() {
        // §9.3: idx -> (w,x,y,z) for signed dim-4 lav-1 codebooks.
        let idx = tuple_index(1, &[-1, 0, 1, 1]);
        let (m, off) = (3usize, 1i32);
        let w = (idx / (m * m * m)) as i32 - off;
        let x = (idx / (m * m) % m) as i32 - off;
        let y = (idx / m % m) as i32 - off;
        let z = (idx % m) as i32 - off;
        assert_eq!((w, x, y, z), (-1, 0, 1, 1));
        assert_eq!(tuple_index(11, &[-20, 3]), 16 * 17 + 3);
        assert_eq!(first_codebook(13), ESC_HCB);
    }
}
