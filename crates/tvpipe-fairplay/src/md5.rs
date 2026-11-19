// SPDX-License-Identifier: LGPL-3.0-or-later
// Ported from doubletake internal/airplay/fairplay_md5.go
// @ be27776beb05beaa149a7096e4722f5cba60eb37 (LGPL-3.0-or-later).
//! FairPlay's modified MD5 compression: standard rounds and constants, but
//! big-endian message words and a message-schedule mutation after round 31.

/// The schedule mutation applied after round 31.
#[derive(Clone, Copy, Debug)]
pub(super) enum Mutation {
    /// Swap word `i` with the word picked by nibble `i` of the state.
    Swap,
    /// Rotate the eight words picked by the state's nibbles.
    Cycle,
    /// Swap pairs of words picked by the state's nibbles (key derivation).
    Kdf,
}

const SHIFT: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, //
    5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, //
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, //
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// One compression of `block` into `state`.
pub(super) fn compress(state: [u32; 4], block: &[u8; 64], mutation: Mutation) -> [u32; 4] {
    let mut msg = [0u32; 16];
    for (i, w) in msg.iter_mut().enumerate() {
        *w = u32::from_be_bytes([
            block[4 * i],
            block[4 * i + 1],
            block[4 * i + 2],
            block[4 * i + 3],
        ]);
    }

    let [mut a, mut b, mut c, mut d] = state;
    for round in 0..64 {
        let (f, word) = match round {
            0..=15 => ((b & c) | (!b & d), round),
            16..=31 => ((d & b) | (!d & c), (5 * round + 1) & 15),
            32..=47 => (b ^ c ^ d, (3 * round + 5) & 15),
            _ => (c ^ (b | !d), (7 * round) & 15),
        };
        let mixed = a
            .wrapping_add(f)
            .wrapping_add(K[round])
            .wrapping_add(msg[word])
            .rotate_left(SHIFT[round]);
        let next_b = b.wrapping_add(mixed);
        a = d;
        d = c;
        c = b;
        b = next_b;

        if round == 31 {
            mutate(&mut msg, [a, b, c, d], mutation);
        }
    }

    [
        state[0].wrapping_add(a),
        state[1].wrapping_add(b),
        state[2].wrapping_add(c),
        state[3].wrapping_add(d),
    ]
}

fn mutate(msg: &mut [u32; 16], [a, b, c, d]: [u32; 4], mutation: Mutation) {
    let nib = |x: u32, shift: u32| ((x >> shift) & 15) as usize;
    match mutation {
        Mutation::Swap | Mutation::Cycle => {
            let idx = [
                nib(a, 0),
                nib(b, 0),
                nib(c, 0),
                nib(d, 0),
                nib(a, 4),
                nib(b, 4),
                nib(c, 4),
                nib(d, 4),
            ];
            if let Mutation::Swap = mutation {
                for (i, &j) in idx.iter().enumerate() {
                    msg.swap(i, j);
                }
            } else {
                let first = msg[idx[0]];
                for i in 0..idx.len() - 1 {
                    msg[idx[i]] = msg[idx[i + 1]];
                }
                msg[idx[7]] = first;
            }
        }
        Mutation::Kdf => {
            msg.swap(nib(a, 0), nib(b, 0));
            msg.swap(nib(c, 0), nib(d, 0));
            for shift in [4, 8, 12] {
                msg.swap(nib(a, shift), nib(b, shift));
            }
        }
    }
}

/// Little-endian words of a 16-byte key.
pub(super) fn words_le(bytes: &[u8; 16]) -> [u32; 4] {
    let mut out = [0u32; 4];
    for (i, w) in out.iter_mut().enumerate() {
        *w = u32::from_le_bytes([
            bytes[4 * i],
            bytes[4 * i + 1],
            bytes[4 * i + 2],
            bytes[4 * i + 3],
        ]);
    }
    out
}

/// The state as 16 big-endian bytes.
pub(super) fn bytes_be(words: [u32; 4]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (i, w) in words.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&w.to_be_bytes());
    }
    out
}

/// Bytes `4i..4i+4` of `bytes` as a little-endian word.
pub(super) fn le_word(bytes: &[u8; 16], i: usize) -> u32 {
    u32::from_le_bytes([
        bytes[4 * i],
        bytes[4 * i + 1],
        bytes[4 * i + 2],
        bytes[4 * i + 3],
    ])
}
