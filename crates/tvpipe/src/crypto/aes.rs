//! AES block cipher (FIPS-197), table-based ("T-table") implementation.
//!
//! Supports 128-, 192- and 256-bit keys. All tables (S-box, inverse S-box,
//! the four encryption and four decryption round tables) are computed at
//! compile time from the field arithmetic in FIPS-197 §4, so there are no
//! hand-typed constants to get wrong.
//!
//! Decryption uses the "equivalent inverse cipher" of FIPS-197 §5.3.5:
//! the decryption key schedule is the encryption schedule reversed with
//! InvMixColumns applied to the inner round keys.
//!
//! Note: table lookups depend on secret data, so this is not constant-time
//! with respect to cache-timing attacks. That is acceptable for our use
//! (streaming media to devices on the local network); never use it for
//! long-term secrets exposed to co-resident attackers.

use crate::error::{Error, Result};

pub const BLOCK_LEN: usize = 16;

// ---------------------------------------------------------------------------
// Compile-time table generation.

/// Multiplication by x in GF(2^8) mod x^8 + x^4 + x^3 + x + 1.
const fn xtime(b: u8) -> u8 {
    (b << 1) ^ (if b & 0x80 != 0 { 0x1b } else { 0 })
}

/// General multiplication in GF(2^8).
const fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            p ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    p
}

const fn gen_sbox() -> [u8; 256] {
    let mut sbox = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        // Multiplicative inverse (0 maps to 0): a^254.
        let a = i as u8;
        let mut inv = 1u8;
        let mut e = 0;
        while e < 254 {
            inv = gmul(inv, a);
            e += 1;
        }
        if a == 0 {
            inv = 0;
        }
        // Affine transform (FIPS-197 eq. 5.1).
        let s = inv
            ^ inv.rotate_left(1)
            ^ inv.rotate_left(2)
            ^ inv.rotate_left(3)
            ^ inv.rotate_left(4)
            ^ 0x63;
        sbox[i] = s;
        i += 1;
    }
    sbox
}

const fn gen_inv_sbox(sbox: &[u8; 256]) -> [u8; 256] {
    let mut inv = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        inv[sbox[i] as usize] = i as u8;
        i += 1;
    }
    inv
}

/// Te0[x] = column (2·S[x], S[x], S[x], 3·S[x]) as a big-endian word;
/// Te1..Te3 are byte rotations of it.
const fn gen_te(sbox: &[u8; 256], rot: u32) -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let s = sbox[i];
        let w = ((gmul(s, 2) as u32) << 24)
            | ((s as u32) << 16)
            | ((s as u32) << 8)
            | (gmul(s, 3) as u32);
        t[i] = w.rotate_right(rot * 8);
        i += 1;
    }
    t
}

/// Td0[x] = column (14·S⁻¹[x], 9·S⁻¹[x], 13·S⁻¹[x], 11·S⁻¹[x]).
const fn gen_td(inv_sbox: &[u8; 256], rot: u32) -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let s = inv_sbox[i];
        let w = ((gmul(s, 14) as u32) << 24)
            | ((gmul(s, 9) as u32) << 16)
            | ((gmul(s, 13) as u32) << 8)
            | (gmul(s, 11) as u32);
        t[i] = w.rotate_right(rot * 8);
        i += 1;
    }
    t
}

static SBOX: [u8; 256] = gen_sbox();
static INV_SBOX: [u8; 256] = gen_inv_sbox(&SBOX);
static TE0: [u32; 256] = gen_te(&SBOX, 0);
static TE1: [u32; 256] = gen_te(&SBOX, 1);
static TE2: [u32; 256] = gen_te(&SBOX, 2);
static TE3: [u32; 256] = gen_te(&SBOX, 3);
static TD0: [u32; 256] = gen_td(&INV_SBOX, 0);
static TD1: [u32; 256] = gen_td(&INV_SBOX, 1);
static TD2: [u32; 256] = gen_td(&INV_SBOX, 2);
static TD3: [u32; 256] = gen_td(&INV_SBOX, 3);

const RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

