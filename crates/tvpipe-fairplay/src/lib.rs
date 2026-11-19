// SPDX-License-Identifier: LGPL-3.0-or-later
// Ported from doubletake internal/airplay/{fpsap.go,fairplay_crypto.go,fairplay.go,mirror.go}
// @ be27776beb05beaa149a7096e4722f5cba60eb37 (LGPL-3.0-or-later).
// Copyright of the original: the doubletake authors. See LICENSES/.
//! FairPlay SAP for AirPlay mirroring, for the `tvpipe` crate's optional
//! `fairplay` feature.
//!
//! **License:** LGPL-3.0-or-later, a port of doubletake (see the crate's
//! README). Linking it into a program brings the LGPL's obligations
//! (relinkability); `tvpipe` without its `fairplay` feature does not use
//! it. Nothing outside this crate may copy from it.
//!
//! The crate is bytes in, bytes out: the HTTP requests, the keys derived
//! from the wrapped key and everything else stay in `tvpipe`.
//!
//! # Use
//!
//! On the verified, encrypted AirPlay control connection:
//!
//! 1. Create an [`FpSession`], `POST /fp-setup` [`FpSession::m1`], feed
//!    the response body to [`FpSession::m3`], `POST /fp-setup` the result
//!    and check the response with [`FpSession::check_m4`].
//! 2. Pick a random 16-byte FairPlay key and wrap it with
//!    [`FpSession::wrap_key`]; the 72 bytes are the `ekey` the receiver
//!    unwraps to learn the key.
//!
//! Everything from the network is validated; malformed m2/m4 become
//! errors, never panics.

#![forbid(unsafe_code)]

mod aes;
mod md5;
mod message;
mod sap;
mod tables;
mod whitebox;

use crate::aes::Aes;

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// An m2/m4 that does not parse.
    Malformed,
    /// A mode or key size this implementation does not know.
    Unsupported,
    /// m4 does not confirm m3.
    Crypto,
    /// Called out of order (m4 or a key wrap before m3).
    Protocol,
    /// The OS random number generator failed.
    Random,
}

/// A FairPlay error: its kind and a message (`fairplay: …`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    msg: String,
}

impl Error {
    fn new(kind: ErrorKind, msg: impl Into<String>) -> Error {
        Error {
            kind,
            msg: msg.into(),
        }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn random_fill(buf: &mut [u8]) -> Result<()> {
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new()
        .fill(buf)
        .map_err(|_| Error::new(ErrorKind::Random, "fairplay: no randomness from the OS"))
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, key);
    let mut out = [0u8; 20];
    out.copy_from_slice(ring::hmac::sign(&k, msg).as_ref());
    out
}

/// Constant-time equality of two byte strings of the same length.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
fn hex_decode(s: &str) -> Vec<u8> {
    test_util::hex::decode(s).unwrap()
}

/// Helpers for the tests: hex, SHA-256, the `key = value` vector files.
#[cfg(test)]
pub(crate) mod test_util {
    pub mod hex {
        pub fn encode(b: &[u8]) -> String {
            super::super::hex_encode(b)
        }

        pub fn decode(s: &str) -> Result<Vec<u8>, String> {
            let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
            if s.len() % 2 != 0 {
                return Err(format!("odd hex length {}", s.len()));
            }
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
                .collect()
        }
    }

    pub struct Sha256(ring::digest::Context);

    impl Sha256 {
        pub fn new() -> Sha256 {
            Sha256(ring::digest::Context::new(&ring::digest::SHA256))
        }

        pub fn update(&mut self, d: &[u8]) -> &mut Self {
            self.0.update(d);
            self
        }

        pub fn finish(self) -> [u8; 32] {
            let mut out = [0u8; 32];
            out.copy_from_slice(self.0.finish().as_ref());
            out
        }
    }

    /// Blank-line separated blocks of `key = value` lines (`#` comments,
    /// indented lines continue the previous value), as in tvpipe.
    pub mod vectors {
        pub struct Block {
            pub line: usize,
            pub entries: Vec<(String, String)>,
        }

        impl Block {
            pub fn get(&self, key: &str) -> Option<&str> {
                self.entries
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.as_str())
            }

