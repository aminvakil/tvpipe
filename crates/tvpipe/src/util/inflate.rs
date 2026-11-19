//! DEFLATE decompression (RFC 1951) for zip bundles (`playshare`).
//!
//! A canonical-Huffman decoder that reads one bit at a time: simple
//! rather than fast, which is plenty for game bundles of a few
//! megabytes. Every malformed stream is an error, never a panic; the
//! output is bounded by the caller (zip bombs).
//!
//! Sources: RFC 1951 (the format, the fixed code, the length and
//! distance tables).

use crate::error::{Error, Result};

const MAX_BITS: usize = 15;
/// Literal/length symbols (286 used, 288 in the fixed code).
const MAX_LITLEN: usize = 288;
const MAX_DIST: usize = 30;

/// Base lengths for symbols 257..=285 and their extra bits.
const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// Base distances for symbols 0..=29 and their extra bits.
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// The order code length code lengths arrive in (dynamic blocks).
const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn bad(msg: &str) -> Error {
    Error::malformed(format!("deflate: {msg}"))
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    /// Bits not used yet, LSB first.
    buf: u32,
    count: u32,
}

impl<'a> Bits<'a> {
    fn bits(&mut self, need: u32) -> Result<u32> {
        while self.count < need {
            let Some(&b) = self.data.get(self.pos) else {
                return Err(bad("unexpected end of data"));
            };
            self.pos += 1;
            self.buf |= u32::from(b) << self.count;
            self.count += 8;
        }
        let v = if need == 0 {
            0
        } else {
            self.buf & ((1u32 << need) - 1)
        };
        self.buf = self.buf.checked_shr(need).unwrap_or(0);
        self.count -= need;
        Ok(v)
    }

    /// Drops the bits up to the next byte boundary (stored blocks).
    fn align(&mut self) {
        self.buf = 0;
        self.count = 0;
    }
}

/// A canonical Huffman code: how many codes of each length, and the
/// symbols ordered by code.
struct Huffman {
    count: [u16; MAX_BITS + 1],
    symbol: Vec<u16>,
}