#[inline(always)]
fn b0(w: u32) -> usize {
    (w >> 24) as usize
}
#[inline(always)]
fn b1(w: u32) -> usize {
    ((w >> 16) & 0xff) as usize
}
#[inline(always)]
fn b2(w: u32) -> usize {
    ((w >> 8) & 0xff) as usize
}
#[inline(always)]
fn b3(w: u32) -> usize {
    (w & 0xff) as usize
}

fn sub_word(w: u32) -> u32 {
    ((SBOX[b0(w)] as u32) << 24)
        | ((SBOX[b1(w)] as u32) << 16)
        | ((SBOX[b2(w)] as u32) << 8)
        | (SBOX[b3(w)] as u32)
}

// ---------------------------------------------------------------------------

/// An expanded AES key, usable for both encryption and decryption.
#[derive(Clone)]
pub struct Aes {
    /// Number of rounds: 10, 12 or 14.
    rounds: usize,
    /// Encryption round keys, 4 words per round plus the initial key.
    ek: [u32; 60],
    /// Decryption round keys (equivalent inverse cipher).
    dk: [u32; 60],
}

impl std::fmt::Debug for Aes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material.
        f.debug_struct("Aes")
            .field("bits", &(self.key_words() * 32))
            .finish_non_exhaustive()
    }
}