            pub fn str(&self, key: &str) -> &str {
                self.get(key)
                    .unwrap_or_else(|| panic!("block at line {}: no {key:?}", self.line))
            }

            pub fn hex(&self, key: &str) -> Vec<u8> {
                super::hex::decode(self.str(key)).unwrap()
            }

            pub fn parse<T: std::str::FromStr>(&self, key: &str) -> T {
                self.str(key)
                    .parse()
                    .unwrap_or_else(|_| panic!("block at line {}: bad {key:?}", self.line))
            }

            pub fn name(&self) -> String {
                self.get("name")
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("line {}", self.line))
            }
        }

        /// `tests/vectors/<name>` of this crate.
        pub fn load(name: &str) -> Vec<Block> {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("vectors")
                .join(name);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let mut blocks = Vec::new();
            let mut cur: Option<Block> = None;
            for (i, raw) in text.lines().enumerate() {
                if raw.trim().is_empty() {
                    blocks.extend(cur.take());
                    continue;
                }
                if raw.trim_start().starts_with('#') {
                    continue;
                }
                let b = cur.get_or_insert_with(|| Block {
                    line: i + 1,
                    entries: Vec::new(),
                });
                if raw.starts_with([' ', '\t']) {
                    if let Some((_, v)) = b.entries.last_mut() {
                        v.push_str(raw.trim());
                    }
                    continue;
                }
                let (k, v) = raw
                    .split_once('=')
                    .unwrap_or_else(|| panic!("{name}:{}: no '='", i + 1));
                b.entries.push((k.trim().to_owned(), v.trim().to_owned()));
            }
            blocks.extend(cur);
            blocks
        }
    }
}

/// m1 capability byte: every mode is acceptable; the receiver picks.
const M1_PAYLOAD: [u8; 4] = [0x02, 0x00, 0x03, 0xbb];
const M3_LABEL: [u8; 3] = [0x8f, 0x1a, 0x9c];
const EKEY_HEADER: [u8; 16] = [
    b'F', b'P', b'L', b'Y', 0x01, 0x02, 0x01, 0x00, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x00, 0x00, 0x00,
];

/// One FairPlay SAP exchange (m1 → m2 → m3 → m4) and the key wrapping
/// that depends on it. Use one session per control connection.
pub struct FpSession {
    /// The sender's SAP value: `00 01` and 126 random bytes.
    local_sap: [u8; 128],
    exchanged: Option<Exchanged>,
}

struct Exchanged {
    remote_sap: [u8; 128],
    m3: [u8; 164],
}

impl std::fmt::Debug for FpSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FpSession")
            .field("m3_sent", &self.exchanged.is_some())
            .finish_non_exhaustive()
    }
}

impl FpSession {
    /// A new session with fresh randomness from the OS.
    pub fn new() -> Result<FpSession> {
        let mut entropy = [0u8; 126];
        random_fill(&mut entropy)?;
        Ok(FpSession::with_entropy(&entropy))
    }

    /// A session from fixed randomness (tests and vectors only).
    pub fn with_entropy(entropy: &[u8; 126]) -> FpSession {
        let mut local_sap = [0u8; 128];
        local_sap[1] = 1;
        local_sap[2..].copy_from_slice(entropy);
        FpSession {
            local_sap,
            exchanged: None,
        }
    }

    /// The first request body (16 bytes).
    pub fn m1(&self) -> [u8; 16] {
        let mut m1 = [0u8; 16];
        m1[..12].copy_from_slice(&header(1, 4));
        m1[12..].copy_from_slice(&M1_PAYLOAD);
        m1
    }

