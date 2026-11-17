// SPDX-License-Identifier: LGPL-3.0-or-later
// Ported from doubletake internal/airplay/fairplay_sap.go
// @ be27776beb05beaa149a7096e4722f5cba60eb37 (LGPL-3.0-or-later).
//! FairPlay's proprietary "SAP hash" of one 64-byte block (not a standard
//! hash). All arithmetic is on bytes and wraps, except where a step widens
//! explicitly before a division or an index.

const INITIAL_HASH: [u8; 20] = [
    0x96, 0x5f, 0xc6, 0x53, 0xf8, 0x46, 0xcc, 0x18, 0xdf, 0xbe, 0xb2, 0xf8, 0x38, 0x62, 0xec, 0x22,
    0x93, 0xd1, 0x20, 0x8f,
];

const INITIAL_MATRIX: [u8; 35] = [
    0x43, 0x54, 0x62, 0x7a, 0x18, 0xc3, 0xd6, 0xb3, 0x9a, 0x56, 0xf6, 0x1c, 0x14, 0x3f, 0x0c, 0x1d,
    0x3b, 0x36, 0x83, 0xb1, 0x39, 0x51, 0x4a, 0xaa, 0x09, 0x3e, 0xfe, 0x44, 0xaf, 0xde, 0xc3, 0x20,
    0x9d, 0x42, 0xb8,
];

const SEED: [u8; 21] = [
    0xed, 0x25, 0xd1, 0xbb, 0xbc, 0x27, 0x9f, 0x02, 0xa2, 0xa9, 0x11, 0x00, 0x0c, 0xb3, 0x52, 0xc0,
    0xbd, 0xe3, 0x1b, 0x49, 0xc7,
];

fn add(a: u8, b: u8) -> u8 {
    a.wrapping_add(b)
}

fn sub(a: u8, b: u8) -> u8 {
    a.wrapping_sub(b)
}

fn mul(a: u8, b: u8) -> u8 {
    a.wrapping_mul(b)
}

fn neg(a: u8) -> u8 {
    a.wrapping_neg()
}

fn sq(v: u8) -> u8 {
    mul(v, v)
}

fn cube(v: u8) -> u8 {
    mul(mul(v, v), v)
}

/// Bitwise majority of three bytes.
fn mj(a: u8, b: u8, c: u8) -> u8 {
    a ^ ((a ^ b) & (a ^ c))
}

/// Bits of `if_set` where `mask` is set, of `if_clear` elsewhere.
fn sel(mask: u8, if_set: u8, if_clear: u8) -> u8 {
    if_clear ^ ((if_set ^ if_clear) & mask)
}

/// Rotate left by `count` (mod 8), but 0 when `count` is 0.
fn roz(input: u8, count: u8) -> u8 {
    if count == 0 {
        0
    } else {
        input.rotate_left(count as u32)
    }
}

/// A seed byte indexed by `input` rotated in a wider integer
/// (`count` is always below 8).
fn wide_seed(input: u8, count: u8) -> u8 {
    if count == 0 {
        return SEED[0];
    }
    let count = (count & 7) as u32;
    let i = input as usize;
    SEED[((i << count) | (i >> (8 - count))) % SEED.len()]
}