impl Aes {
    /// Expands a 16-, 24- or 32-byte key.
    pub fn new(key: &[u8]) -> Result<Aes> {
        let nk = match key.len() {
            16 => 4,
            24 => 6,
            32 => 8,
            n => return Err(Error::crypto(format!("aes: invalid key length {n}"))),
        };
        let rounds = nk + 6;
        let total = 4 * (rounds + 1);
        let mut ek = [0u32; 60];
        for (i, chunk) in key.chunks_exact(4).enumerate() {
            ek[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in nk..total {
            let mut t = ek[i - 1];
            if i % nk == 0 {
                t = sub_word(t.rotate_left(8)) ^ ((RCON[i / nk - 1] as u32) << 24);
            } else if nk > 6 && i % nk == 4 {
                t = sub_word(t);
            }
            ek[i] = ek[i - nk] ^ t;
        }

        let mut dk = [0u32; 60];
        for r in 0..=rounds {
            for c in 0..4 {
                let w = ek[4 * (rounds - r) + c];
                dk[4 * r + c] = if r == 0 || r == rounds {
                    w
                } else {
                    // InvMixColumns(w) via Td[S[b]] = InvMixColumns of byte b.
                    TD0[SBOX[b0(w)] as usize]
                        ^ TD1[SBOX[b1(w)] as usize]
                        ^ TD2[SBOX[b2(w)] as usize]
                        ^ TD3[SBOX[b3(w)] as usize]
                };
            }
        }
        Ok(Aes { rounds, ek, dk })
    }

    /// Key size in 32-bit words (4, 6 or 8).
    fn key_words(&self) -> usize {
        self.rounds - 6
    }

    /// Key length in bytes.
    pub fn key_len(&self) -> usize {
        self.key_words() * 4
    }

    /// Encrypts one block in place.
    #[inline]
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        let rk = &self.ek;
        let mut s0 = load(block, 0) ^ rk[0];
        let mut s1 = load(block, 1) ^ rk[1];
        let mut s2 = load(block, 2) ^ rk[2];
        let mut s3 = load(block, 3) ^ rk[3];
        let mut k = 4;
        for _ in 1..self.rounds {
            let t0 = TE0[b0(s0)] ^ TE1[b1(s1)] ^ TE2[b2(s2)] ^ TE3[b3(s3)] ^ rk[k];
            let t1 = TE0[b0(s1)] ^ TE1[b1(s2)] ^ TE2[b2(s3)] ^ TE3[b3(s0)] ^ rk[k + 1];
            let t2 = TE0[b0(s2)] ^ TE1[b1(s3)] ^ TE2[b2(s0)] ^ TE3[b3(s1)] ^ rk[k + 2];
            let t3 = TE0[b0(s3)] ^ TE1[b1(s0)] ^ TE2[b2(s1)] ^ TE3[b3(s2)] ^ rk[k + 3];
            s0 = t0;
            s1 = t1;
            s2 = t2;
            s3 = t3;
            k += 4;
        }
        let f = |a: u32, b: u32, c: u32, d: u32| -> u32 {
            ((SBOX[b0(a)] as u32) << 24)
                | ((SBOX[b1(b)] as u32) << 16)
                | ((SBOX[b2(c)] as u32) << 8)
                | (SBOX[b3(d)] as u32)
        };
        store(block, 0, f(s0, s1, s2, s3) ^ rk[k]);
        store(block, 1, f(s1, s2, s3, s0) ^ rk[k + 1]);
        store(block, 2, f(s2, s3, s0, s1) ^ rk[k + 2]);
        store(block, 3, f(s3, s0, s1, s2) ^ rk[k + 3]);
    }

    /// Encrypts four independent blocks in place. Same result as four
    /// [`encrypt_block`](Self::encrypt_block) calls, but the rounds are
    /// interleaved so the table lookups of different blocks overlap (used
    /// by CTR mode).
    #[inline]
    pub fn encrypt_blocks4(&self, blocks: &mut [[u8; 16]; 4]) {
        let rk = &self.ek;
        let mut s = [[0u32; 4]; 4];
        for (st, b) in s.iter_mut().zip(blocks.iter()) {
            for (c, w) in st.iter_mut().enumerate() {
                *w = load(b, c) ^ rk[c];
            }
        }
        let mut k = 4;
        for _ in 1..self.rounds {
            for st in s.iter_mut() {
                let [s0, s1, s2, s3] = *st;
                *st = [
                    TE0[b0(s0)] ^ TE1[b1(s1)] ^ TE2[b2(s2)] ^ TE3[b3(s3)] ^ rk[k],
                    TE0[b0(s1)] ^ TE1[b1(s2)] ^ TE2[b2(s3)] ^ TE3[b3(s0)] ^ rk[k + 1],
                    TE0[b0(s2)] ^ TE1[b1(s3)] ^ TE2[b2(s0)] ^ TE3[b3(s1)] ^ rk[k + 2],
                    TE0[b0(s3)] ^ TE1[b1(s0)] ^ TE2[b2(s1)] ^ TE3[b3(s2)] ^ rk[k + 3],
                ];
            }
            k += 4;
        }
        let f = |a: u32, b: u32, c: u32, d: u32| -> u32 {
            ((SBOX[b0(a)] as u32) << 24)
                | ((SBOX[b1(b)] as u32) << 16)
                | ((SBOX[b2(c)] as u32) << 8)
                | (SBOX[b3(d)] as u32)
        };
        for (st, b) in s.iter().zip(blocks.iter_mut()) {
            let [s0, s1, s2, s3] = *st;
            store(b, 0, f(s0, s1, s2, s3) ^ rk[k]);
            store(b, 1, f(s1, s2, s3, s0) ^ rk[k + 1]);
            store(b, 2, f(s2, s3, s0, s1) ^ rk[k + 2]);
            store(b, 3, f(s3, s0, s1, s2) ^ rk[k + 3]);
        }
    }

    /// Decrypts one block in place.
    #[inline]
    pub fn decrypt_block(&self, block: &mut [u8; 16]) {
        let rk = &self.dk;
        let mut s0 = load(block, 0) ^ rk[0];
        let mut s1 = load(block, 1) ^ rk[1];
        let mut s2 = load(block, 2) ^ rk[2];
        let mut s3 = load(block, 3) ^ rk[3];
        let mut k = 4;
        for _ in 1..self.rounds {
            let t0 = TD0[b0(s0)] ^ TD1[b1(s3)] ^ TD2[b2(s2)] ^ TD3[b3(s1)] ^ rk[k];
            let t1 = TD0[b0(s1)] ^ TD1[b1(s0)] ^ TD2[b2(s3)] ^ TD3[b3(s2)] ^ rk[k + 1];
            let t2 = TD0[b0(s2)] ^ TD1[b1(s1)] ^ TD2[b2(s0)] ^ TD3[b3(s3)] ^ rk[k + 2];
            let t3 = TD0[b0(s3)] ^ TD1[b1(s2)] ^ TD2[b2(s1)] ^ TD3[b3(s0)] ^ rk[k + 3];
            s0 = t0;
            s1 = t1;
            s2 = t2;
            s3 = t3;
            k += 4;
        }
        let f = |a: u32, b: u32, c: u32, d: u32| -> u32 {
            ((INV_SBOX[b0(a)] as u32) << 24)
                | ((INV_SBOX[b1(b)] as u32) << 16)
                | ((INV_SBOX[b2(c)] as u32) << 8)
                | (INV_SBOX[b3(d)] as u32)
        };
        store(block, 0, f(s0, s3, s2, s1) ^ rk[k]);
        store(block, 1, f(s1, s0, s3, s2) ^ rk[k + 1]);
        store(block, 2, f(s2, s1, s0, s3) ^ rk[k + 2]);
        store(block, 3, f(s3, s2, s1, s0) ^ rk[k + 3]);
    }
}

#[inline(always)]
fn load(b: &[u8; 16], i: usize) -> u32 {
    u32::from_be_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]])
}

