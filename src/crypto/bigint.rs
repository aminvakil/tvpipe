//! Fixed-width unsigned big integers for SRP (3072-bit group, 4096-bit
//! headroom).
//!
//! [`Uint`] is a 4096-bit little-endian array of 64-bit limbs. Arithmetic
//! is checked (no silent overflow) and every fallible operation returns a
//! `Result` or `Option`: inputs come from the network (SRP `B`, salts).
//!
//! `modpow` uses Montgomery multiplication (CIOS, Koç et al. 1996) with a
//! fixed 4-bit window for odd moduli, and plain square-and-multiply with
//! long division for even ones (only used in tests; SRP moduli are prime).
//! The window table is read with a constant-time select and the Montgomery
//! product does its final subtraction branch-free, but the code is not
//! audited as constant-time; the exponent's bit length is visible.

// Limb loops index several arrays in lockstep; iterators would obscure them.
#![allow(clippy::needless_range_loop)]

use std::cmp::Ordering;

use crate::error::{Error, Result};

/// Number of 64-bit limbs.
pub const LIMBS: usize = 64;
/// Width in bits.
pub const BITS: usize = LIMBS * 64;
/// Width in bytes.
pub const BYTES: usize = BITS / 8;

/// A 4096-bit unsigned integer.
#[derive(Clone, PartialEq, Eq)]
pub struct Uint {
    /// Little-endian limbs: `limbs[0]` is the least significant.
    limbs: [u64; LIMBS],
}

impl std::fmt::Debug for Uint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Uint(0x{})",
            crate::util::hex::encode(&self.to_be_bytes())
        )
    }
}

impl Default for Uint {
    fn default() -> Uint {
        Uint::ZERO
    }
}

impl Ord for Uint {
    fn cmp(&self, other: &Uint) -> Ordering {
        cmp_limbs(&self.limbs, &other.limbs)
    }
}

impl PartialOrd for Uint {
    fn partial_cmp(&self, other: &Uint) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[inline(always)]
fn adc(a: u64, b: u64, carry: u64) -> (u64, u64) {
    let t = a as u128 + b as u128 + carry as u128;
    (t as u64, (t >> 64) as u64)
}

#[inline(always)]
fn sbb(a: u64, b: u64, borrow: u64) -> (u64, u64) {
    let t = (a as u128).wrapping_sub(b as u128 + borrow as u128);
    (t as u64, ((t >> 64) as u64) & 1)
}

/// `t + a * b + carry`, as (low, high). Cannot overflow 128 bits.
#[inline(always)]
fn mac(t: u64, a: u64, b: u64, carry: u64) -> (u64, u64) {
    let v = t as u128 + (a as u128) * (b as u128) + carry as u128;
    (v as u64, (v >> 64) as u64)
}

fn cmp_limbs(a: &[u64], b: &[u64]) -> Ordering {
    debug_assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().rev().zip(b.iter().rev()) {
        match x.cmp(y) {
            Ordering::Equal => {}
            o => return o,
        }
    }
    Ordering::Equal
}

/// Number of significant limbs.
fn limb_len(a: &[u64]) -> usize {
    a.iter().rposition(|&l| l != 0).map_or(0, |i| i + 1)
}

impl Uint {
    pub const ZERO: Uint = Uint { limbs: [0; LIMBS] };
    pub const ONE: Uint = Uint::from_u64(1);

    pub const fn from_u64(v: u64) -> Uint {
        let mut limbs = [0; LIMBS];
        limbs[0] = v;
        Uint { limbs }
    }

    /// Parses a big-endian byte string. Leading zero bytes are allowed in
    /// any number; errors if the value needs more than 4096 bits.
    pub fn from_be_bytes(bytes: &[u8]) -> Result<Uint> {
        let start = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len());
        let bytes = &bytes[start..];
        if bytes.len() > BYTES {
            return Err(Error::crypto(format!(
                "bigint: {} bytes exceed the {BITS}-bit width",
                bytes.len()
            )));
        }
        let mut limbs = [0u64; LIMBS];
        for (i, chunk) in bytes.rchunks(8).enumerate() {
            limbs[i] = chunk.iter().fold(0u64, |acc, &b| (acc << 8) | b as u64);
        }
        Ok(Uint { limbs })
    }

