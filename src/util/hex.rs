//! Hex encoding and decoding.

use crate::error::{Error, Result};

const DIGITS: &[u8; 16] = b"0123456789abcdef";

pub fn encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for &b in data {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 15) as usize] as char);
    }
    s
}

/// Decodes hex, ignoring ASCII whitespace and `:` separators.
pub fn decode(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut hi: Option<u8> = None;
    for c in s.bytes() {
        if c.is_ascii_whitespace() || c == b':' {
            continue;
        }
        let v = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => {
                return Err(Error::malformed(format!(
                    "hex: invalid digit {:?}",
                    c as char
                )))
            }
        };
        match hi.take() {
            None => hi = Some(v),
            Some(h) => out.push((h << 4) | v),
        }
    }
    if hi.is_some() {
        return Err(Error::malformed("hex: odd number of digits"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        assert_eq!(encode(&[0, 0xab, 0xff]), "00abff");
        assert_eq!(decode("00 AB:ff").unwrap(), [0, 0xab, 0xff]);
        assert!(decode("abc").is_err());
        assert!(decode("zz").is_err());
    }
}
