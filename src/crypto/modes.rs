//! Block cipher modes over [`Aes`]: CTR and CBC (NIST SP 800-38A).
//!
//! - [`Ctr`] is a stateful keystream with a 128-bit big-endian counter that
//!   wraps modulo 2^128. It keeps its byte position, so a continuous stream
//!   can be encrypted in arbitrary pieces (AirPlay mirroring), and it can be
//!   re-seeded per message (Cast frames). [`ctr_xor`] is the one-shot form.
//! - [`cbc_encrypt`] / [`cbc_decrypt`] work on whole blocks only and do no
//!   padding; the caller decides what to do with a trailing partial block.

use super::aes::{Aes, BLOCK_LEN};
use crate::error::{Error, Result};

/// AES in counter mode. Encryption and decryption are the same operation.
#[derive(Clone, Debug)]
pub struct Ctr {
    aes: Aes,
    /// Initial counter block.
    iv: u128,
    /// Keystream bytes consumed since `iv`.
    pos: u64,
    /// Keystream block for `iv + pos / 16`, valid when `pos % 16 != 0`.
    ks: [u8; BLOCK_LEN],
}

impl Ctr {
    /// Starts a keystream at counter block `iv`.
    pub fn new(aes: Aes, iv: &[u8; BLOCK_LEN]) -> Ctr {
        Ctr {
            aes,
            iv: u128::from_be_bytes(*iv),
            pos: 0,
            ks: [0; BLOCK_LEN],
        }
    }

    /// Convenience: expands `key` (16/24/32 bytes) and starts at `iv`.
    pub fn with_key(key: &[u8], iv: &[u8; BLOCK_LEN]) -> Result<Ctr> {
        Ok(Ctr::new(Aes::new(key)?, iv))
    }

    /// The underlying cipher (e.g. to start another stream with the same key).
    pub fn cipher(&self) -> &Aes {
        &self.aes
    }

    /// Restarts at a new initial counter block, keeping the key.
    pub fn reset(&mut self, iv: &[u8; BLOCK_LEN]) {
        self.iv = u128::from_be_bytes(*iv);
        self.pos = 0;
    }

