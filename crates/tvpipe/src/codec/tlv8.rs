//! HAP TLV8: the type-length-value encoding used by AirPlay pairing
//! (`/pair-setup`, `/pair-verify`).
//!
//! Each item is `type: u8 | length: u8 | value: [u8; length]`. Values longer
//! than 255 bytes are split into consecutive items of the same type, every
//! fragment but the last exactly 255 bytes long; decoders merge them back.
//! Two distinct items of the same type in a row (as in lists) are separated by
//! a zero-length [`tag::SEPARATOR`] item.
//!
//! Sources: HomeKit Accessory Protocol Specification (non-commercial
//! version): "TLV8 Encoding" rules and examples, and the pairing tables for
//! TLV types, methods and error codes. Integers inside values are little
//! endian.

use crate::error::{Error, Result};
use crate::util::bytes::{Reader, Writer};

/// TLV types (HAP pairing "TLV Types" table).
pub mod tag {
    /// Method to use for pairing (see [`super::method`]).
    pub const METHOD: u8 = 0x00;
    /// Identifier for authentication (pairing identifier).
    pub const IDENTIFIER: u8 = 0x01;
    /// 16+ bytes of random salt (SRP).
    pub const SALT: u8 = 0x02;
    /// Curve25519, SRP public key, or signed Ed25519 key.
    pub const PUBLIC_KEY: u8 = 0x03;
    /// Ed25519 or SRP proof.
    pub const PROOF: u8 = 0x04;
    /// Encrypted data with auth tag at the end.
    pub const ENCRYPTED_DATA: u8 = 0x05;
    /// State of the pairing process (M1 = 1, ...).
    pub const STATE: u8 = 0x06;
    /// Error code (see [`super::errors`]). Only present on errors.
    pub const ERROR: u8 = 0x07;
    /// Seconds to delay until retrying a setup code.
    pub const RETRY_DELAY: u8 = 0x08;
    /// X.509 certificate.
    pub const CERTIFICATE: u8 = 0x09;
    /// Ed25519 signature.
    pub const SIGNATURE: u8 = 0x0a;
    /// Bit value describing permissions of the controller being added.
    pub const PERMISSIONS: u8 = 0x0b;
    /// Non-last fragment of data (fragmented pairing).
    pub const FRAGMENT_DATA: u8 = 0x0c;
    /// Last fragment of data (fragmented pairing).
    pub const FRAGMENT_LAST: u8 = 0x0d;
    /// Pairing type flags.
    pub const FLAGS: u8 = 0x13;
    /// Zero-length separator between items of the same type.
    pub const SEPARATOR: u8 = 0xff;
}

/// Values of [`tag::METHOD`] (HAP "Methods" table).
pub mod method {
    pub const PAIR_SETUP: u8 = 0;
    pub const PAIR_SETUP_WITH_AUTH: u8 = 1;
    pub const PAIR_VERIFY: u8 = 2;
    pub const ADD_PAIRING: u8 = 3;
    pub const REMOVE_PAIRING: u8 = 4;
    pub const LIST_PAIRINGS: u8 = 5;
}

/// Values of [`tag::ERROR`] (HAP "Error Codes" table).
pub mod errors {
    pub const UNKNOWN: u8 = 0x01;
    pub const AUTHENTICATION: u8 = 0x02;
    pub const BACKOFF: u8 = 0x03;
    pub const MAX_PEERS: u8 = 0x04;
    pub const MAX_TRIES: u8 = 0x05;
    pub const UNAVAILABLE: u8 = 0x06;
    pub const BUSY: u8 = 0x07;

    /// A human-readable name for an error code.
    pub fn name(code: u8) -> &'static str {
        match code {
            UNKNOWN => "unknown",
            AUTHENTICATION => "authentication failed",
            BACKOFF => "too many attempts, back off",
            MAX_PEERS => "no room for more pairings",
            MAX_TRIES => "too many authentication attempts",
            UNAVAILABLE => "pairing unavailable",
            BUSY => "busy with another pairing",
            _ => "unrecognized error",
        }
    }
}

/// Largest value one item can carry.
const MAX_FRAGMENT: usize = 255;

/// An ordered list of TLV8 items. Fragments are already merged: every item
/// holds its whole value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tlv8 {
    items: Vec<(u8, Vec<u8>)>,
}

impl Tlv8 {
    pub fn new() -> Tlv8 {
        Tlv8::default()
    }