    /// Minimal big-endian encoding (no leading zeros; zero is empty).
    pub fn to_be_bytes(&self) -> Vec<u8> {
        let len = self.bits().div_ceil(8);
        let mut out = vec![0u8; len];
        self.write_be(&mut out);
        out
    }

    /// Big-endian encoding left-padded with zeros to exactly `len` bytes
    /// (SRP's `PAD()`); errors if the value does not fit.
    pub fn to_be_bytes_padded(&self, len: usize) -> Result<Vec<u8>> {
        if self.bits().div_ceil(8) > len {
            return Err(Error::crypto(format!(
                "bigint: value does not fit in {len} bytes"
            )));
        }
        let mut out = vec![0u8; len];
        self.write_be(&mut out);
        Ok(out)
    }

    /// Writes the low `out.len()` bytes, big-endian.
    fn write_be(&self, out: &mut [u8]) {
        let n = out.len();
        for (i, b) in out.iter_mut().enumerate() {
            let byte_idx = n - 1 - i; // little-endian byte index
            if byte_idx < BYTES {
                *b = (self.limbs[byte_idx / 8] >> (8 * (byte_idx % 8))) as u8;
            }
        }
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.iter().all(|&l| l == 0)
    }

    /// Position of the highest set bit plus one (0 for zero).
    pub fn bits(&self) -> usize {
        let n = limb_len(&self.limbs);
        if n == 0 {
            0
        } else {
            (n - 1) * 64 + (64 - self.limbs[n - 1].leading_zeros() as usize)
        }
    }

    /// Bit `i` (0 = least significant); false beyond the width.
    pub fn bit(&self, i: usize) -> bool {
        i < BITS && (self.limbs[i / 64] >> (i % 64)) & 1 == 1
    }

    pub fn checked_add(&self, other: &Uint) -> Option<Uint> {
        let mut out = Uint::ZERO;
        let mut carry = 0;
        for i in 0..LIMBS {
            (out.limbs[i], carry) = adc(self.limbs[i], other.limbs[i], carry);
        }
        (carry == 0).then_some(out)
    }

    /// `self - other`, or `None` if `other > self`.
    pub fn checked_sub(&self, other: &Uint) -> Option<Uint> {
        let mut out = Uint::ZERO;
        let mut borrow = 0;
        for i in 0..LIMBS {
            (out.limbs[i], borrow) = sbb(self.limbs[i], other.limbs[i], borrow);
        }
        (borrow == 0).then_some(out)
    }

    pub fn checked_mul(&self, other: &Uint) -> Option<Uint> {
        let wide = mul_wide(self, other);
        if wide[LIMBS..].iter().any(|&l| l != 0) {
            return None;
        }
        let mut out = Uint::ZERO;
        out.limbs.copy_from_slice(&wide[..LIMBS]);
        Some(out)
    }

    /// `self mod m`; errors if `m` is zero.
    pub fn rem(&self, m: &Uint) -> Result<Uint> {
        check_modulus(m)?;
        Ok(rem_limbs(&self.limbs, m))
    }

    /// `(self + other) mod m`.
    pub fn mod_add(&self, other: &Uint, m: &Uint) -> Result<Uint> {
        check_modulus(m)?;
        let a = rem_limbs(&self.limbs, m);
        let b = rem_limbs(&other.limbs, m);
        // a + b < 2m <= 2^4097: keep the carry in an extra limb.
        let mut t = [0u64; LIMBS + 1];
        let mut carry = 0;
        for i in 0..LIMBS {
            (t[i], carry) = adc(a.limbs[i], b.limbs[i], carry);
        }
        t[LIMBS] = carry;
        Ok(rem_limbs(&t, m))
    }

    /// `(self - other) mod m`, always in `[0, m)`.
    pub fn mod_sub(&self, other: &Uint, m: &Uint) -> Result<Uint> {
        check_modulus(m)?;
        let a = rem_limbs(&self.limbs, m);
        let b = rem_limbs(&other.limbs, m);
        Ok(match a.checked_sub(&b) {
            Some(d) => d,
            // a < b < m, so m - (b - a) is in range.
            None => m
                .checked_sub(&b.checked_sub(&a).unwrap_or_default())
                .unwrap_or_default(),
        })
    }