    /// Number of keystream bytes consumed since the initial counter block.
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Moves to byte `offset` of the keystream (relative to the initial
    /// counter block).
    pub fn seek(&mut self, offset: u64) {
        self.pos = offset;
        if offset % BLOCK_LEN as u64 != 0 {
            self.ks = self.keystream_block(offset / BLOCK_LEN as u64);
        }
    }

    #[inline]
    fn keystream_block(&self, index: u64) -> [u8; BLOCK_LEN] {
        let mut b = self.iv.wrapping_add(index as u128).to_be_bytes();
        self.aes.encrypt_block(&mut b);
        b
    }

    /// XORs the keystream into `data` in place, advancing the position.
    pub fn apply(&mut self, mut data: &mut [u8]) {
        // Finish a partially used keystream block.
        let off = (self.pos % BLOCK_LEN as u64) as usize;
        if off != 0 {
            let n = (BLOCK_LEN - off).min(data.len());
            for (d, k) in data[..n].iter_mut().zip(&self.ks[off..off + n]) {
                *d ^= k;
            }
            self.pos += n as u64;
            data = &mut data[n..];
        }
        if data.is_empty() {
            return;
        }

        let mut ctr = self.iv.wrapping_add((self.pos / BLOCK_LEN as u64) as u128);

        // Whole blocks, four at a time so independent table lookups overlap.
        let mut chunks = data.chunks_exact_mut(4 * BLOCK_LEN);
        for chunk in &mut chunks {
            let mut ks = [[0u8; BLOCK_LEN]; 4];
            for (i, k) in ks.iter_mut().enumerate() {
                *k = ctr.wrapping_add(i as u128).to_be_bytes();
            }
            self.aes.encrypt_blocks4(&mut ks);
            for (blk, k) in chunk.chunks_exact_mut(BLOCK_LEN).zip(&ks) {
                xor16(blk, k);
            }
            ctr = ctr.wrapping_add(4);
        }
        let rest = chunks.into_remainder();
        let mut blocks = rest.chunks_exact_mut(BLOCK_LEN);
        for blk in &mut blocks {
            let mut k = ctr.to_be_bytes();
            self.aes.encrypt_block(&mut k);
            xor16(blk, &k);
            ctr = ctr.wrapping_add(1);
        }
        let tail = blocks.into_remainder();
        if !tail.is_empty() {
            let mut k = ctr.to_be_bytes();
            self.aes.encrypt_block(&mut k);
            for (d, k) in tail.iter_mut().zip(&k) {
                *d ^= k;
            }
            self.ks = k;
        }
        self.pos += data.len() as u64;
    }
}

#[inline(always)]
fn xor16(blk: &mut [u8], k: &[u8; BLOCK_LEN]) {
    let Ok(b): std::result::Result<&mut [u8; BLOCK_LEN], _> = blk.try_into() else {
        return;
    };
    let v = u128::from_ne_bytes(*b) ^ u128::from_ne_bytes(*k);
    *b = v.to_ne_bytes();
}

/// One-shot CTR: XORs the keystream starting at counter block `iv` into
/// `data`.
pub fn ctr_xor(aes: &Aes, iv: &[u8; BLOCK_LEN], data: &mut [u8]) {
    // Cloning the expanded key (~0.5 KiB) is cheap next to any real payload.
    Ctr::new(aes.clone(), iv).apply(data);
}

fn check_blocks(data: &[u8]) -> Result<()> {
    if data.len() % BLOCK_LEN != 0 {
        return Err(Error::crypto(format!(
            "aes-cbc: length {} is not a multiple of {BLOCK_LEN}",
            data.len()
        )));
    }
    Ok(())
}

/// CBC-encrypts whole blocks in place. On return `iv` holds the last
/// ciphertext block, so consecutive calls chain. Errors (and changes
/// nothing) if `data.len()` is not a multiple of 16.
pub fn cbc_encrypt(aes: &Aes, iv: &mut [u8; BLOCK_LEN], data: &mut [u8]) -> Result<()> {
    check_blocks(data)?;
    let mut prev = *iv;
    for blk in data.chunks_exact_mut(BLOCK_LEN) {
        let mut b = [0u8; BLOCK_LEN];
        for i in 0..BLOCK_LEN {
            b[i] = blk[i] ^ prev[i];
        }
        aes.encrypt_block(&mut b);
        blk.copy_from_slice(&b);
        prev = b;
    }
    *iv = prev;
    Ok(())
}

/// CBC-decrypts whole blocks in place. On return `iv` holds the last
/// ciphertext block, so consecutive calls chain. Errors (and changes
/// nothing) if `data.len()` is not a multiple of 16.
pub fn cbc_decrypt(aes: &Aes, iv: &mut [u8; BLOCK_LEN], data: &mut [u8]) -> Result<()> {
    check_blocks(data)?;
    let mut prev = *iv;
    for blk in data.chunks_exact_mut(BLOCK_LEN) {
        let mut c = [0u8; BLOCK_LEN];
        c.copy_from_slice(blk);
        let mut b = c;
        aes.decrypt_block(&mut b);
        for i in 0..BLOCK_LEN {
            blk[i] = b[i] ^ prev[i];
        }
        prev = c;
    }
    *iv = prev;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::vectors;

    fn arr16(v: Vec<u8>) -> [u8; 16] {
        v.try_into().unwrap()
    }

    #[test]
    fn sp800_38a_vectors() {
        let blocks = vectors::load("crypto/modes_sp800_38a.txt");
        assert!(blocks.len() >= 7);
        for b in blocks {
            let aes = Aes::new(&b.hex("key")).unwrap();
            let iv = arr16(b.hex("iv"));
            let pt = b.hex("pt");
            let ct = b.hex("ct");
            match b.str("mode") {
                "cbc" => {
                    let mut buf = pt.clone();
                    let mut v = iv;
                    cbc_encrypt(&aes, &mut v, &mut buf).unwrap();
                    assert_eq!(buf, ct, "{}", b.name());
                    assert_eq!(&v[..], &ct[ct.len() - 16..]);
                    let mut v = iv;
                    cbc_decrypt(&aes, &mut v, &mut buf).unwrap();
                    assert_eq!(buf, pt, "{}", b.name());

                    // Chained block by block gives the same result.
                    let mut buf = pt.clone();
                    let mut v = iv;
                    for blk in buf.chunks_mut(16) {
                        cbc_encrypt(&aes, &mut v, blk).unwrap();
                    }
                    assert_eq!(buf, ct, "{} chained", b.name());
                }
                "ctr" => {
                    let mut buf = pt.clone();
                    ctr_xor(&aes, &iv, &mut buf);
                    assert_eq!(buf, ct, "{}", b.name());
                    // Every split point, in pieces, must match one-shot.
                    for split in 0..=pt.len() {
                        let mut buf = pt.clone();
                        let mut c = Ctr::new(aes.clone(), &iv);
                        let (a, rest) = buf.split_at_mut(split);
                        c.apply(a);
                        c.apply(rest);
                        assert_eq!(buf, ct, "{} split {split}", b.name());
                        assert_eq!(c.position(), pt.len() as u64);
                    }
                    // Byte-at-a-time.
                    let mut buf = pt.clone();
                    let mut c = Ctr::new(aes.clone(), &iv);
                    for byte in buf.chunks_mut(1) {
                        c.apply(byte);
                    }
                    assert_eq!(buf, ct, "{} bytewise", b.name());
                    // Seek into the middle.
                    for off in [0usize, 5, 16, 17, 40] {
                        let mut buf = pt[off..].to_vec();
                        let mut c = Ctr::new(aes.clone(), &iv);
                        c.seek(off as u64);
                        c.apply(&mut buf);
                        assert_eq!(buf, ct[off..], "{} seek {off}", b.name());
                    }
                }
                m => panic!("unknown mode {m}"),
            }
        }
    }

    #[test]
    fn ctr_bulk_matches_blockwise() {
        // Exercise the 4-block path against the single-block path over
        // odd lengths and offsets.
        let aes = Aes::new(&[7u8; 16]).unwrap();
        let iv = [0xffu8; 16];
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 31) as u8).collect();
        let mut reference = data.clone();
        let mut c = Ctr::new(aes.clone(), &iv);
        for b in reference.chunks_mut(1) {
            c.apply(b);
        }
        for piece in [1usize, 3, 15, 16, 17, 63, 64, 65, 200] {
            let mut buf = data.clone();
            let mut c = Ctr::new(aes.clone(), &iv);
            for chunk in buf.chunks_mut(piece) {
                c.apply(chunk);
            }
            assert_eq!(buf, reference, "piece {piece}");
        }
    }

    #[test]
    fn ctr_reset() {
        let aes = Aes::new(&[1u8; 16]).unwrap();
        let mut c = Ctr::new(aes.clone(), &[0; 16]);
        let mut a = [0u8; 20];
        c.apply(&mut a);
        c.reset(&[0; 16]);
        let mut b = [0u8; 20];
        c.apply(&mut b);
        assert_eq!(a, b);
        assert_eq!(c.position(), 20);
    }

    #[test]
    fn cbc_rejects_partial_blocks() {
        let aes = Aes::new(&[0u8; 16]).unwrap();
        let mut iv = [0u8; 16];
        let mut buf = [1u8; 17];
        assert!(cbc_encrypt(&aes, &mut iv, &mut buf).is_err());
        assert!(cbc_decrypt(&aes, &mut iv, &mut buf).is_err());
        assert_eq!(buf, [1u8; 17]);
        assert_eq!(iv, [0u8; 16]);
        cbc_encrypt(&aes, &mut iv, &mut []).unwrap();
        assert_eq!(iv, [0u8; 16]);
    }

    /// Throughput check. Run with
    /// `cargo test --release ctr_throughput -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn ctr_throughput() {
        let aes = Aes::new(&[0x42u8; 16]).unwrap();
        let mut c = Ctr::new(aes, &[0; 16]);
        let mut buf = vec![0u8; 1 << 20];
        // Warm up.
        for _ in 0..16 {
            c.apply(&mut buf);
        }
        let rounds = 512;
        let start = std::time::Instant::now();
        for _ in 0..rounds {
            c.apply(&mut buf);
        }
        let secs = start.elapsed().as_secs_f64();
        let mbps = rounds as f64 / secs;
        println!("AES-128-CTR: {mbps:.0} MB/s ({rounds} MiB in {secs:.3}s)");
        std::hint::black_box(&buf);
        if !cfg!(debug_assertions) {
            assert!(mbps >= 200.0, "AES-128-CTR too slow: {mbps:.0} MB/s");
        }
    }
}
