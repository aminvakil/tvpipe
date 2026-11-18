//! Mirroring stream keys (spec `docs/spec/airplay-mirroring.md` §5).
//!
//! Two crypto profiles exist (§5.0). The **HAP profile** (every
//! connection made by [`pair::connect`](crate::airplay::pair::connect))
//! keys video with ChaCha20-Poly1305 from the pair-verify shared secret
//! `S`, audio with a random key sent in SETUP, and the event channel with
//! its own HKDF keys. The **legacy profile** (raw pair-verify, plaintext
//! control) keys video with AES-128-CTR and audio with AES-128-CBC, both
//! from the FairPlay stream master key `K`.
//!
//! The FairPlay parts (`K`, the legacy CTR key/IV) come from
//! [`airplay::fairplay`](crate::airplay::fairplay), used only through its
//! public API.
//!
//! Sources: docs/spec/airplay-mirroring.md §2.2, §5, Appendix A; vectors
//! tests/vectors/airplay-mirror/keys.txt, channels.txt.

use crate::airplay::fairplay;
use crate::airplay::secure::Keys;
use crate::crypto::ring::{hkdf_sha512_array, Sha512, CHACHA_KEY_LEN};
use crate::error::Result;

pub const DATASTREAM_SALT_PREFIX: &str = "DataStream-Salt";
pub const DATASTREAM_OUTPUT_INFO: &[u8] = b"DataStream-Output-Encryption-Key";
pub const DATASTREAM_INPUT_INFO: &[u8] = b"DataStream-Input-Encryption-Key";
pub const EVENTS_SALT: &[u8] = b"Events-Salt";
/// Receiver → sender (what we read).
pub const EVENTS_RECEIVER_TO_SENDER_INFO: &[u8] = b"Events-Write-Encryption-Key";
/// Sender → receiver (what we write).
pub const EVENTS_SENDER_TO_RECEIVER_INFO: &[u8] = b"Events-Read-Encryption-Key";

/// `K = SHA-512(K_fp ‖ S)[0..16]` (or `K_fp` for an empty secret, §2.2).
pub fn stream_master_key(fairplay_key: &[u8; 16], pair_secret: &[u8]) -> [u8; 16] {
    fairplay::stream_master_key(fairplay_key, pair_secret)
}

/// Legacy profile: the video AES-128-CTR key and initial counter block
/// for a video `streamConnectionID` (§5.2).
pub fn legacy_video_key_iv(master: &[u8; 16], stream_connection_id: u64) -> ([u8; 16], [u8; 16]) {
    fairplay::stream_key_iv(master, stream_connection_id)
}

fn datastream_salt(n: u64) -> Vec<u8> {
    format!("{DATASTREAM_SALT_PREFIX}{n}").into_bytes()
}

/// HAP profile: the ChaCha20-Poly1305 video key (§5.1).
pub fn video_key(pair_secret: &[u8], stream_connection_id: u64) -> Result<[u8; CHACHA_KEY_LEN]> {
    hkdf_sha512_array(
        &datastream_salt(stream_connection_id),
        pair_secret,
        DATASTREAM_OUTPUT_INFO,
    )
}

/// HAP profile: the event channel keys, from the sender's side (§5.1).
pub fn event_keys(pair_secret: &[u8]) -> Result<Keys> {
    Keys::derive(
        pair_secret,
        EVENTS_SALT,
        EVENTS_SENDER_TO_RECEIVER_INFO,
        EVENTS_RECEIVER_TO_SENDER_INFO,
    )
}

/// HAP profile: the MediaDataControl keys for a 64-bit seed (§5.1).
pub fn media_data_control_keys(pair_secret: &[u8], seed: u64) -> Result<Keys> {
    Keys::derive(
        pair_secret,
        &datastream_salt(seed),
        DATASTREAM_OUTPUT_INFO,
        DATASTREAM_INPUT_INFO,
    )
}

/// Legacy raw pair-verify: the AES-128-CTR key and IV that protect the
/// signatures (Appendix A).
pub fn legacy_pair_verify_key_iv(secret: &[u8]) -> ([u8; 16], [u8; 16]) {
    let half = |label: &[u8]| {
        let mut h = Sha512::new();
        h.update(label).update(secret);
        let d = h.finish();
        let mut out = [0u8; 16];
        out.copy_from_slice(&d[..16]);
        out
    };
    (half(b"Pair-Verify-AES-Key"), half(b"Pair-Verify-AES-IV"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::vectors;

    fn arr16(v: &[u8]) -> [u8; 16] {
        v.try_into().unwrap()
    }

    #[test]
    fn key_vectors() {
        let blocks = vectors::load("airplay-mirror/keys.txt");
        let mut seen = 0;
        for b in &blocks {
            let name = b.name();
            if name.starts_with("stream master key") {
                let k =
                    stream_master_key(&arr16(&b.hex("fairplay_key")), &b.hex("pair_verify_secret"));
                assert_eq!(k.to_vec(), b.hex("master_key"), "{name}");
            } else if name.starts_with("video AES-CTR") {
                let (k, iv) =
                    legacy_video_key_iv(&arr16(&b.hex("shk")), b.parse("stream_connection_id"));
                assert_eq!(k.to_vec(), b.hex("key"), "{name}");
                assert_eq!(iv.to_vec(), b.hex("iv"), "{name}");
            } else if name.starts_with("video ChaCha20") {
                let id: u64 = b.parse("stream_connection_id");
                assert_eq!(datastream_salt(id), b.str("salt").as_bytes());
                let k = video_key(&b.hex("pair_verify_secret"), id).unwrap();
                assert_eq!(k.to_vec(), b.hex("key"), "{name}");
            } else if name == "event channel keys" {
                let k = event_keys(&b.hex("pair_verify_secret")).unwrap();
                assert_eq!(k.read.to_vec(), b.hex("sender_read_key"));
                assert_eq!(k.write.to_vec(), b.hex("sender_write_key"));
            } else if name.starts_with("MediaDataControl") {
                let seed: u64 = b.parse("seed_decimal");
                assert_eq!(seed as i64, b.parse::<i64>("seed_as_bplist_int"));
                let k = media_data_control_keys(&b.hex("pair_verify_secret"), seed).unwrap();
                assert_eq!(k.read.to_vec(), b.hex("sender_read_key"));
                assert_eq!(k.write.to_vec(), b.hex("sender_write_key"));
            } else {
                panic!("unknown block {name}");
            }
            seen += 1;
        }
        assert_eq!(seen, 7);
    }

    #[test]
    fn legacy_pair_verify_vector() {
        let b = vectors::load("airplay-mirror/channels.txt")
            .into_iter()
            .find(|b| b.name().starts_with("legacy raw pair-verify"))
            .unwrap();
        let (k, iv) = legacy_pair_verify_key_iv(&b.hex("pair_verify_secret"));
        assert_eq!(k.to_vec(), b.hex("key"));
        assert_eq!(iv.to_vec(), b.hex("iv"));
    }
}