/// The SAP hash of one block.
pub(super) fn sap_hash(block: &[u8; 64]) -> [u8; 16] {
    let mut hash = INITIAL_HASH;
    let mut matrix = INITIAL_MATRIX;
    let mut aux = [0u8; 10];
    let mut work = [0u8; 210];

    for (i, w) in work.iter_mut().enumerate() {
        *w = block[(i & 63) ^ 3];
    }
    // The indices wrap as 32-bit unsigned integers, which changes the
    // first of the four passes.
    for i in 0u32..840 {
        let x = work[(i.wrapping_sub(155) % 210) as usize];
        let y = work[(i.wrapping_sub(57) % 210) as usize];
        let z = work[(i.wrapping_sub(13) % 210) as usize];
        let w = work[(i % 210) as usize];
        work[(i % 210) as usize] = sub(
            add(y.rotate_left(5), z.rotate_left(3) ^ w),
            x.rotate_left(7),
        );
    }

    circuit(&mut hash, &mut matrix, &mut aux, &mut work);

    let mut out = [0u8; 16];
    out[..3].copy_from_slice(&aux[..3]);
    out[4..11].copy_from_slice(&aux[3..]);
    for b in out.iter_mut() {
        *b = add(*b, 0xe1);
    }
    out[3] = 0x3d;
    out[11] = 0x3c;
    out[10] ^= aux[3] ^ 133;

    for (i, &w) in work.iter().enumerate() {
        let mut value = w;
        if i < matrix.len() {
            value ^= matrix[i];
        }
        if i < hash.len() {
            value ^= hash[i];
        }
        out[i & 15] ^= value;
    }

    for i in 0..256usize {
        out[i & 15] ^= out[(i + 16 - 7) & 15].rotate_left(1)
            ^ out[(i + 16 - 5) & 15].rotate_left(6)
            ^ out[(i + 16 - 1) & 15].rotate_left(5);
    }
    out
}