    /// `(self * other) mod m`.
    pub fn mod_mul(&self, other: &Uint, m: &Uint) -> Result<Uint> {
        check_modulus(m)?;
        Ok(rem_limbs(&mul_wide(self, other), m))
    }

    /// `self ^ exp mod m`; errors if `m` is zero.
    pub fn modpow(&self, exp: &Uint, m: &Uint) -> Result<Uint> {
        check_modulus(m)?;
        if *m == Uint::ONE {
            return Ok(Uint::ZERO);
        }
        if m.limbs[0] & 1 == 1 {
            return Ok(Mont::new(m).pow(self, exp));
        }
        // Even modulus: plain right-to-left square-and-multiply.
        let mut base = rem_limbs(&self.limbs, m);
        let mut acc = Uint::ONE;
        for i in 0..exp.bits() {
            if exp.bit(i) {
                acc = rem_limbs(&mul_wide(&acc, &base), m);
            }
            base = rem_limbs(&mul_wide(&base, &base), m);
        }
        Ok(acc)
    }
}

fn check_modulus(m: &Uint) -> Result<()> {
    if m.is_zero() {
        return Err(Error::crypto("bigint: modulus is zero"));
    }
    Ok(())
}

/// Full 8192-bit product.
fn mul_wide(a: &Uint, b: &Uint) -> [u64; 2 * LIMBS] {
    let mut out = [0u64; 2 * LIMBS];
    let (na, nb) = (limb_len(&a.limbs), limb_len(&b.limbs));
    for i in 0..na {
        let mut carry = 0;
        for j in 0..nb {
            (out[i + j], carry) = mac(out[i + j], a.limbs[i], b.limbs[j], carry);
        }
        out[i + nb] = carry;
    }
    out
}

/// `x mod m` for any number of limbs, by binary long division (shift in one
/// bit at a time, subtract when the remainder reaches `m`). `m` is nonzero.
fn rem_limbs(x: &[u64], m: &Uint) -> Uint {
    let nm = limb_len(&m.limbs);
    let nx = limb_len(x);
    // Fast path: x < m (x always has at least nm limbs of storage).
    if nx < nm || (nx == nm && cmp_limbs(&x[..nm], &m.limbs[..nm]) == Ordering::Less) {
        let mut out = Uint::ZERO;
        out.limbs[..nx].copy_from_slice(&x[..nx]);
        return out;
    }
    // Remainder stays < 2m, which fits in nm + 1 limbs.
    let w = nm + 1;
    let mut r = [0u64; LIMBS + 1];
    let total_bits = nx * 64;
    for bit in (0..total_bits).rev() {
        // r = 2r + bit
        let mut carry = (x[bit / 64] >> (bit % 64)) & 1;
        for limb in r[..w].iter_mut() {
            let next = *limb >> 63;
            *limb = (*limb << 1) | carry;
            carry = next;
        }
        // if r >= m { r -= m }
        let mut ge = r[nm] != 0;
        if !ge {
            ge = cmp_limbs(&r[..nm], &m.limbs[..nm]) != Ordering::Less;
        }
        if ge {
            let mut borrow = 0;
            for i in 0..w {
                let mi = if i < nm { m.limbs[i] } else { 0 };
                (r[i], borrow) = sbb(r[i], mi, borrow);
            }
        }
    }
    let mut out = Uint::ZERO;
    out.limbs[..nm].copy_from_slice(&r[..nm]);
    out
}

/// Montgomery arithmetic modulo an odd `m` with `k` significant limbs and
/// R = 2^(64k).
struct Mont {
    m: [u64; LIMBS],
    k: usize,
    /// -m^-1 mod 2^64.
    n0inv: u64,
    /// R mod m (Montgomery form of 1).
    r1: Uint,
    /// R^2 mod m.
    r2: Uint,
}