    /// Processes the receiver's reply to m1 (`m2`, 142 bytes) and returns
    /// the second request body (164 bytes). Fails on a malformed m2 or a
    /// mode this implementation does not know.
    pub fn m3(&mut self, m2: &[u8]) -> Result<[u8; 164]> {
        check_header(m2, 2, 130, "m2")?;
        if m2[12] != 2 {
            return Err(Error::new(
                ErrorKind::Malformed,
                format!("fairplay: m2 payload marker {} (want 2)", m2[12]),
            ));
        }
        let mode = m2[13] as usize;
        if mode >= message::MODES {
            return Err(Error::new(
                ErrorKind::Unsupported,
                format!("fairplay: receiver picked unknown mode {mode}"),
            ));
        }
        let mut body = [0u8; 128];
        body.copy_from_slice(&m2[14..142]);
        let remote_sap = message::decrypt(mode, &body);

        let mut m3 = [0u8; 164];
        m3[..12].copy_from_slice(&header(3, 152));
        m3[12] = mode as u8;
        m3[13..16].copy_from_slice(&M3_LABEL);
        m3[16..144].copy_from_slice(&message::encrypt(mode, &self.local_sap));
        m3[144..].copy_from_slice(&whitebox::m3_tail(&self.local_sap, &remote_sap));
        self.exchanged = Some(Exchanged { remote_sap, m3 });
        Ok(m3)
    }

    /// Checks the receiver's reply to m3 (`m4`, 32 bytes): it must confirm
    /// the m3 just sent. Fails before [`m3`](Self::m3) succeeded.
    pub fn check_m4(&self, m4: &[u8]) -> Result<()> {
        let ex = self.exchanged()?;
        check_header(m4, 4, 20, "m4")?;
        if !ct_eq(&m4[12..], &ex.m3[144..]) {
            return Err(Error::new(
                ErrorKind::Crypto,
                "fairplay: m4 does not confirm m3",
            ));
        }
        Ok(())
    }

    /// Wraps a 16-byte FairPlay `key` into the 72-byte `ekey` for this
    /// session's receiver (fresh random mask from the OS). Fails before
    /// [`m3`](Self::m3) succeeded.
    pub fn wrap_key(&self, key: &[u8; 16]) -> Result<[u8; 72]> {
        let mut mask = [0u8; 16];
        random_fill(&mut mask)?;
        self.wrap_key_with_mask(key, &mask)
    }

    /// [`wrap_key`](Self::wrap_key) with a fixed mask (tests and vectors).
    pub fn wrap_key_with_mask(&self, key: &[u8; 16], mask: &[u8; 16]) -> Result<[u8; 72]> {
        let ex = self.exchanged()?;
        let mut ekey = [0u8; 72];
        ekey[..16].copy_from_slice(&EKEY_HEADER);
        ekey[16..32].copy_from_slice(mask);
        ekey[32..36].copy_from_slice(&16u32.to_be_bytes());

        let mut block = [0u8; 16];
        for (i, b) in block.iter_mut().enumerate() {
            *b = key[i] ^ mask[i];
        }
        Aes::new(&whitebox::wrapping_key(&self.local_sap, &ex.remote_sap))?
            .encrypt_block(&mut block);
        ekey[56..].copy_from_slice(&block);

        let mut mac_input = [0u8; 52];
        mac_input[..36].copy_from_slice(&ekey[..36]);
        mac_input[36..].copy_from_slice(key);
        let mac_key = whitebox::descriptor(&self.local_sap, &ex.remote_sap);
        ekey[36..56].copy_from_slice(&hmac_sha1(&mac_key, &mac_input));
        Ok(ekey)
    }

    fn exchanged(&self) -> Result<&Exchanged> {
        self.exchanged
            .as_ref()
            .ok_or_else(|| Error::new(ErrorKind::Protocol, "fairplay: m3 has not been generated"))
    }
}

/// The 12-byte FPLY record header.
fn header(kind: u8, payload_len: u32) -> [u8; 12] {
    let mut h = [0u8; 12];
    h[..4].copy_from_slice(b"FPLY");
    h[4..8].copy_from_slice(&[3, 1, kind, 0]);
    h[8..].copy_from_slice(&payload_len.to_be_bytes());
    h
}

fn check_header(record: &[u8], kind: u8, payload_len: u32, what: &str) -> Result<()> {
    let want = 12 + payload_len as usize;
    if record.len() != want {
        return Err(Error::new(
            ErrorKind::Malformed,
            format!("fairplay: {what} is {} bytes (want {want})", record.len()),
        ));
    }
    if record[..12] != header(kind, payload_len) {
        return Err(Error::new(
            ErrorKind::Malformed,
            format!(
                "fairplay: {what} has a bad header {}",
                hex_encode(&record[..12])
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
