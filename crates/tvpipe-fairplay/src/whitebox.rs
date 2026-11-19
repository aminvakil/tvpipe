// SPDX-License-Identifier: LGPL-3.0-or-later
// Ported from doubletake internal/airplay/fpsap.go and fairplay_crypto.go
// @ be27776beb05beaa149a7096e4722f5cba60eb37 (LGPL-3.0-or-later).
//! The SAP-pair digests: the m3 tail (a white-box network keyed by a
//! descriptor of both SAP values), the ekey MAC key and the ekey wrapping
//! key.

use super::md5::{self, bytes_be, compress, le_word, words_le, Mutation};
use super::sap::sap_hash;
use super::tables::{
    NetworkTables, FIRST_INPUT_MASK, FIRST_TABLES, SECOND_OUTPUT_MASK, SECOND_TABLES,
};

const INITIAL_SESSION_KEY: [u8; 16] = [
    0xdc, 0xdc, 0xf3, 0xb9, 0x0b, 0x74, 0xdc, 0xfb, 0x86, 0x7f, 0xf7, 0x60, 0x16, 0x72, 0x90, 0x51,
];

const KDF_PREFIX: [u8; 17] = [
    0xfa, 0x9c, 0xad, 0x4d, 0x4b, 0x68, 0x26, 0x8c, 0x7f, 0xf3, 0x88, 0x99, 0xde, 0x92, 0x2e, 0x95,
    0x1e,
];

const KDF_SUFFIX: [u8; 17] = [
    0xec, 0x4e, 0x27, 0x5e, 0xfd, 0xf2, 0xe8, 0x30, 0x97, 0xae, 0x70, 0xfb, 0xe0, 0x00, 0x3f, 0x1c,
    0x39,
];

const DESCRIPTOR_PREFIX: [u8; 17] = [
    0xa0, 0x44, 0x9c, 0x4d, 0x09, 0xe4, 0xbd, 0x7f, 0x6e, 0xc5, 0xd0, 0xcc, 0x35, 0x9d, 0xa7, 0x46,
    0x7a,
];

const DESCRIPTOR_SUFFIX: [u8; 17] = [
    0x97, 0xb5, 0x0f, 0x84, 0xe2, 0x15, 0x5a, 0x9c, 0x24, 0x99, 0x1c, 0xf4, 0x3a, 0x09, 0x63, 0x55,
    0x47,
];

const FIXED_BLOCK: [u8; 16] = [
    0xaf, 0xc2, 0x2b, 0xa0, 0x49, 0xef, 0xfc, 0xfb, 0xfe, 0x67, 0xac, 0x5e, 0xbe, 0xf6, 0xfb, 0xcb,
];

const MASK_STATE: [u32; 4] = [0x1d4a4587, 0x92f39fcc, 0x1d87d836, 0xcdc86697];

const MASK_SUFFIX: [u8; 15] = [
    0x57, 0xd8, 0xee, 0xcb, 0xde, 0xfb, 0xcf, 0x59, 0x1c, 0x27, 0xa2, 0xcf, 0xbe, 0xb0, 0x89,
];

const DIGEST32_STATE: [u32; 4] = [0xb9f3dcdc, 0xfbdc740b, 0x60f77f86, 0x51907216];

const FIRST_POSITIONS: [usize; 16] = [0, 5, 10, 15, 4, 9, 14, 3, 8, 13, 2, 7, 12, 1, 6, 11];
const SECOND_POSITIONS: [usize; 16] = [0, 13, 10, 7, 4, 1, 14, 11, 8, 5, 2, 15, 12, 9, 6, 3];

/// `prefix || a || b || suffix`, MD5-padded to five 64-byte blocks.
fn padded_pair(
    prefix: &[u8; 17],
    a: &[u8; 128],
    b: &[u8; 128],
    suffix: &[u8; 17],
) -> [[u8; 64]; 5] {
    let mut flat = [0u8; 320];
    flat[..17].copy_from_slice(prefix);
    flat[17..145].copy_from_slice(a);
    flat[145..273].copy_from_slice(b);
    flat[273..290].copy_from_slice(suffix);
    flat[290] = 0x80;
    flat[312..].copy_from_slice(&(290u64 * 8).to_le_bytes());
    let mut blocks = [[0u8; 64]; 5];
    for (i, b) in blocks.iter_mut().enumerate() {
        b.copy_from_slice(&flat[64 * i..64 * i + 64]);
    }
    blocks
}

/// The 20-byte descriptor of a (sender SAP, receiver SAP) pair: seeds the
/// m3 tail network and is the ekey's HMAC-SHA1 key.
pub(super) fn descriptor(local: &[u8; 128], remote: &[u8; 128]) -> [u8; 20] {
    let blocks = padded_pair(&DESCRIPTOR_PREFIX, local, remote, &DESCRIPTOR_SUFFIX);
    let mut state = words_le(&INITIAL_SESSION_KEY);
    let mut first_final = [0u32; 4];
    for (i, block) in blocks.iter().enumerate() {
        let add = sap_hash(block);
        for (w, s) in state.iter_mut().enumerate() {
            *s = s.wrapping_add(le_word(&add, w));
        }
        state = compress(state, block, Mutation::Cycle);
        if i == blocks.len() - 1 {
            first_final = state;
            state = compress(state, block, Mutation::Cycle);
        }
    }
    let mut out = [0u8; 20];
    out[..4].copy_from_slice(&first_final[0].to_be_bytes());
    out[4..].copy_from_slice(&bytes_be(state));
    out
}