impl Huffman {
    /// From code lengths per symbol (0: unused). An over-subscribed set
    /// is an error; an incomplete one is allowed only for a single code
    /// (RFC 1951 §3.2.7 allows one distance code), which `decode` turns
    /// into an error if an unused code shows up.
    fn new(lengths: &[u8]) -> Result<Huffman> {
        let mut count = [0u16; MAX_BITS + 1];
        for &l in lengths {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let mut left: i32 = 1;
        for &c in &count[1..] {
            left <<= 1;
            left -= i32::from(c);
            if left < 0 {
                return Err(bad("over-subscribed Huffman code"));
            }
        }
        let mut offs = [0u16; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offs[len + 1] = offs[len] + count[len];
        }
        let mut symbol = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbol[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        Ok(Huffman { count, symbol })
    }

    fn decode(&self, bits: &mut Bits<'_>) -> Result<u16> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..=MAX_BITS {
            code |= bits.bits(1)? as i32;
            let count = i32::from(self.count[len]);
            if code - first < count {
                return self
                    .symbol
                    .get((index + code - first) as usize)
                    .copied()
                    .ok_or_else(|| bad("bad Huffman code"));
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(bad("bad Huffman code"))
    }
}

/// Decompresses a raw DEFLATE stream; more than `max_out` bytes of output
/// is an error.
pub fn inflate(data: &[u8], max_out: usize) -> Result<Vec<u8>> {
    let mut bits = Bits {
        data,
        pos: 0,
        buf: 0,
        count: 0,
    };
    let mut out: Vec<u8> = Vec::new();
    let mut fixed: Option<(Huffman, Huffman)> = None;
    loop {
        let last = bits.bits(1)? == 1;
        match bits.bits(2)? {
            0 => stored(&mut bits, &mut out, max_out)?,
            1 => {
                if fixed.is_none() {
                    fixed = Some(fixed_codes()?);
                }
                let (lit, dist) = fixed.as_ref().ok_or_else(|| bad("fixed codes"))?;
                codes(&mut bits, &mut out, lit, dist, max_out)?;
            }
            2 => {
                let (lit, dist) = dynamic_codes(&mut bits)?;
                codes(&mut bits, &mut out, &lit, &dist, max_out)?;
            }
            _ => return Err(bad("reserved block type")),
        }
        if last {
            return Ok(out);
        }
    }
}

fn stored(bits: &mut Bits<'_>, out: &mut Vec<u8>, max_out: usize) -> Result<()> {
    bits.align();
    let p = bits.pos;
    let head = bits
        .data
        .get(p..p + 4)
        .ok_or_else(|| bad("unexpected end of data"))?;
    let len = usize::from(u16::from_le_bytes([head[0], head[1]]));
    let nlen = u16::from_le_bytes([head[2], head[3]]);
    if nlen != !(len as u16) {
        return Err(bad("stored block length check failed"));
    }
    let body = bits
        .data
        .get(p + 4..p + 4 + len)
        .ok_or_else(|| bad("unexpected end of data"))?;
    if out.len() + len > max_out {
        return Err(bad("output too large"));
    }
    out.extend_from_slice(body);
    bits.pos = p + 4 + len;
    Ok(())
}

fn fixed_codes() -> Result<(Huffman, Huffman)> {
    let mut lengths = [0u8; MAX_LITLEN];
    for (i, l) in lengths.iter_mut().enumerate() {
        *l = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    Ok((Huffman::new(&lengths)?, Huffman::new(&[5u8; MAX_DIST])?))
}

fn dynamic_codes(bits: &mut Bits<'_>) -> Result<(Huffman, Huffman)> {
    let nlen = bits.bits(5)? as usize + 257;
    let ndist = bits.bits(5)? as usize + 1;
    let ncode = bits.bits(4)? as usize + 4;
    if nlen > 286 || ndist > MAX_DIST {
        return Err(bad("too many length or distance codes"));
    }
    let mut cl = [0u8; 19];
    for &i in &CL_ORDER[..ncode] {
        cl[i] = bits.bits(3)? as u8;
    }
    let clcode = Huffman::new(&cl)?;
    let mut lengths = vec![0u8; nlen + ndist];
    let mut i = 0;
    while i < nlen + ndist {
        let sym = clcode.decode(bits)?;
        let (value, repeat) = match sym {
            0..=15 => (sym as u8, 1),
            16 => {
                if i == 0 {
                    return Err(bad("repeat with no first length"));
                }
                (lengths[i - 1], 3 + bits.bits(2)? as usize)
            }
            17 => (0, 3 + bits.bits(3)? as usize),
            18 => (0, 11 + bits.bits(7)? as usize),
            _ => return Err(bad("bad code length code")),
        };
        if i + repeat > nlen + ndist {
            return Err(bad("too many code lengths"));
        }
        lengths[i..i + repeat].fill(value);
        i += repeat;
    }
    if lengths[256] == 0 {
        return Err(bad("no end-of-block code"));
    }
    Ok((
        Huffman::new(&lengths[..nlen])?,
        Huffman::new(&lengths[nlen..])?,
    ))
}

fn codes(
    bits: &mut Bits<'_>,
    out: &mut Vec<u8>,
    lit: &Huffman,
    dist: &Huffman,
    max_out: usize,
) -> Result<()> {
    loop {
        let sym = lit.decode(bits)?;
        match sym {
            0..=255 => {
                if out.len() >= max_out {
                    return Err(bad("output too large"));
                }
                out.push(sym as u8);
            }
            256 => return Ok(()),
            257..=285 => {
                let k = usize::from(sym - 257);
                let len = usize::from(LEN_BASE[k]) + bits.bits(u32::from(LEN_EXTRA[k]))? as usize;
                let d = usize::from(dist.decode(bits)?);
                if d >= MAX_DIST {
                    return Err(bad("bad distance code"));
                }
                let back =
                    usize::from(DIST_BASE[d]) + bits.bits(u32::from(DIST_EXTRA[d]))? as usize;
                if back > out.len() {
                    return Err(bad("distance too far back"));
                }
                if out.len() + len > max_out {
                    return Err(bad("output too large"));
                }
                let start = out.len() - back;
                for j in 0..len {
                    let b = out[start + j];
                    out.push(b);
                }
            }
            _ => return Err(bad("bad literal/length code")),
        }
    }
}

#[cfg(test)]
mod tests;