    /// Appends an item (any length; it is fragmented on encode).
    pub fn push(&mut self, tag: u8, value: impl Into<Vec<u8>>) -> &mut Self {
        self.items.push((tag, value.into()));
        self
    }

    /// Appends a one-byte item (e.g. [`tag::STATE`]).
    pub fn push_u8(&mut self, tag: u8, value: u8) -> &mut Self {
        self.push(tag, vec![value])
    }

    /// Builder form of [`Tlv8::push`].
    pub fn with(mut self, tag: u8, value: impl Into<Vec<u8>>) -> Tlv8 {
        self.push(tag, value);
        self
    }

    /// Builder form of [`Tlv8::push_u8`].
    pub fn with_u8(mut self, tag: u8, value: u8) -> Tlv8 {
        self.push_u8(tag, value);
        self
    }

    pub fn items(&self) -> &[(u8, Vec<u8>)] {
        &self.items
    }

    pub fn iter(&self) -> impl Iterator<Item = (u8, &[u8])> {
        self.items.iter().map(|(t, v)| (*t, v.as_slice()))
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The first item of this type.
    pub fn get(&self, tag: u8) -> Option<&[u8]> {
        self.iter().find(|(t, _)| *t == tag).map(|(_, v)| v)
    }

    /// All items of this type, in order (for separator-delimited lists).
    pub fn get_all(&self, tag: u8) -> impl Iterator<Item = &[u8]> {
        self.iter().filter(move |(t, _)| *t == tag).map(|(_, v)| v)
    }

    /// The first item of this type as a single byte.
    pub fn get_u8(&self, tag: u8) -> Option<u8> {
        match self.get(tag) {
            Some([b]) => Some(*b),
            _ => None,
        }
    }

    /// Like [`Tlv8::get`] but a missing item is a protocol error.
    pub fn require(&self, tag: u8) -> Result<&[u8]> {
        self.get(tag)
            .ok_or_else(|| Error::protocol(format!("tlv8: missing item of type 0x{tag:02x}")))
    }

    /// Like [`Tlv8::get_u8`] but missing or wrong-length is a protocol error.
    pub fn require_u8(&self, tag: u8) -> Result<u8> {
        self.get_u8(tag).ok_or_else(|| {
            Error::protocol(format!("tlv8: missing one-byte item of type 0x{tag:02x}"))
        })
    }

    /// The pairing state ([`tag::STATE`]), if present.
    pub fn state(&self) -> Option<u8> {
        self.get_u8(tag::STATE)
    }

    /// Fails with an auth error if the peer sent a [`tag::ERROR`] item.
    pub fn check_error(&self) -> Result<()> {
        match self.get(tag::ERROR) {
            None => Ok(()),
            Some(v) => {
                let code = v.first().copied().unwrap_or(0);
                let mut msg = format!("tlv8: peer error {code}: {}", errors::name(code));
                if let Some(delay) = self.get(tag::RETRY_DELAY) {
                    let secs = delay
                        .iter()
                        .rev()
                        .fold(0u64, |acc, &b| (acc << 8) | b as u64);
                    msg.push_str(&format!(" (retry in {secs} s)"));
                }
                Err(Error::auth(msg))
            }
        }
    }

    /// Serializes, splitting values longer than 255 bytes into fragments.
    /// A zero-length value is emitted as one empty item.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(self.items.iter().map(|(_, v)| v.len() + 2).sum());
        for (tag, value) in &self.items {
            if value.is_empty() {
                w.u8(*tag).u8(0);
                continue;
            }
            for chunk in value.chunks(MAX_FRAGMENT) {
                w.u8(*tag).u8(chunk.len() as u8).bytes(chunk);
            }
        }
        w.into_vec()
    }

    /// Parses, merging consecutive same-type items where the earlier one is
    /// a full 255-byte fragment.
    pub fn decode(data: &[u8]) -> Result<Tlv8> {
        let mut r = Reader::new(data, "tlv8");
        let mut items: Vec<(u8, Vec<u8>)> = Vec::new();
        // Whether the last item ended with a full fragment and may continue.
        let mut open = false;
        while !r.is_at_end() {
            let tag = r.u8()?;
            let len = r.u8()? as usize;
            let value = r.bytes(len)?;
            match items.last_mut() {
                Some((last, buf)) if open && *last == tag => buf.extend_from_slice(value),
                _ => items.push((tag, value.to_vec())),
            }
            open = len == MAX_FRAGMENT;
        }
        Ok(Tlv8 { items })
    }
}