/// The AES-128 key that wraps the ekey's (key XOR mask).
pub(super) fn wrapping_key(local: &[u8; 128], remote: &[u8; 128]) -> [u8; 16] {
    let blocks = padded_pair(&KDF_PREFIX, local, remote, &KDF_SUFFIX);
    let mut state = words_le(&INITIAL_SESSION_KEY);
    for block in &blocks {
        let modified = compress(state, block, Mutation::Kdf);
        let hashed = sap_hash(block);
        for (w, s) in state.iter_mut().enumerate() {
            *s = modified[w].wrapping_add(le_word(&hashed, w));
        }
    }
    bytes_be(state)
}

/// The 20-byte m3 tail for a SAP pair.
pub(super) fn m3_tail(local: &[u8; 128], remote: &[u8; 128]) -> [u8; 20] {
    exchange(&descriptor(local, remote))
}

fn exchange(seed: &[u8; 20]) -> [u8; 20] {
    let masks = masks(seed);
    let intermediate = first_network(&masks);
    let left = digest32(&intermediate, &FIXED_BLOCK);
    let output = second_network(left, &masks);
    let digest = digest32(&left, &output);
    let mut out = [0u8; 20];
    out[..4].copy_from_slice(&output[..4]);
    out[4..].copy_from_slice(&digest);
    out
}

fn masks(seed: &[u8; 20]) -> [[u8; 16]; 9] {
    let mut masks = [[0u8; 16]; 9];
    for (i, mask) in masks.iter_mut().enumerate() {
        let mut block = [0u8; 64];
        block[..20].copy_from_slice(seed);
        block[20] = i as u8;
        block[21..36].copy_from_slice(&MASK_SUFFIX);
        block[36] = 0x80;
        block[56..60].copy_from_slice(&0x320u32.to_le_bytes());
        *mask = bytes_be(compress(MASK_STATE, &block, Mutation::Swap));
    }
    masks
}

fn digest32(left: &[u8; 16], right: &[u8; 16]) -> [u8; 16] {
    let mut block = [0u8; 64];
    block[..16].copy_from_slice(left);
    block[16..32].copy_from_slice(right);
    block[32] = 0x80;
    block[56..60].copy_from_slice(&0x100u32.to_le_bytes());
    md5::bytes_be(compress(DIGEST32_STATE, &block, Mutation::Swap))
}

fn first_network(masks: &[[u8; 16]; 9]) -> [u8; 16] {
    let t = &FIRST_TABLES;
    let mut state = FIXED_BLOCK;
    for (s, m) in state.iter_mut().zip(&FIRST_INPUT_MASK) {
        *s ^= m;
    }
    for (round, mask) in t.rounds.iter().zip(masks) {
        let mut substituted = [0u8; 16];
        for (output, &input) in FIRST_POSITIONS.iter().enumerate() {
            substituted[output] = round[input].substitute(state[input]);
        }
        mix(t, &mut state, &substituted);
        for (s, m) in state.iter_mut().zip(mask) {
            *s ^= m;
        }
    }
    let mut out = [0u8; 16];
    for (output, &input) in FIRST_POSITIONS.iter().enumerate() {
        out[output] = t.last[input].substitute(state[input]);
    }
    out
}

fn second_network(mut state: [u8; 16], masks: &[[u8; 16]; 9]) -> [u8; 16] {
    let t = &SECOND_TABLES;
    for bank in (0..9).rev() {
        let mut substituted = [0u8; 16];
        for (output, &input) in SECOND_POSITIONS.iter().enumerate() {
            substituted[output] =
                t.rounds[bank][output].substitute(state[input]) ^ masks[bank][output];
        }
        mix(t, &mut state, &substituted);
    }
    let mut out = [0u8; 16];
    for (output, &input) in SECOND_POSITIONS.iter().enumerate() {
        out[output] = t.last[output].substitute(state[input]) ^ SECOND_OUTPUT_MASK[output];
    }
    out
}

fn mix(t: &NetworkTables, state: &mut [u8; 16], substituted: &[u8; 16]) {
    for word in 0..4 {
        for out_byte in 0..4 {
            let mut mixed = 0;
            for in_byte in 0..4 {
                mixed ^= t.mix[in_byte][out_byte].mix(substituted[4 * word + in_byte]);
            }
            state[4 * word + out_byte] = mixed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::ring::Sha256;
    use crate::util::hex;

    /// Matches doubletake's TestFPSAPTableData expansion checksum, so the
    /// tables survived the port intact.
    #[test]
    fn table_checksum() {
        let mut hash = Sha256::new();
        let mut written = 0;
        let mut write = |d: &[u8]| {
            hash.update(d);
            written += d.len();
        };
        write(&FIRST_INPUT_MASK);
        for t in [&FIRST_TABLES, &SECOND_TABLES] {
            for round in &t.rounds {
                for l in round {
                    let e: Vec<u8> = (0..=255u8).map(|v| l.substitute(v)).collect();
                    write(&e);
                }
            }
            for input in &t.mix {
                let mut e = Vec::with_capacity(1024);
                for v in 0..=255u8 {
                    for l in input {
                        e.push(l.mix(v));
                    }
                }
                write(&e);
            }
            for l in &t.last {
                let e: Vec<u8> = (0..=255u8).map(|v| l.substitute(v)).collect();
                write(&e);
            }
        }
        assert_eq!(written, 90128);
        assert_eq!(
            hex::encode(&hash.finish()),
            "28d0986abebe30458348dfa2957aa1d52d6f3ad5a9468c5d8a9c4139b7ca2b43"
        );
    }
}