impl Mont {
    fn new(m: &Uint) -> Mont {
        let k = limb_len(&m.limbs);
        // Newton iteration: each step doubles the correct low bits.
        let m0 = m.limbs[0];
        let mut inv = 1u64;
        for _ in 0..6 {
            inv = inv.wrapping_mul(2u64.wrapping_sub(m0.wrapping_mul(inv)));
        }
        let mut r = [0u64; LIMBS + 1];
        r[k] = 1;
        let r1 = rem_limbs(&r, m);
        let r2 = rem_limbs(&mul_wide(&r1, &r1), m);
        Mont {
            m: m.limbs,
            k,
            n0inv: inv.wrapping_neg(),
            r1,
            r2,
        }
    }

    /// a * b * R^-1 mod m, for a, b < m (CIOS).
    fn mul(&self, a: &Uint, b: &Uint) -> Uint {
        let k = self.k;
        let (a, b, n) = (&a.limbs, &b.limbs, &self.m);
        let mut t = [0u64; LIMBS + 2];
        for &bi in &b[..k] {
            let mut c = 0;
            for j in 0..k {
                (t[j], c) = mac(t[j], a[j], bi, c);
            }
            let (s, c2) = adc(t[k], c, 0);
            t[k] = s;
            t[k + 1] = c2;

            let q = t[0].wrapping_mul(self.n0inv);
            let (_, mut c) = mac(t[0], q, n[0], 0);
            for j in 1..k {
                (t[j - 1], c) = mac(t[j], q, n[j], c);
            }
            let (s, c2) = adc(t[k], c, 0);
            t[k - 1] = s;
            t[k] = t[k + 1] + c2;
        }
        // t < 2m: subtract m once if t >= m, without branching on the data.
        let mut d = [0u64; LIMBS];
        let mut borrow = 0;
        for j in 0..k {
            (d[j], borrow) = sbb(t[j], n[j], borrow);
        }
        let (_, borrow) = sbb(t[k], 0, borrow);
        // borrow == 1 means t < m: keep t.
        let keep_t = borrow.wrapping_neg();
        let mut out = Uint::ZERO;
        for j in 0..k {
            out.limbs[j] = (t[j] & keep_t) | (d[j] & !keep_t);
        }
        out
    }

    fn pow(&self, base: &Uint, exp: &Uint) -> Uint {
        let m = Uint { limbs: self.m };
        let b = rem_limbs(&base.limbs, &m);
        let mut table = [Uint::ZERO; 16];
        table[0] = self.r1.clone();
        table[1] = self.mul(&b, &self.r2);
        for i in 2..16 {
            table[i] = self.mul(&table[i - 1], &table[1]);
        }
        let windows = exp.bits().div_ceil(4);
        let mut acc = self.r1.clone();
        for w in (0..windows).rev() {
            if w + 1 != windows {
                for _ in 0..4 {
                    acc = self.mul(&acc, &acc);
                }
            }
            let nibble = ((exp.limbs[w / 16] >> (4 * (w % 16))) & 0xf) as usize;
            let sel = ct_select(&table, nibble, self.k);
            acc = self.mul(&acc, &sel);
        }
        self.mul(&acc, &Uint::ONE)
    }
}