#[inline(always)]
fn store(b: &mut [u8; 16], i: usize, w: u32) {
    b[4 * i..4 * i + 4].copy_from_slice(&w.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::vectors;

    #[test]
    fn sbox_spot_checks() {
        // FIPS-197 Figure 7 / Figure 14.
        assert_eq!(SBOX[0x00], 0x63);
        assert_eq!(SBOX[0x53], 0xed);
        assert_eq!(SBOX[0xff], 0x16);
        assert_eq!(INV_SBOX[0x63], 0x00);
        assert_eq!(INV_SBOX[0x00], 0x52);
    }

    #[test]
    fn key_expansion_fips197_a1() {
        // FIPS-197 Appendix A.1: last round key word w[43].
        let key = crate::util::hex::decode("2b7e151628aed2a6abf7158809cf4f3c").unwrap();
        let aes = Aes::new(&key).unwrap();
        assert_eq!(aes.ek[4], 0xa0fafe17);
        assert_eq!(aes.ek[43], 0xb6630ca6);
    }

    #[test]
    fn rejects_bad_key_length() {
        for n in [0, 15, 17, 31, 33] {
            assert!(Aes::new(&vec![0; n]).is_err());
        }
    }

    #[test]
    fn fips197_vectors() {
        let blocks = vectors::load("crypto/aes_fips197.txt");
        assert!(blocks.len() >= 3);
        for b in blocks {
            let aes = Aes::new(&b.hex("key")).unwrap();
            let pt: [u8; 16] = b.hex("pt").try_into().unwrap();
            let ct: [u8; 16] = b.hex("ct").try_into().unwrap();
            let mut blk = pt;
            aes.encrypt_block(&mut blk);
            assert_eq!(blk, ct, "encrypt {}", b.name());
            aes.decrypt_block(&mut blk);
            assert_eq!(blk, pt, "decrypt {}", b.name());
        }
    }

    #[test]
    fn decrypt_inverts_encrypt() {
        let mut x = 0x12345678u32;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        };
        for klen in [16, 24, 32] {
            let key: Vec<u8> = (0..klen).map(|_| next()).collect();
            let aes = Aes::new(&key).unwrap();
            for _ in 0..64 {
                let pt: [u8; 16] = std::array::from_fn(|_| next());
                let mut b = pt;
                aes.encrypt_block(&mut b);
                assert_ne!(b, pt);
                aes.decrypt_block(&mut b);
                assert_eq!(b, pt);
            }
            let pts: [[u8; 16]; 4] = std::array::from_fn(|_| std::array::from_fn(|_| next()));
            let mut four = pts;
            aes.encrypt_blocks4(&mut four);
            for (p, c) in pts.iter().zip(&four) {
                let mut one = *p;
                aes.encrypt_block(&mut one);
                assert_eq!(&one, c);
            }
        }
    }
}
