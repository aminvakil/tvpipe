//! DEFLATE compression (RFC 1951) for the zip bundles `tvpipe bundle`
//! writes.
//!
//! LZ77 over a 32 KiB window (hash chains of three-byte prefixes, a
//! bounded chain walk, one step of lazy matching), then blocks of up to
//! [`BLOCK_SYMBOLS`] symbols, each written the cheapest of three ways:
//! stored, the fixed code, or dynamic Huffman codes (length-limited to
//! 15 bits, the code-length code to 7). Everything is deterministic: the
//! same input gives the same bytes, on every platform and in every
//! version that keeps these constants. Not fast, not zlib's ratio, but
//! within a few percent of `zlib -6` on HTML, JS and CSS.
//!
//! Sources: RFC 1951 (the format, §3.2.5 the length and distance
//! tables, §3.2.6 the fixed code, §3.2.7 dynamic blocks); the
//! length-limiting step is the Kraft-sum repair that miniz and stb use.

/// Literal/length and distance symbols per block at most.
pub const BLOCK_SYMBOLS: usize = 16 * 1024;
const WINDOW: usize = 32 * 1024;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const HASH_BITS: u32 = 15;
/// Positions a match search looks at at most.
const MAX_CHAIN: usize = 128;
/// A match at least this long is taken without a lazy look one byte on.
const GOOD_ENOUGH: usize = 64;

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// One LZ77 symbol: a literal, or a match (`len` 3..=258, `dist` 1..=32768).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    Literal(u8),
    Match { len: u16, dist: u16 },
}

fn len_code(len: u16) -> usize {
    // The last base not above `len` (258 has a code of its own).
    LEN_BASE.iter().rposition(|&b| b <= len).unwrap_or(0)
}

fn dist_code(dist: u16) -> usize {
    DIST_BASE.iter().rposition(|&b| b <= dist).unwrap_or(0)
}

/// LSB-first bit output.
struct BitWriter {
    out: Vec<u8>,
    buf: u64,
    count: u32,
}

impl BitWriter {
    fn put(&mut self, value: u32, bits: u32) {
        debug_assert!(bits <= 32);
        self.buf |= u64::from(value) << self.count;
        self.count += bits;
        while self.count >= 8 {
            self.out.push(self.buf as u8);
            self.buf >>= 8;
            self.count -= 8;
        }
    }

    /// A Huffman code: written most significant bit first.
    fn code(&mut self, code: u16, len: u8) {
        let rev = u32::from(code.reverse_bits()) >> (16 - u32::from(len));
        self.put(rev, u32::from(len));
    }

    fn align(&mut self) {
        if self.count > 0 {
            self.out.push(self.buf as u8);
            self.buf = 0;
            self.count = 0;
        }
    }
}

/// Compresses `data` into a raw DEFLATE stream (no zlib or gzip header).
pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut w = BitWriter {
        out: Vec::with_capacity(data.len() / 3 + 16),
        buf: 0,
        count: 0,
    };
    let tokens = lz77(data);
    if tokens.is_empty() {
        // One empty fixed block: the end-of-block code alone.
        w.put(1, 1);
        w.put(1, 2);
        w.code(0, 7);
        w.align();
        return w.out;
    }
    let mut pos = 0usize;
    let chunks: Vec<&[Token]> = tokens.chunks(BLOCK_SYMBOLS).collect();
    for (i, block) in chunks.iter().enumerate() {
        let len: usize = block
            .iter()
            .map(|t| match t {
                Token::Literal(_) => 1,
                Token::Match { len, .. } => usize::from(*len),
            })
            .sum();
        write_block(&mut w, block, &data[pos..pos + len], i + 1 == chunks.len());
        pos += len;
    }
    w.align();
    w.out
}