/// `table[idx]`, reading every entry so the access pattern does not depend
/// on `idx`. Only the low `k` limbs are copied.
fn ct_select(table: &[Uint; 16], idx: usize, k: usize) -> Uint {
    let mut out = Uint::ZERO;
    for (i, entry) in table.iter().enumerate() {
        let mask = ((i == idx) as u64).wrapping_neg();
        for j in 0..k {
            out.limbs[j] |= entry.limbs[j] & mask;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::vectors;

    fn u(hex: &str) -> Uint {
        Uint::from_be_bytes(&crate::util::hex::decode(hex).unwrap()).unwrap()
    }

    #[test]
    fn bytes_roundtrip() {
        assert_eq!(Uint::from_be_bytes(&[]).unwrap(), Uint::ZERO);
        assert_eq!(Uint::ZERO.to_be_bytes(), Vec::<u8>::new());
        assert_eq!(Uint::from_u64(0x0102).to_be_bytes(), [1, 2]);
        assert_eq!(
            Uint::from_be_bytes(&[0, 0, 1, 2]).unwrap(),
            Uint::from_u64(0x102)
        );
        assert_eq!(
            Uint::from_u64(0x0102).to_be_bytes_padded(4).unwrap(),
            [0, 0, 1, 2]
        );
        assert!(Uint::from_u64(0x010203).to_be_bytes_padded(2).is_err());
        let max = Uint::from_be_bytes(&[0xff; BYTES]).unwrap();
        assert_eq!(max.bits(), BITS);
        assert_eq!(max.to_be_bytes(), vec![0xff; BYTES]);
        let mut long = vec![0u8; 3];
        long.extend_from_slice(&[0xff; BYTES]);
        assert_eq!(Uint::from_be_bytes(&long).unwrap(), max);
        assert!(Uint::from_be_bytes(&[1; BYTES + 1]).is_err());
        assert_eq!(max.checked_add(&Uint::ONE), None);
        assert_eq!(Uint::ZERO.checked_sub(&Uint::ONE), None);
        assert_eq!(max.checked_mul(&Uint::from_u64(2)), None);
    }

    #[test]
    fn zero_modulus_is_error() {
        let a = Uint::from_u64(5);
        assert!(a.rem(&Uint::ZERO).is_err());
        assert!(a.modpow(&a, &Uint::ZERO).is_err());
        assert!(a.mod_mul(&a, &Uint::ZERO).is_err());
        assert!(a.mod_add(&a, &Uint::ZERO).is_err());
        assert!(a.mod_sub(&a, &Uint::ZERO).is_err());
    }

    #[test]
    fn small_values() {
        let m = Uint::from_u64(1_000_000_007);
        assert_eq!(
            Uint::from_u64(2)
                .modpow(&Uint::from_u64(1_000_000_006), &m)
                .unwrap(),
            Uint::ONE
        );
        assert_eq!(
            Uint::from_u64(3)
                .modpow(&Uint::from_u64(4), &Uint::from_u64(100))
                .unwrap(),
            Uint::from_u64(81)
        );
        assert_eq!(
            Uint::from_u64(3)
                .mod_sub(&Uint::from_u64(5), &Uint::from_u64(7))
                .unwrap(),
            Uint::from_u64(5)
        );
        assert_eq!(u("0100").bits(), 9);
    }

    /// `cargo test --release modpow_speed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn modpow_speed() {
        // Odd 3072-bit modulus, 3072-bit exponent: ~11 ms on an M5 Max.
        let m = Uint::from_be_bytes(&[0xa5; 384]).unwrap();
        let e = Uint::from_be_bytes(&[0x5a; 384]).unwrap();
        let start = std::time::Instant::now();
        let n = 20;
        for i in 0..n {
            std::hint::black_box(Uint::from_u64(i + 2).modpow(&e, &m).unwrap());
        }
        println!("3072-bit modpow: {:?} each", start.elapsed() / n as u32);
    }

    #[test]
    fn python_vectors() {
        let blocks = vectors::load("crypto/bigint.txt");
        assert!(blocks.len() > 50);
        for b in blocks {
            let a = u(b.str("a"));
            let r = u(b.str("r"));
            let name = b.name();
            match b.str("op") {
                "add" => assert_eq!(a.checked_add(&u(b.str("b"))), Some(r), "{name}"),
                "sub" => assert_eq!(a.checked_sub(&u(b.str("b"))), Some(r), "{name}"),
                "mul" => assert_eq!(a.checked_mul(&u(b.str("b"))), Some(r), "{name}"),
                "rem" => assert_eq!(a.rem(&u(b.str("m"))).unwrap(), r, "{name}"),
                "modadd" => {
                    assert_eq!(
                        a.mod_add(&u(b.str("b")), &u(b.str("m"))).unwrap(),
                        r,
                        "{name}"
                    )
                }
                "modsub" => {
                    assert_eq!(
                        a.mod_sub(&u(b.str("b")), &u(b.str("m"))).unwrap(),
                        r,
                        "{name}"
                    )
                }
                "modmul" => {
                    assert_eq!(
                        a.mod_mul(&u(b.str("b")), &u(b.str("m"))).unwrap(),
                        r,
                        "{name}"
                    )
                }
                "modpow" => {
                    assert_eq!(
                        a.modpow(&u(b.str("e")), &u(b.str("m"))).unwrap(),
                        r,
                        "{name}"
                    )
                }
                op => panic!("unknown op {op}"),
            }
        }
    }
}