/// The nonlinear mixing of hash/matrix/aux state from the work buffer.
#[allow(clippy::many_single_char_names)]
fn circuit(hash: &mut [u8; 20], matrix: &mut [u8; 35], aux: &mut [u8; 10], work: &mut [u8; 210]) {
    // hi/si index hash/seed directly; h/m/s index hash/matrix/seed through
    // a work byte; ma indexes matrix through an aux byte.
    macro_rules! hi {
        ($i:expr) => {
            hash[(($i) as usize) % 20]
        };
    }
    macro_rules! si {
        ($i:expr) => {
            SEED[(($i) as usize) % 21]
        };
    }
    macro_rules! h {
        ($i:expr) => {
            hi!(work[$i])
        };
    }
    macro_rules! m {
        ($i:expr) => {
            matrix[(work[$i] as usize) % 35]
        };
    }
    macro_rules! s {
        ($i:expr) => {
            si!(work[$i])
        };
    }
    macro_rules! ma {
        ($i:expr) => {
            matrix[(aux[$i] as usize) % 35]
        };
    }

    matrix[12] = add(
        0x14,
        sel(92, work[64], work[99] / 3) & wide_seed(s!(206), 4),
    );
    work[4] = mul(2, sq(work[99] / 5));
    work[153] ^= mul(sq(m!(203)), work[190]);
    hash[3] = 0x13 ^ ((s!(205) >> 1) & 0x10);
    work[33] = sub(work[33], s!(36) & !9);
    aux[5] = sub(
        (m!(67) & !2) | 1 | ((h!(181) >> 6) & 2) | (hash[3] & 0x10),
        15,
    );
    matrix[12] = 0x07;
    work[2] = sub(work[2], 64);
    hash[19] = s!(58);
    aux[4] = sub(92, m!(32));
    aux[9] = add(m!(15), 0x9e);
    work[34] = add(work[34], si!(aux[9]) / 5);
    hash[19] = add(hash[19], 0xe6 ^ ((hi!(aux[9]) >> 1) & 0x66));
    work[15] ^= sub(mul(3, roz(work[72], neg(s!(190)) & 7)), mul(9, s!(126)));
    hash[15] ^= cube(m!(181));
    matrix[4] ^= work[202] / 3;
    matrix[1] = add(matrix[1], cube(mj(sub(92, hi!(aux[4])), !work[105], 0xc6)));
    hash[19] ^= (((224 | (s!(92) & 27)) as u32 * m!(41) as u32) / 3) as u8;
    work[140] = add(work[140], roz(92, neg(work[5]) & 7));
    matrix[12] = add(matrix[12], mj((!work[4]) ^ m!(12), work[182], 192));
    work[36] = add(work[36], 125);
    work[124] = mj(mj(work[138], hash[15], 74), h!(43), 95).rotate_left(4);
    let aux_hash = hi!(aux[9]);
    aux[1] = 0x4c & !(aux_hash & (s!(68) << 1));
    aux[2] = sub(
        222,
        mj(
            ((work[177] as u32 + s!(79) as u32) >> 1) as u8,
            ((3 * work[148] as u32) / 5) as u8,
            matrix[1],
        ),
    );
    matrix[16] = add(
        matrix[16],
        sub(
            (ma!(4) & !0x60) | aux_hash | 8,
            work[33].rotate_left(2) | 128,
        ),
    );
    hash[14] ^= ma!(2);
    work[19] = add(
        work[19],
        mj(
            roz(si!(h!(201)), (m!(112) << 1) & 6),
            ((h!(208) & !0x7c) | (h!(164) & 0x7c)) / 5,
            37,
        ),
    );
    matrix[8] = roz(140, neg(sq(s!(45))) & 7) ^ aux[4];
    work[190] = 56;
    work[53] = !((h!(83) | 204) / 5);
    hash[13] = add(hash[13], h!(41));
    hash[10] = mj(ma!(4), work[2], aux[2]) / 15;
    aux[3] = sub(92, sq(0x28 | (ma!(1) & (0x12 | (s!(2) & 4)))));
    let seed_bits = si!(aux[4]);
    matrix[13] ^= seed_bits;
    aux[6] = add(92, sq(mj(sub(m!(179), 38), aux[2], 177)));
    let expansion_bits = mj(add(aux[3], aux[4] & 74), !seed_bits, 121);
    work[47] ^= add(m!(89), mj(expansion_bits ^ 0xa6, aux[4], 4));
    aux[7] = sub(
        sub(seed_bits / 3, ma!(9)),
        0x14 | (work[151] & ((aux[4] & 0x88) | 0x62)) | (aux[4] & 0x22),
    );
    let expanded_selector = expansion_bits ^ ((aux[4] & 0xca) >> 1) ^ 75;
    aux[9] = add(
        aux[9],
        0x80 | (mj(aux[7], work[151], 0x20) & 0x64) | (seed_bits & 0x44) | (ma!(9) & 0x1b),
    );
    matrix[33] ^= work[26];
    matrix[30] = sub(aux[9] / 3, (aux[4] & !8) | 0x13) ^ h!(122);
    work[22] = (m!(90) & 0x1b) | 0x44;
    let wide = sel(71, matrix[(expanded_selector as usize) % 35], si!(aux[5])) as u32;
    matrix[18] = add(matrix[18], ((wide * wide * wide) >> 1) as u8);
    matrix[5] = sub(matrix[5], s!(92));
    matrix[18] ^= mul(
        sel(aux[3], ma!(3), sel(16, m!(183), work[41])),
        sel(expanded_selector, h!(59), work[17]),
    );
    matrix[22] = add(
        mj(
            sel(hash[14] | 28, (work[7] & 28) | 0x82, h!(93)),
            roz(ma!(4), roz(work[11], neg(m!(28)) & 7) & 7),
            matrix[33],
        ),
        74,
    );
    hash[15] = sub(
        hash[15],
        mj(mj(aux[3], aux[4], 214), si!(h!(39) ^ 217), aux[6]),
    );

    let hash9 = hi!(aux[9]);
    let indexed_hash = hi!(sub(aux[4] / 3, aux[9] | work[22])
        ^ aux[6]
        ^ (((m!(57) | hash9) & (0x52 | (aux[9] & 0x0d))) | (((m!(57) & hash9) | aux[9]) & 0x20)));
    aux[6] = sq(sq(h!(99))) | ma!(9);
    aux[1] = add(
        aux[1],
        add(
            roz(h!(151) | s!(202), h!(50) & 7),
            mj(
                h!(4),
                ((sel(matrix[16], indexed_hash, m!(138)) as u32 + sel(17, work[33], s!(39)) as u32)
                    / 5) as u8,
                147,
            ),
        ),
    );
    aux[0] = sel(
        hash[10] & 7,
        ma!(6) & h!(209),
        sel(0x47, roz(s!(127), ma!(6) & 7), si!(ma!(5)) << 1),
    );
    let selected_square = sel(198, sq(m!(14)), h!(145) ^ aux[0]);
    let seed9 = si!(aux[9]);
    let hash3 = hi!(aux[3]);
    matrix[2] = add(
        matrix[2],
        ((hash3 << 1) & ((work[25] & 0x96) | (seed9 & 8))) | (seed9 & 0x40),
    );
    matrix[14] = sub(matrix[14], sel(34, work[97], ma!(3) & (aux[0] ^ m!(100))));
    work[23] ^= mj(mj(s!(17), hash3, aux[0]), work[50] / 3, 0x76) << 1;
    hash[17] = 115;
    hash[13] = ((mj(hi!(aux[7]), work[10], 82) >> 1) & 0x68) | (h!(39) & 0x17);
    matrix[33] = sub(matrix[33], work[113] & 9);
    matrix[28] = sub(matrix[28], (aux[3] & !0x20) | ((work[110] >> 1) & 0x20));
    work[95] = si!(aux[3]);
    hash[15] = mj(sub(work[95], 48), !work[184], 189) & cube(mj(aux[7], si!(aux[1]), 0xaa));
    matrix[22] = add(matrix[22], work[183]);
    aux[4] ^= mul(3, s!(1));
    aux[5] = add(
        aux[5],
        mul(mul(mul(198, mj(s!(178), ma!(1), 209)), h!(13)), s!(26) >> 1),
    );
    aux[8] = sel(10, ma!(3), ma!(9));
    matrix[18] = sub(
        matrix[18],
        sel(hash[15], aux[5] / 15, cube(hi!(aux[6]) | 81)),
    );
    aux[1] = add(aux[1], sub(si!(hi!(aux[1])) / 3, h!(160)));
    hash[16] = sub(
        147,
        mj(
            aux[0],
            mj(s!(69), work[172], add(sub(aux[2], selected_square), 77)),
            0xc2 | (aux[0] & 5),
        ),
    );
    hash[3] = sub(
        hash[3],
        wide_seed(mj(s!(155), work[105], 141), mj(s!(168), h!(29), 6) & 7),
    );
    work[5] = roz(0x38, neg(h!(61) / 5) & 7) ^ ((!ma!(8)) / 5);
    work[198] = add(work[198], work[3]);
    let wide = (162 | ma!(9)) as u32;
    work[164] = add(work[164], ((wide * wide) / 5) as u8);
    aux[2] = mj(roz(139, neg(aux[5]) & 6), hi!(aux[3]), 12) | sel(95, cube(seed9), hi!(aux[7]));
    matrix[12] = add(
        matrix[12],
        (16 | ((work[103] | 60) & (aux[2] | (work[103] & 32)))) / 3,
    );
    work[143] = sub(
        work[143],
        0x12 | (sel(aux[9], sel(matrix[8], work[35], aux[7]), aux[8] / 3)
            & (0x4d | ((work[172] >> 1) & 0x20))),
    );
    matrix[29] = 162;
    hash[15] = add(
        hash[15],
        mj(
            m!(149) ^ sq(work[43]),
            sel(95, h!(125), si!(aux[1])) >> 1,
            115,
        ),
    );
    aux[9] = sub(aux[9], hi!(aux[7]));
    hash[7] = sub(hash[7], sq(roz(ma!(5), mul(neg(m!(17)), m!(17) & 1))));
    matrix[8] = add(matrix[8], sub(cube(s!(202)), work[184]));
    hash[16] = (m!(102) << 1) & 0x84;
    aux[6] ^= si!(aux[7]) >> 1;
    hash[7] = sub(
        hash[7],
        sub(h!(191), sel(177, si!(si!(aux[1])), s!(80) << 1)),
    );
    hash[6] = h!(119);
    hash[12] =
        (hi!(aux[8]) ^ add(m!(71), m!(15))) & mj((work[118] & !0x2c) | 2, sq(hi!(aux[9])), 27);
    let digest_index = sel(0xa9, mul(s!(57), 231), mj(work[32], ma!(1), 23)) / 5;
    let seed_sample = si!(aux[6]);
    aux[5] = mj(
        (seed_sample & 0x1c) | (h!(82) & 0xa2) | (si!(digest_index) & 0x41),
        mj(cube(hi!(aux[7])), work[82], 92),
        192,
    ) ^ digest_index;
    matrix[25] ^= sub(
        mul(mul(2, hi!(aux[9])), work[5]),
        roz(aux[4], seed_sample & 7) & add(aux[3], 110),
    );
}