fn hash(d: &[u8], i: usize) -> usize {
    let v = u32::from(d[i]) << 16 | u32::from(d[i + 1]) << 8 | u32::from(d[i + 2]);
    (v.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
}

/// The LZ77 pass: hash chains, a lazy step.
fn lz77(data: &[u8]) -> Vec<Token> {
    let n = data.len();
    let mut out = Vec::with_capacity(n / 2);
    // `head[h]`, `prev[i % WINDOW]`: positions + 1 (0: none).
    let mut head = vec![0u32; 1 << HASH_BITS];
    let mut prev = vec![0u32; WINDOW];
    let insert = |head: &mut Vec<u32>, prev: &mut Vec<u32>, i: usize| {
        if i + MIN_MATCH <= n {
            let h = hash(data, i);
            prev[i % WINDOW] = head[h];
            head[h] = (i + 1) as u32;
        }
    };
    let find = |head: &Vec<u32>, prev: &Vec<u32>, i: usize| -> (usize, usize) {
        if i + MIN_MATCH > n {
            return (0, 0);
        }
        let max = (n - i).min(MAX_MATCH);
        let (mut best, mut best_dist) = (0usize, 0usize);
        let mut cand = head[hash(data, i)] as usize;
        let mut chain = MAX_CHAIN;
        while cand > 0 && chain > 0 {
            let j = cand - 1;
            if j >= i || i - j > WINDOW {
                break;
            }
            if data[j + best] == data[i + best] || best == 0 {
                let l = data[j..j + max]
                    .iter()
                    .zip(&data[i..i + max])
                    .take_while(|(a, b)| a == b)
                    .count();
                if l > best {
                    best = l;
                    best_dist = i - j;
                    if l == max {
                        break;
                    }
                }
            }
            let next = prev[j % WINDOW] as usize;
            // Chains only go back; an entry overwritten by a newer
            // position points forward and ends the walk.
            if next >= cand {
                break;
            }
            cand = next;
            chain -= 1;
        }
        if best >= MIN_MATCH {
            (best, best_dist)
        } else {
            (0, 0)
        }
    };
    let mut i = 0;
    while i < n {
        let (len, dist) = find(&head, &prev, i);
        if len == 0 {
            out.push(Token::Literal(data[i]));
            insert(&mut head, &mut prev, i);
            i += 1;
            continue;
        }
        if len < GOOD_ENOUGH && i + 1 < n {
            // Lazy: a longer match one byte on wins over this one.
            insert(&mut head, &mut prev, i);
            let (len2, dist2) = find(&head, &prev, i + 1);
            if len2 > len {
                out.push(Token::Literal(data[i]));
                out.push(Token::Match {
                    len: len2 as u16,
                    dist: dist2 as u16,
                });
                for k in i + 1..i + 1 + len2 {
                    insert(&mut head, &mut prev, k);
                }
                i += 1 + len2;
                continue;
            }
            out.push(Token::Match {
                len: len as u16,
                dist: dist as u16,
            });
            for k in i + 1..i + len {
                insert(&mut head, &mut prev, k);
            }
            i += len;
            continue;
        }
        out.push(Token::Match {
            len: len as u16,
            dist: dist as u16,
        });
        for k in i..i + len {
            insert(&mut head, &mut prev, k);
        }
        i += len;
    }
    out
}

/// Code lengths for `freq`, at most `limit` bits; every used symbol gets
/// a code, and at least two symbols do (a complete code for every
/// decoder).
fn huffman_lengths(freq: &[u32], limit: u8) -> Vec<u8> {
    let mut freq = freq.to_vec();
    let used = freq.iter().filter(|&&f| f > 0).count();
    for _ in used..2 {
        if let Some(f) = freq.iter_mut().find(|f| **f == 0) {
            *f = 1;
        }
    }
    // Symbols by (frequency, symbol): the merge order and the order
    // lengths are handed out in, both deterministic.
    let mut syms: Vec<usize> = (0..freq.len()).filter(|&s| freq[s] > 0).collect();
    syms.sort_by_key(|&s| (freq[s], s));
    // Huffman's algorithm with two queues (leaves sorted, internal nodes
    // made in increasing weight order).
    let leaves: Vec<(u64, usize)> = syms.iter().map(|&s| (u64::from(freq[s]), s)).collect();
    let mut parent: Vec<usize> = vec![usize::MAX; leaves.len() * 2];
    let mut nodes: Vec<u64> = Vec::new(); // internal node weights
    let (mut li, mut ni) = (0usize, 0usize);
    let nl = leaves.len();
    // Node ids: leaves 0..nl, internal nodes nl.. in creation order.
    let take = |nodes: &Vec<u64>, li: &mut usize, ni: &mut usize| -> (u64, usize) {
        let leaf = (*li < nl).then(|| leaves[*li].0);
        let node = (*ni < nodes.len()).then(|| nodes[*ni]);
        match (leaf, node) {
            (Some(l), Some(m)) if l <= m => {
                *li += 1;
                (l, *li - 1)
            }
            (Some(l), None) => {
                *li += 1;
                (l, *li - 1)
            }
            (_, Some(m)) => {
                *ni += 1;
                (m, nl + *ni - 1)
            }
            (None, None) => (0, usize::MAX),
        }
    };
    while (nl - li) + (nodes.len() - ni) > 1 {
        let (w1, a) = take(&nodes, &mut li, &mut ni);
        let (w2, b) = take(&nodes, &mut li, &mut ni);
        let id = nl + nodes.len();
        nodes.push(w1 + w2);
        parent[a] = id;
        parent[b] = id;
    }
    // Depths: internal nodes were made after their children, so walk
    // from the root (the last one) down.
    let total = nl + nodes.len();
    let mut depth = vec![0u32; total];
    for id in (0..total.saturating_sub(1)).rev() {
        depth[id] = depth[parent[id]] + 1;
    }
    // Count per length, clamp to `limit`, repair the Kraft sum.
    let limit = usize::from(limit);
    let mut count = vec![0u32; limit.max(32) + 1];
    for d in depth.iter().take(nl) {
        count[(*d as usize).min(limit)] += 1;
    }
    let mut kraft: u64 = (1..=limit)
        .map(|l| u64::from(count[l]) << (limit - l))
        .sum();
    while kraft > 1 << limit {
        count[limit] -= 1;
        for l in (1..limit).rev() {
            if count[l] > 0 {
                count[l] -= 1;
                count[l + 1] += 2;
                break;
            }
        }
        kraft -= 1;
    }
    // Hand out: the rarest symbols get the longest codes.
    let mut lengths = vec![0u8; freq.len()];
    let mut it = syms.iter();
    for l in (1..=limit).rev() {
        for _ in 0..count[l] {
            if let Some(&s) = it.next() {
                lengths[s] = l as u8;
            }
        }
    }
    lengths
}

/// Canonical codes from lengths (RFC 1951 §3.2.2).
fn canonical(lengths: &[u8]) -> Vec<u16> {
    let mut count = [0u16; 16];
    for &l in lengths {
        count[l as usize] += 1;
    }
    count[0] = 0;
    let mut next = [0u16; 16];
    let mut code = 0u16;
    for bits in 1..16 {
        code = (code + count[bits - 1]) << 1;
        next[bits] = code;
    }
    lengths
        .iter()
        .map(|&l| {
            if l == 0 {
                0
            } else {
                let c = next[l as usize];
                next[l as usize] += 1;
                c
            }
        })
        .collect()
}

fn fixed_lengths() -> (Vec<u8>, Vec<u8>) {
    let lit = (0..288)
        .map(|i| match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        })
        .collect();
    (lit, vec![5u8; 30])
}

