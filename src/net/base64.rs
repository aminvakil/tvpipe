//! Base64 (RFC 4648 §4, the standard alphabet with `=` padding).
//!
//! Used by the WebSocket handshake (`Sec-WebSocket-Key`/`-Accept`) and by
//! the DevTools protocol (screenshots, binary payloads). Decoding is
//! strict: padding is required, non-alphabet characters (including
//! whitespace) are rejected, and the unused bits of the last quantum must
//! be zero (RFC 4648 §3.5), so every input has one canonical encoding.
//!
//! Sources: RFC 4648.

use crate::error::{Error, Result};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes `data` with padding.
pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn value(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    } as u32)
}

/// Decodes padded base64.
pub fn decode(text: &str) -> Result<Vec<u8>> {
    let s = text.as_bytes();
    if s.len() % 4 != 0 {
        return Err(Error::malformed("base64: length is not a multiple of 4"));
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let quanta = s.len() / 4;
    for (q, chunk) in s.chunks_exact(4).enumerate() {
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && q + 1 != quanta) {
            return Err(Error::malformed("base64: misplaced padding"));
        }
        let mut n = 0u32;
        for &c in &chunk[..4 - pad] {
            let v = value(c).ok_or_else(|| Error::malformed("base64: invalid character"))?;
            n = n << 6 | v;
        }
        n <<= 6 * pad as u32;
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        let keep = 3 - pad;
        // Non-canonical: bits below the last full byte must be zero.
        if bytes[keep..].iter().any(|&b| b != 0) {
            return Err(Error::malformed("base64: non-zero padding bits"));
        }
        out.extend_from_slice(&bytes[..keep]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4648 §10.
    const VECTORS: &[(&str, &str)] = &[
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ];

    #[test]
    fn rfc4648_vectors() {
        for (plain, enc) in VECTORS {
            assert_eq!(encode(plain.as_bytes()), *enc);
            assert_eq!(decode(enc).unwrap(), plain.as_bytes());
        }
    }

    #[test]
    fn all_bytes_round_trip() {
        let data: Vec<u8> = (0..=255u8).chain((0..=255u8).rev()).collect();
        for n in 0..data.len() {
            let e = encode(&data[..n]);
            assert_eq!(decode(&e).unwrap(), &data[..n]);
        }
        // "+/" appear for the high values.
        assert_eq!(encode(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "Zg", "Zg=", "Z===", "Zg==Zg==", "Zm9v\n", "Zm 9", "Zh==", "Zm9=", "Zm-v", "====",
        ] {
            assert!(decode(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn websocket_key_shape() {
        // RFC 6455 §1.3: the sample nonce decodes to 16 bytes.
        assert_eq!(
            decode("dGhlIHNhbXBsZSBub25jZQ==").unwrap(),
            b"the sample nonce"
        );
    }
}