impl FromIterator<(u8, Vec<u8>)> for Tlv8 {
    fn from_iter<I: IntoIterator<Item = (u8, Vec<u8>)>>(iter: I) -> Tlv8 {
        Tlv8 {
            items: iter.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::fuzz::Rng;
    use crate::util::vectors;

    /// Vector blocks: `encoded` plus the decoded items as `t0`/`v0`,
    /// `t1`/`v1`, ... (tag hex, value hex). `decode_only` marks inputs our
    /// encoder would not produce byte for byte.
    fn items_of(b: &vectors::Block) -> Vec<(u8, Vec<u8>)> {
        let mut out = Vec::new();
        for i in 0.. {
            let Some(t) = b.get(&format!("t{i}")) else {
                break;
            };
            let t = u8::from_str_radix(t, 16).expect("tag hex");
            let v = b.get(&format!("v{i}")).unwrap_or("");
            let v = crate::util::hex::decode(v).expect("value hex");
            out.push((t, v));
        }
        out
    }

    #[test]
    fn vectors() {
        let blocks = vectors::load("codec/tlv8.txt");
        assert!(blocks.len() >= 5);
        for b in blocks {
            let name = b.name();
            let items = items_of(&b);
            let encoded = b.hex("encoded");
            let tlv: Tlv8 = items.clone().into_iter().collect();
            if b.get("decode_only").is_none() {
                assert_eq!(tlv.encode(), encoded, "{name}: encode");
            }
            let decoded = Tlv8::decode(&encoded).unwrap();
            assert_eq!(decoded.items(), &items[..], "{name}: decode");
        }
    }

    #[test]
    fn getters() {
        let t = Tlv8::new()
            .with_u8(tag::STATE, 2)
            .with(tag::PUBLIC_KEY, vec![1, 2, 3])
            .with(tag::SEPARATOR, Vec::new())
            .with(tag::PUBLIC_KEY, vec![4]);
        assert_eq!(t.state(), Some(2));
        assert_eq!(t.get(tag::PUBLIC_KEY), Some(&[1u8, 2, 3][..]));
        assert_eq!(t.get_all(tag::PUBLIC_KEY).count(), 2);
        assert!(t.require(tag::SALT).is_err());
        assert_eq!(t.get_u8(tag::PUBLIC_KEY), None);
        assert!(t.check_error().is_ok());
        let back = Tlv8::decode(&t.encode()).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn peer_error() {
        let t = Tlv8::new()
            .with_u8(tag::STATE, 4)
            .with_u8(tag::ERROR, errors::BACKOFF)
            .with(tag::RETRY_DELAY, vec![0x3c, 0x00]);
        let err = t.check_error().unwrap_err();
        assert_eq!(err.kind(), crate::error::ErrorKind::Auth);
        assert_eq!(
            err.to_string(),
            "tlv8: peer error 3: too many attempts, back off (retry in 60 s)"
        );
    }

    #[test]
    fn fragment_boundaries() {
        for len in [0usize, 1, 254, 255, 256, 510, 511, 765, 1000] {
            let value: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let t = Tlv8::new()
                .with(tag::ENCRYPTED_DATA, value.clone())
                .with_u8(tag::STATE, 5);
            let enc = t.encode();
            let frags = if len == 0 { 1 } else { len.div_ceil(255) };
            assert_eq!(enc.len(), len + 2 * frags + 3, "len {len}");
            assert_eq!(Tlv8::decode(&enc).unwrap(), t, "len {len}");
        }
    }

    #[test]
    fn truncated() {
        assert!(Tlv8::decode(&[0x06]).is_err());
        assert!(Tlv8::decode(&[0x06, 0x02, 0x01]).is_err());
        assert!(Tlv8::decode(&[]).unwrap().is_empty());
    }

    #[test]
    fn fuzz_smoke() {
        let seeds: Vec<Vec<u8>> = vectors::load("codec/tlv8.txt")
            .iter()
            .map(|b| b.hex("encoded"))
            .collect();
        let mut rng = Rng::new(0x7174_3800);
        for _ in 0..10_000 {
            let input = rng.mutate_any(&seeds);
            if let Ok(t) = Tlv8::decode(&input) {
                // Re-encoding a decoded value must decode to the same items.
                assert_eq!(Tlv8::decode(&t.encode()).unwrap(), t);
            }
        }
    }
}
