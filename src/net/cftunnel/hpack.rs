//! HPACK (RFC 7541) for the tunnel's HTTP/2: a complete decoder (static
//! and dynamic table, Huffman) and an encoder that writes only literals
//! without indexing and without Huffman (always valid, and our response
//! headers are few).
//!
//! The Huffman code (Appendix B) is canonical, so it is kept as the
//! symbols sorted by code length and the number of codes per length
//! (a test checks this against Appendix C.4/C.6).
//!
//! Sources: RFC 7541.

use crate::error::{Error, Result};

/// Appendix A.
const STATIC: [(&str, &str); 61] = [
    (":authority", ""),
    (":method", "GET"),
    (":method", "POST"),
    (":path", "/"),
    (":path", "/index.html"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "200"),
    (":status", "204"),
    (":status", "206"),
    (":status", "304"),
    (":status", "400"),
    (":status", "404"),
    (":status", "500"),
    ("accept-charset", ""),
    ("accept-encoding", "gzip, deflate"),
    ("accept-language", ""),
    ("accept-ranges", ""),
    ("accept", ""),
    ("access-control-allow-origin", ""),
    ("age", ""),
    ("allow", ""),
    ("authorization", ""),
    ("cache-control", ""),
    ("content-disposition", ""),
    ("content-encoding", ""),
    ("content-language", ""),
    ("content-length", ""),
    ("content-location", ""),
    ("content-range", ""),
    ("content-type", ""),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("expect", ""),
    ("expires", ""),
    ("from", ""),
    ("host", ""),
    ("if-match", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("if-range", ""),
    ("if-unmodified-since", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("max-forwards", ""),
    ("proxy-authenticate", ""),
    ("proxy-authorization", ""),
    ("range", ""),
    ("referer", ""),
    ("refresh", ""),
    ("retry-after", ""),
    ("server", ""),
    ("set-cookie", ""),
    ("strict-transport-security", ""),
    ("transfer-encoding", ""),
    ("user-agent", ""),
    ("vary", ""),
    ("via", ""),
    ("www-authenticate", ""),
];

/// Number of Huffman codes of each length from 5 to 30 bits.
const HUFFMAN_COUNTS: [u8; 26] = [
    10, 26, 32, 6, 0, 5, 3, 2, 6, 2, 3, 0, 0, 0, 3, 8, 13, 26, 29, 12, 4, 15, 19, 29, 0, 4,
];

/// The symbols in code order (shortest codes first, then by value); the
/// 257th code is EOS.
const HUFFMAN_SYMBOLS: &[u8; 256] = b"012aceiost %-./3456789=A_bdfghlmnpru:BCDEFGHIJKLMNOPQRSTUVWYjkqvwxyz&*,;XZ!\"()?'+|#>\x00$@[]~^}<`{\\\xc3\xd0\x80\x82\x83\xa2\xb8\xc2\xe0\xe2\x99\xa1\xa7\xac\xb0\xb1\xb3\xd1\xd8\xd9\xe3\xe5\xe6\x81\x84\x85\x86\x88\x92\x9a\x9c\xa0\xa3\xa4\xa9\xaa\xad\xb2\xb5\xb9\xba\xbb\xbd\xbe\xc4\xc6\xe4\xe8\xe9\x01\x87\x89\x8a\x8b\x8c\x8d\x8f\x93\x95\x96\x97\x98\x9b\x9d\x9e\xa5\xa6\xa8\xae\xaf\xb4\xb6\xb7\xbc\xbf\xc5\xe7\xef\x09\x8e\x90\x91\x94\x9f\xab\xce\xd7\xe1\xec\xed\xc7\xcf\xea\xeb\xc0\xc1\xc8\xc9\xca\xcd\xd2\xd5\xda\xdb\xee\xf0\xf2\xf3\xff\xcb\xcc\xd3\xd4\xd6\xdd\xde\xdf\xf1\xf4\xf5\xf6\xf7\xf8\xfa\xfb\xfc\xfd\xfe\x02\x03\x04\x05\x06\x07\x08\x0b\x0c\x0e\x0f\x10\x11\x12\x13\x14\x15\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f\x7f\xdc\xf9\x0a\x0d\x16";

/// Decodes a Huffman string (§5.2): canonical decoding, one bit at a
/// time; the padding must be a prefix of EOS (all ones, under 8 bits).
fn huffman(data: &[u8]) -> Result<Vec<u8>> {
    let bad = || Error::malformed("hpack: bad Huffman string");
    let mut out = Vec::with_capacity(data.len() * 8 / 5);
    let (mut code, mut first, mut index, mut len, mut ones) = (0u32, 0u32, 0usize, 0usize, true);
    for &byte in data {
        for i in (0..8).rev() {
            let bit = u32::from(byte >> i & 1);
            ones &= bit == 1;
            code |= bit;
            len += 1;
            let count = if len >= 5 {
                u32::from(HUFFMAN_COUNTS[len - 5])
            } else {
                0
            };
            if code.wrapping_sub(first) < count {
                let sym = HUFFMAN_SYMBOLS
                    .get(index + (code - first) as usize)
                    .ok_or_else(bad)?; // EOS
                out.push(*sym);
                (code, first, index, len, ones) = (0, 0, 0, 0, true);
            } else if len >= 30 {
                return Err(bad());
            } else {
                index += count as usize;
                first = (first + count) << 1;
                code <<= 1;
            }
        }
    }
    if len > 7 || !ones {
        return Err(bad());
    }
    Ok(out)
}

/// An integer with an `n`-bit prefix (§5.1); `b` advances past it.
fn int(b: &mut &[u8], n: u32) -> Result<usize> {
    let short = || Error::malformed("hpack: truncated");
    let (&first, rest) = b.split_first().ok_or_else(short)?;
    *b = rest;
    let max = (1usize << n) - 1;
    let mut v = usize::from(first) & max;
    if v < max {
        return Ok(v);
    }
    for shift in (0..28).step_by(7) {
        let (&c, rest) = b.split_first().ok_or_else(short)?;
        *b = rest;
        v += usize::from(c & 0x7f) << shift;
        if c & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(Error::malformed("hpack: integer too large"))
}

fn string(b: &mut &[u8]) -> Result<String> {
    let huff = b.first().is_some_and(|c| c & 0x80 != 0);
    let n = int(b, 7)?;
    if n > b.len() {
        return Err(Error::malformed("hpack: truncated string"));
    }
    let (raw, rest) = b.split_at(n);
    *b = rest;
    let bytes = if huff { huffman(raw)? } else { raw.to_vec() };
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub type Header = (String, String);

/// The decoding side of one connection (its dynamic table).
pub struct Decoder {
    /// Newest first.
    table: std::collections::VecDeque<Header>,
    size: usize,
    max: usize,
    /// What SETTINGS_HEADER_TABLE_SIZE allows (we keep the default).
    limit: usize,
}

/// Largest decoded header list we accept.
const MAX_LIST: usize = 64 * 1024;

impl Decoder {
    pub fn new() -> Decoder {
        Decoder {
            table: Default::default(),
            size: 0,
            max: 4096,
            limit: 4096,
        }
    }

    fn get(&self, i: usize) -> Result<Header> {
        let e = match i {
            1..=61 => STATIC.get(i - 1).map(|(n, v)| ((*n).into(), (*v).into())),
            _ => self.table.get(i.wrapping_sub(62)).cloned(),
        };
        e.ok_or_else(|| Error::malformed(format!("hpack: no table entry {i}")))
    }

    fn evict(&mut self) {
        while self.size > self.max {
            let Some((n, v)) = self.table.pop_back() else {
                break;
            };
            self.size -= n.len() + v.len() + 32;
        }
    }

    /// Decodes one header block.
    pub fn decode(&mut self, mut b: &[u8]) -> Result<Vec<Header>> {
        let mut out = Vec::new();
        let mut total = 0;
        while let Some(&c) = b.first() {
            let h = if c & 0x80 != 0 {
                let i = int(&mut b, 7)?;
                self.get(i)?
            } else if c & 0xe0 == 0x20 {
                let m = int(&mut b, 5)?;
                if m > self.limit {
                    return Err(Error::malformed("hpack: table size above the limit"));
                }
                self.max = m;
                self.evict();
                continue;
            } else {
                let indexed = c & 0x40 != 0;
                let i = int(&mut b, if indexed { 6 } else { 4 })?;
                let name = if i == 0 {
                    string(&mut b)?
                } else {
                    self.get(i)?.0
                };
                let h = (name, string(&mut b)?);
                if indexed {
                    self.size += h.0.len() + h.1.len() + 32;
                    self.table.push_front(h.clone());
                    self.evict();
                }
                h
            };
            total += h.0.len() + h.1.len() + 32;
            if total > MAX_LIST {
                return Err(Error::malformed("hpack: header list too large"));
            }
            out.push(h);
        }
        Ok(out)
    }
}

fn put_int(out: &mut Vec<u8>, first: u8, n: u32, mut v: usize) {
    let max = (1usize << n) - 1;
    if v < max {
        out.push(first | v as u8);
        return;
    }
    out.push(first | max as u8);
    v -= max;
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Appends `name: value` as a literal without indexing, new name (§6.2.2).
pub fn encode(out: &mut Vec<u8>, name: &str, value: &str) {
    out.push(0);
    for s in [name, value] {
        put_int(out, 0, 7, s.len());
        out.extend_from_slice(s.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::hex;

    fn h(list: &[(&str, &str)]) -> Vec<Header> {
        list.iter()
            .map(|(n, v)| ((*n).into(), (*v).into()))
            .collect()
    }

    fn table(d: &Decoder) -> Vec<Header> {
        d.table.iter().cloned().collect()
    }

    #[test]
    fn integers() {
        // C.1.
        let mut v = Vec::new();
        put_int(&mut v, 0, 5, 10);
        put_int(&mut v, 0, 5, 1337);
        put_int(&mut v, 0, 8, 42);
        assert_eq!(v, [10, 31, 154, 10, 42]);
        let mut b = &v[..];
        assert_eq!(int(&mut b, 5).unwrap(), 10);
        assert_eq!(int(&mut b, 5).unwrap(), 1337);
        assert_eq!(int(&mut b, 8).unwrap(), 42);
        assert!(int(&mut &[0x1f, 0x80][..], 5).is_err());
        assert!(int(&mut &[0x1f, 0xff, 0xff, 0xff, 0xff, 0x7f][..], 5).is_err());
    }

    /// C.3 (without Huffman) and C.4 (with): the same three requests on
    /// one connection, checking the dynamic table after each.
    #[test]
    fn requests() {
        for blocks in [
            [
                "828684410f7777772e6578616d706c652e636f6d",
                "828684be58086e6f2d6361636865",
                "828785bf400a637573746f6d2d6b65790c637573746f6d2d76616c7565",
            ],
            [
                "828684418cf1e3c2e5f23a6ba0ab90f4ff",
                "828684be5886a8eb10649cbf",
                "828785bf408825a849e95ba97d7f8925a849e95bb8e8b4bf",
            ],
        ] {
            let mut d = Decoder::new();
            let first = d.decode(&hex::decode(blocks[0]).unwrap()).unwrap();
            assert_eq!(
                first,
                h(&[
                    (":method", "GET"),
                    (":scheme", "http"),
                    (":path", "/"),
                    (":authority", "www.example.com")
                ])
            );
            assert_eq!(d.size, 57);
            let second = d.decode(&hex::decode(blocks[1]).unwrap()).unwrap();
            assert_eq!(second[4], ("cache-control".into(), "no-cache".into()));
            assert_eq!(second[3], (":authority".into(), "www.example.com".into()));
            assert_eq!(d.size, 110);
            let third = d.decode(&hex::decode(blocks[2]).unwrap()).unwrap();
            assert_eq!(
                third,
                h(&[
                    (":method", "GET"),
                    (":scheme", "https"),
                    (":path", "/index.html"),
                    (":authority", "www.example.com"),
                    ("custom-key", "custom-value")
                ])
            );
            assert_eq!(
                table(&d),
                h(&[
                    ("custom-key", "custom-value"),
                    ("cache-control", "no-cache"),
                    (":authority", "www.example.com")
                ])
            );
            assert_eq!(d.size, 164);
        }
    }

    /// C.6: responses with Huffman coding and a 256-byte table (evictions).
    #[test]
    fn responses_with_eviction() {
        let mut d = Decoder::new();
        d.max = 256;
        let blocks = [
            "488264025885aec3771a4b6196d07abe941054d444a8200595040b8166e082a62d1bff6e919d29ad171863c78f0b97c8e9ae82ae43d3",
            "4883640effc1c0bf",
            "88c16196d07abe941054d444a8200595040b8166e084a62d1bffc05a839bd9ab77ad94e7821dd7f2e6c7b335dfdfcd5b3960d5af27087f3672c1ab270fb5291f9587316065c003ed4ee5b1063d5007",
        ];
        d.decode(&hex::decode(blocks[0]).unwrap()).unwrap();
        assert_eq!(d.size, 222);
        let second = d.decode(&hex::decode(blocks[1]).unwrap()).unwrap();
        assert_eq!(second[0], (":status".into(), "307".into()));
        assert_eq!(d.size, 222);
        let third = d.decode(&hex::decode(blocks[2]).unwrap()).unwrap();
        assert_eq!(
            third,
            h(&[
                (":status", "200"),
                ("cache-control", "private"),
                ("date", "Mon, 21 Oct 2013 20:13:22 GMT"),
                ("location", "https://www.example.com"),
                ("content-encoding", "gzip"),
                (
                    "set-cookie",
                    "foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1"
                ),
            ])
        );
        assert_eq!(d.size, 215);
        assert_eq!(d.table.len(), 3);
    }

    #[test]
    fn literal_encoding_round_trips() {
        let mut b = Vec::new();
        encode(&mut b, ":status", "200");
        encode(&mut b, "x", &"v".repeat(300));
        let got = Decoder::new().decode(&b).unwrap();
        assert_eq!(got, h(&[(":status", "200"), ("x", &"v".repeat(300))]));
        assert_eq!(table(&Decoder::new()), vec![]);
    }

    #[test]
    fn rejects_garbage() {
        let mut d = Decoder::new();
        for bad in [
            &[0x80][..],                           // index 0
            &[0xff, 0x00],                         // index 127: past the tables
            &[0x3f, 0xe2, 0x1f],                   // table size above the limit
            &[0x00, 0x05, b'a'],                   // truncated name
            &[0x00, 0x81, 0x00],                   // Huffman padding of zeros
            &[0x00, 0x84, 0xff, 0xff, 0xff, 0xff], // EOS
        ] {
            assert!(d.decode(bad).is_err(), "{bad:02x?}");
        }
    }
}