/// The code-length sequence run-length encoded: (symbol, extra bits value).
fn rle_lengths(lengths: &[u8]) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lengths.len() {
        let l = lengths[i];
        let run = lengths[i..].iter().take_while(|&&x| x == l).count();
        if l == 0 && run >= 3 {
            let r = run.min(138);
            if r >= 11 {
                out.push((18, (r - 11) as u8));
            } else {
                out.push((17, (r - 3) as u8));
            }
            i += r;
        } else if l != 0 && run >= 4 {
            out.push((l, 0));
            let r = (run - 1).min(6);
            out.push((16, (r - 3) as u8));
            i += 1 + r;
        } else {
            out.push((l, 0));
            i += 1;
        }
    }
    out
}

fn extra_bits(sym: u8) -> u32 {
    match sym {
        16 => 2,
        17 => 3,
        18 => 7,
        _ => 0,
    }
}

/// The bits of `tokens` (and end-of-block) with these lengths.
fn data_bits(tokens: &[Token], lit: &[u8], dist: &[u8]) -> u64 {
    let mut bits = u64::from(lit[256]);
    for t in tokens {
        bits += match *t {
            Token::Literal(b) => u64::from(lit[usize::from(b)]),
            Token::Match { len, dist: d } => {
                let lc = len_code(len);
                let dc = dist_code(d);
                u64::from(lit[257 + lc])
                    + u64::from(LEN_EXTRA[lc])
                    + u64::from(dist[dc])
                    + u64::from(DIST_EXTRA[dc])
            }
        };
    }
    bits
}

