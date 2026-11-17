// SPDX-License-Identifier: LGPL-3.0-or-later
// Ported from doubletake internal/airplay/{fpsap.go,fairplay_crypto.go,fairplay.go,mirror.go}
// @ be27776beb05beaa149a7096e4722f5cba60eb37 (LGPL-3.0-or-later).
// Copyright of the original: the doubletake authors. See LICENSES/.
//! FairPlay SAP for AirPlay mirroring (PLAN §7.3).
//!
//! **License:** this directory is an LGPL-3.0-or-later port of doubletake
//! (decision 0005). Nothing outside `src/airplay/fairplay/` may copy from
//! it; use it only through the API below.
//!
//! # Use
//!
//! On the verified, encrypted control connection ([`pair::connect`]):
//!
//! 1. [`fp_setup`] (or by hand: create an [`FpSession`], `POST /fp-setup`
//!    [`FpSession::m1`], feed the response body to [`FpSession::m3`],
//!    `POST /fp-setup` the result, check the response with
//!    [`FpSession::check_m4`]; both POSTs carry `X-Apple-ET: 32` and
//!    `Content-Type: application/octet-stream`).
//! 2. Pick a random 16-byte FairPlay key and wrap it with
//!    [`FpSession::wrap_key`]; the 72 bytes are the `ekey` the receiver
//!    unwraps to learn the key.
//! 3. [`stream_master_key`] mixes that FairPlay key with the pair-verify
//!    shared secret ([`pair::Control::shared`]) into the key the receiver
//!    decrypts the stream with; [`stream_key_iv`] derives the per-stream
//!    AES key and IV from it and a stream's `streamConnectionID`.
//!
//! Everything from the network is validated; malformed m2/m4 become
//! errors, never panics.
//!
//! [`pair::connect`]: crate::airplay::pair::connect
//! [`pair::Control::shared`]: crate::airplay::pair::Control

mod md5;
mod message;
mod sap;
mod tables;
mod whitebox;

use std::io::{Read, Write};

use crate::crypto::aes::Aes;
use crate::crypto::ring::{ct_eq, hmac_sha1, random_fill, Sha512};
use crate::error::{Error, Result};
use crate::net::rtsp::RtspClient;

/// The `X-Apple-ET` header value both `/fp-setup` requests carry.
pub const ENCRYPTION_TYPE: &str = "32";
/// The `Content-Type` of both `/fp-setup` requests.
pub const CONTENT_TYPE: &str = "application/octet-stream";

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
            return Err(Error::malformed(format!(
                "fairplay: m2 payload marker {} (want 2)",
                m2[12]
            )));
        }
        let mode = m2[13] as usize;
        if mode >= message::MODES {
            return Err(Error::unsupported(format!(
                "fairplay: receiver picked unknown mode {mode}"
            )));
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
            return Err(Error::crypto("fairplay: m4 does not confirm m3"));
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
            .ok_or_else(|| Error::protocol("fairplay: m3 has not been generated"))
    }
}

/// Runs the whole `/fp-setup` exchange on `client` (the encrypted control
/// connection) and returns the session, ready for
/// [`FpSession::wrap_key`].
pub fn fp_setup<S: Read + Write>(client: &mut RtspClient<S>) -> Result<FpSession> {
    let mut session = FpSession::new()?;
    let m2 = post(client, &session.m1(), "m1")?;
    let m3 = session.m3(&m2)?;
    let m4 = post(client, &m3, "m3")?;
    session.check_m4(&m4)?;
    Ok(session)
}

fn post<S: Read + Write>(client: &mut RtspClient<S>, body: &[u8], what: &str) -> Result<Vec<u8>> {
    let resp = client.request(
        "POST",
        "/fp-setup",
        &[("X-Apple-ET", ENCRYPTION_TYPE)],
        Some((CONTENT_TYPE, body.to_vec())),
    )?;
    if resp.status == 404 {
        return Err(Error::unsupported(
            "fairplay: receiver has no /fp-setup (404)",
        ));
    }
    let resp = resp.error_for_status(&format!("fairplay: /fp-setup ({what})"))?;
    Ok(resp.body)
}

/// The key the receiver decrypts the stream with, from the FairPlay `key`
/// given to [`FpSession::wrap_key`] and the pair-verify shared secret
/// (`pair::Control::shared.secret()`). Receivers paired through
/// [`pair::connect`](crate::airplay::pair::connect) always expect the
/// secret mixed in; an empty `pair_secret` returns `key` unchanged (only
/// for receivers without HAP pairing).
pub fn stream_master_key(key: &[u8; 16], pair_secret: &[u8]) -> [u8; 16] {
    if pair_secret.is_empty() {
        return *key;
    }
    // SHA-512(key || secret), truncated.
    let mut h = Sha512::new();
    h.update(key).update(pair_secret);
    first16(&h.finish())
}

/// The per-stream AES-128 key and IV, from the [`stream_master_key`] and
/// the stream's `streamConnectionID` exactly as sent in that stream's
/// SETUP (a 64-bit integer; pass its unsigned value).
pub fn stream_key_iv(master: &[u8; 16], stream_connection_id: u64) -> ([u8; 16], [u8; 16]) {
    // SHA-512(label || decimal id || master), truncated, per label.
    let derive = |label: &str| {
        let mut h = Sha512::new();
        h.update(format!("{label}{stream_connection_id}").as_bytes())
            .update(master);
        first16(&h.finish())
    };
    (derive("AirPlayStreamKey"), derive("AirPlayStreamIV"))
}

fn first16(d: &[u8; 64]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&d[..16]);
    out
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
        return Err(Error::malformed(format!(
            "fairplay: {what} is {} bytes (want {want})",
            record.len()
        )));
    }
    if record[..12] != header(kind, payload_len) {
        return Err(Error::malformed(format!(
            "fairplay: {what} has a bad header {}",
            crate::util::hex::encode(&record[..12])
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
