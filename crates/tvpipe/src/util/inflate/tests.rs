use super::*;
use crate::crypto::ring::sha256;
use crate::util::{hex, vectors};

#[test]
fn zlib_vectors() {
    let blocks = vectors::load("inflate/zlib.txt");
    assert!(blocks.len() >= 10);
    for b in blocks {
        let len: usize = b.parse("len");
        let out = inflate(&b.hex("deflate"), len).unwrap_or_else(|e| panic!("{}: {e}", b.name()));
        assert_eq!(out.len(), len, "{}", b.name());
        assert_eq!(hex::encode(&sha256(&out)), b.str("sha256"), "{}", b.name());
        if b.get("plain").is_some() {
            assert_eq!(out, b.hex("plain"), "{}", b.name());
        }
        // One byte less than the output is refused.
        if len > 0 {
            assert!(inflate(&b.hex("deflate"), len - 1).is_err(), "{}", b.name());
        }
    }
}

#[test]
fn stored_blocks_hand_made() {
    // Two stored blocks: "ab" (not final), "c" (final).
    let data = [
        0x00, 2, 0, 0xfd, 0xff, b'a', b'b', 0x01, 1, 0, 0xfe, 0xff, b'c',
    ];
    assert_eq!(inflate(&data, 10).unwrap(), b"abc");
    // NLEN that is not the complement of LEN.
    let bad = [0x01, 2, 0, 0xfd, 0xfe, b'a', b'b'];
    assert!(inflate(&bad, 10).is_err());
}

#[test]
fn malformed_streams_are_errors() {
    let good = vectors::load("inflate/zlib.txt")
        .into_iter()
        .find(|b| b.name().starts_with("text, dynamic"))
        .unwrap()
        .hex("deflate");
    // Every truncation fails cleanly.
    for n in 0..good.len() {
        assert!(inflate(&good[..n], 1 << 20).is_err(), "truncated at {n}");
    }
    // Reserved block type 3.
    assert!(inflate(&[0x07], 100).is_err());
    // A distance before the start: fixed block, literal-free match.
    // Fixed code for length 3 (257) is 0000001, distance 0 is 00000.
    assert!(inflate(&[0x03, 0x02, 0x00], 100).is_err());
    // Flipped bits never panic.
    let mut rng = 0x1234_5678u32;
    for _ in 0..2000 {
        let mut d = good.clone();
        for _ in 0..3 {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let i = rng as usize % d.len();
            d[i] ^= 1 << (rng >> 29);
        }
        let _ = inflate(&d, 1 << 16);
    }
}