fn write_block(w: &mut BitWriter, tokens: &[Token], raw: &[u8], last: bool) {
    let mut lfreq = vec![0u32; 286];
    let mut dfreq = vec![0u32; 30];
    for t in tokens {
        match *t {
            Token::Literal(b) => lfreq[usize::from(b)] += 1,
            Token::Match { len, dist } => {
                lfreq[257 + len_code(len)] += 1;
                dfreq[dist_code(dist)] += 1;
            }
        }
    }
    lfreq[256] = 1;
    let llen = huffman_lengths(&lfreq, 15);
    let dlen = huffman_lengths(&dfreq, 15);
    let nlit = 257.max(llen.iter().rposition(|&l| l > 0).map_or(0, |p| p + 1));
    let ndist = 1.max(dlen.iter().rposition(|&l| l > 0).map_or(0, |p| p + 1));
    let mut all = llen[..nlit].to_vec();
    all.extend_from_slice(&dlen[..ndist]);
    let rle = rle_lengths(&all);
    let mut cfreq = vec![0u32; 19];
    for &(s, _) in &rle {
        cfreq[usize::from(s)] += 1;
    }
    let clen = huffman_lengths(&cfreq, 7);
    let ncl = 4.max(
        CL_ORDER
            .iter()
            .rposition(|&s| clen[s] > 0)
            .map_or(0, |p| p + 1),
    );
    let header: u64 = 14
        + 3 * ncl as u64
        + rle
            .iter()
            .map(|&(s, _)| u64::from(clen[usize::from(s)]) + u64::from(extra_bits(s)))
            .sum::<u64>();
    let dynamic = 3 + header + data_bits(tokens, &llen, &dlen);
    let (flit, fdist) = fixed_lengths();
    let fixed = 3 + data_bits(tokens, &flit, &fdist);
    // Stored: the header bits, up to 7 to align, then per 65535 bytes 4
    // bytes of lengths (and 3 bits of header for each further block).
    let pieces = raw.len().div_ceil(65535).max(1) as u64;
    let stored = 3 + 7 + pieces * 32 + (pieces - 1) * (3 + 7) + raw.len() as u64 * 8;
    if stored < dynamic.min(fixed) {
        let pieces: Vec<&[u8]> = if raw.is_empty() {
            vec![raw]
        } else {
            raw.chunks(65535).collect()
        };
        for (i, p) in pieces.iter().enumerate() {
            w.put(u32::from(last && i + 1 == pieces.len()), 1);
            w.put(0, 2);
            w.align();
            let len = p.len() as u16;
            w.put(u32::from(len), 16);
            w.put(u32::from(!len), 16);
            w.out.extend_from_slice(p);
        }
        return;
    }
    w.put(u32::from(last), 1);
    let (lit, dist) = if fixed <= dynamic {
        w.put(1, 2);
        (flit, fdist)
    } else {
        w.put(2, 2);
        w.put((nlit - 257) as u32, 5);
        w.put((ndist - 1) as u32, 5);
        w.put((ncl - 4) as u32, 4);
        for &s in &CL_ORDER[..ncl] {
            w.put(u32::from(clen[s]), 3);
        }
        let ccode = canonical(&clen);
        for &(s, x) in &rle {
            w.code(ccode[usize::from(s)], clen[usize::from(s)]);
            let e = extra_bits(s);
            if e > 0 {
                w.put(u32::from(x), e);
            }
        }
        (llen, dlen)
    };
    let lcode = canonical(&lit);
    let dcode = canonical(&dist);
    for t in tokens {
        match *t {
            Token::Literal(b) => w.code(lcode[usize::from(b)], lit[usize::from(b)]),
            Token::Match { len, dist: d } => {
                let lc = len_code(len);
                w.code(lcode[257 + lc], lit[257 + lc]);
                if LEN_EXTRA[lc] > 0 {
                    w.put(u32::from(len - LEN_BASE[lc]), u32::from(LEN_EXTRA[lc]));
                }
                let dc = dist_code(d);
                w.code(dcode[dc], dist[dc]);
                if DIST_EXTRA[dc] > 0 {
                    w.put(u32::from(d - DIST_BASE[dc]), u32::from(DIST_EXTRA[dc]));
                }
            }
        }
    }
    w.code(lcode[256], lit[256]);
}

#[cfg(test)]
mod tests;
