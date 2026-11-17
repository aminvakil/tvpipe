//! Thin wrappers over `ring` with the shapes the protocols need: fixed-size
//! arrays in and out, [`Error`] instead of `ring::error::Unspecified`, and
//! the HAP nonce conventions.
//!
//! - Ed25519 long-term keys ([`Ed25519KeyPair`], [`ed25519_verify`]).
//! - X25519 ephemeral agreement ([`X25519Ephemeral`]). `ring` only offers
//!   ephemeral X25519 keys, which is all pair-verify needs.
//! - ChaCha20-Poly1305 ([`ChaCha20Poly1305`]) with [`nonce_counter`] and
//!   [`nonce_label`] for HAP's 8-byte nonces left-padded to 12 bytes.
//! - HKDF ([`hkdf_sha512`], [`hkdf_sha256`]), SHA-1/256/512, HMAC, CSPRNG,
//!   and a constant-time comparison ([`ct_eq`]).

use ::ring::{aead, agreement, digest, hkdf, hmac, rand, signature};

use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Randomness

/// Fills `buf` from the operating system's CSPRNG.
pub fn random_fill(buf: &mut [u8]) -> Result<()> {
    use ::ring::rand::SecureRandom;
    rand::SystemRandom::new()
        .fill(buf)
        .map_err(|_| Error::crypto("rng: system random source failed"))
}

/// `N` random bytes from the operating system's CSPRNG.
pub fn random_array<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0u8; N];
    random_fill(&mut out)?;
    Ok(out)
}

/// Constant-time equality of two byte strings (length is not secret).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

// ---------------------------------------------------------------------------
// Ed25519

pub const ED25519_SEED_LEN: usize = 32;
pub const ED25519_PUBLIC_KEY_LEN: usize = 32;
pub const ED25519_SIGNATURE_LEN: usize = 64;

/// An Ed25519 key pair that remembers its 32-byte seed so it can be stored.
pub struct Ed25519KeyPair {
    seed: [u8; ED25519_SEED_LEN],
    inner: signature::Ed25519KeyPair,
}

impl std::fmt::Debug for Ed25519KeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ed25519KeyPair")
            .field("public_key", &crate::util::hex::encode(&self.public_key()))
            .finish_non_exhaustive()
    }
}

impl Ed25519KeyPair {
    /// A fresh key pair from the CSPRNG.
    pub fn generate() -> Result<Ed25519KeyPair> {
        Ed25519KeyPair::from_seed(&random_array()?)
    }

    /// The key pair for a 32-byte seed (RFC 8032 "private key").
    pub fn from_seed(seed: &[u8; ED25519_SEED_LEN]) -> Result<Ed25519KeyPair> {
        let inner = signature::Ed25519KeyPair::from_seed_unchecked(seed)
            .map_err(|_| Error::crypto("ed25519: invalid seed"))?;
        Ok(Ed25519KeyPair { seed: *seed, inner })
    }

    /// Like [`from_seed`](Self::from_seed), from a slice (e.g. a stored
    /// credential); errors unless it is exactly 32 bytes.
    pub fn from_seed_slice(seed: &[u8]) -> Result<Ed25519KeyPair> {
        let seed: &[u8; ED25519_SEED_LEN] = seed
            .try_into()
            .map_err(|_| Error::crypto("ed25519: seed must be 32 bytes"))?;
        Ed25519KeyPair::from_seed(seed)
    }

    pub fn seed(&self) -> &[u8; ED25519_SEED_LEN] {
        &self.seed
    }

    pub fn public_key(&self) -> [u8; ED25519_PUBLIC_KEY_LEN] {
        use ::ring::signature::KeyPair;
        let mut pk = [0u8; ED25519_PUBLIC_KEY_LEN];
        pk.copy_from_slice(self.inner.public_key().as_ref());
        pk
    }

    pub fn sign(&self, msg: &[u8]) -> [u8; ED25519_SIGNATURE_LEN] {
        let mut sig = [0u8; ED25519_SIGNATURE_LEN];
        sig.copy_from_slice(self.inner.sign(msg).as_ref());
        sig
    }
}

/// Verifies an Ed25519 signature. All inputs may come from the network;
/// wrong lengths are errors, not panics.
pub fn ed25519_verify(public_key: &[u8], msg: &[u8], sig: &[u8]) -> Result<()> {
    if public_key.len() != ED25519_PUBLIC_KEY_LEN {
        return Err(Error::crypto("ed25519: public key must be 32 bytes"));
    }
    if sig.len() != ED25519_SIGNATURE_LEN {
        return Err(Error::crypto("ed25519: signature must be 64 bytes"));
    }
    signature::UnparsedPublicKey::new(&signature::ED25519, public_key)
        .verify(msg, sig)
        .map_err(|_| Error::crypto("ed25519: bad signature"))
}

// ---------------------------------------------------------------------------
// X25519

pub const X25519_KEY_LEN: usize = 32;

/// An ephemeral X25519 private key; consumed by [`agree`](Self::agree).
pub struct X25519Ephemeral {
    key: agreement::EphemeralPrivateKey,
    public: [u8; X25519_KEY_LEN],
}

impl std::fmt::Debug for X25519Ephemeral {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X25519Ephemeral")
            .field("public_key", &crate::util::hex::encode(&self.public))
            .finish_non_exhaustive()
    }
}

impl X25519Ephemeral {
    pub fn generate() -> Result<X25519Ephemeral> {
        let rng = rand::SystemRandom::new();
        let key = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng)
            .map_err(|_| Error::crypto("x25519: key generation failed"))?;
        let pk = key
            .compute_public_key()
            .map_err(|_| Error::crypto("x25519: public key computation failed"))?;
        let mut public = [0u8; X25519_KEY_LEN];
        public.copy_from_slice(pk.as_ref());
        Ok(X25519Ephemeral { key, public })
    }

    pub fn public_key(&self) -> [u8; X25519_KEY_LEN] {
        self.public
    }

    /// The shared secret with `peer_public` (32 bytes from the network).
    pub fn agree(self, peer_public: &[u8]) -> Result<[u8; X25519_KEY_LEN]> {
        if peer_public.len() != X25519_KEY_LEN {
            return Err(Error::crypto("x25519: peer public key must be 32 bytes"));
        }
        let peer = agreement::UnparsedPublicKey::new(&agreement::X25519, peer_public);
        agreement::agree_ephemeral(self.key, &peer, |secret| {
            let mut out = [0u8; X25519_KEY_LEN];
            out.copy_from_slice(secret);
            out
        })
        .map_err(|_| Error::crypto("x25519: key agreement failed"))
    }
}

// ---------------------------------------------------------------------------
// ChaCha20-Poly1305

pub const CHACHA_KEY_LEN: usize = 32;
pub const CHACHA_NONCE_LEN: usize = 12;
pub const CHACHA_TAG_LEN: usize = 16;

/// HAP counter nonce: 4 zero bytes, then the counter as 8 little-endian
/// bytes (the 8-byte nonce left-padded with zeros to 12 bytes).
pub fn nonce_counter(counter: u64) -> [u8; CHACHA_NONCE_LEN] {
    let mut n = [0u8; CHACHA_NONCE_LEN];
    n[4..].copy_from_slice(&counter.to_le_bytes());
    n
}

/// HAP label nonce such as `b"PS-Msg05"`: the label right-aligned in 12
/// bytes, zero-padded on the left. Errors if longer than 12 bytes.
pub fn nonce_label(label: &[u8]) -> Result<[u8; CHACHA_NONCE_LEN]> {
    if label.len() > CHACHA_NONCE_LEN {
        return Err(Error::crypto("chacha20-poly1305: nonce label too long"));
    }
    let mut n = [0u8; CHACHA_NONCE_LEN];
    n[CHACHA_NONCE_LEN - label.len()..].copy_from_slice(label);
    Ok(n)
}

/// A ChaCha20-Poly1305 key (RFC 8439). The caller guarantees nonce
/// uniqueness (HAP uses counters or fixed per-message labels).
pub struct ChaCha20Poly1305 {
    key: aead::LessSafeKey,
}

impl std::fmt::Debug for ChaCha20Poly1305 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChaCha20Poly1305 { .. }")
    }
}

impl ChaCha20Poly1305 {
    pub fn new(key: &[u8; CHACHA_KEY_LEN]) -> ChaCha20Poly1305 {
        ChaCha20Poly1305::from_slice(key).expect("32-byte key is always valid")
    }

    /// Errors unless `key` is exactly 32 bytes.
    pub fn from_slice(key: &[u8]) -> Result<ChaCha20Poly1305> {
        let unbound = aead::UnboundKey::new(&aead::CHACHA20_POLY1305, key)
            .map_err(|_| Error::crypto("chacha20-poly1305: key must be 32 bytes"))?;
        Ok(ChaCha20Poly1305 {
            key: aead::LessSafeKey::new(unbound),
        })
    }

    /// Encrypts `buf` in place and appends the 16-byte tag.
    pub fn seal_in_place(
        &self,
        nonce: &[u8; CHACHA_NONCE_LEN],
        aad: &[u8],
        buf: &mut Vec<u8>,
    ) -> Result<()> {
        self.key
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(*nonce),
                aead::Aad::from(aad),
                buf,
            )
            .map_err(|_| Error::crypto("chacha20-poly1305: seal failed"))
    }

    /// Returns `ciphertext || tag`.
    pub fn seal(&self, nonce: &[u8; CHACHA_NONCE_LEN], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(plaintext.len() + CHACHA_TAG_LEN);
        buf.extend_from_slice(plaintext);
        // Sealing only fails for inputs beyond ~256 GiB.
        self.seal_in_place(nonce, aad, &mut buf)
            .expect("chacha20-poly1305: message too long");
        buf
    }

    /// Decrypts `ciphertext || tag` in place; returns the plaintext prefix
    /// of `buf`. Errors on a short input or a bad tag.
    pub fn open_in_place<'a>(
        &self,
        nonce: &[u8; CHACHA_NONCE_LEN],
        aad: &[u8],
        buf: &'a mut [u8],
    ) -> Result<&'a mut [u8]> {
        if buf.len() < CHACHA_TAG_LEN {
            return Err(Error::crypto("chacha20-poly1305: input shorter than tag"));
        }
        self.key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(*nonce),
                aead::Aad::from(aad),
                buf,
            )
            .map(|pt| &mut pt[..])
            .map_err(|_| Error::crypto("chacha20-poly1305: authentication failed"))
    }

    /// Decrypts `ciphertext || tag` into a new vector.
    pub fn open(
        &self,
        nonce: &[u8; CHACHA_NONCE_LEN],
        aad: &[u8],
        ciphertext_and_tag: &[u8],
    ) -> Result<Vec<u8>> {
        let mut buf = ciphertext_and_tag.to_vec();
        let n = self.open_in_place(nonce, aad, &mut buf)?.len();
        buf.truncate(n);
        Ok(buf)
    }
}

// ---------------------------------------------------------------------------
// HKDF

struct OkmLen(usize);

impl hkdf::KeyType for OkmLen {
    fn len(&self) -> usize {
        self.0
    }
}

fn hkdf_fill(
    alg: hkdf::Algorithm,
    salt: &[u8],
    ikm: &[u8],
    info: &[u8],
    out: &mut [u8],
) -> Result<()> {
    let info = [info];
    hkdf::Salt::new(alg, salt)
        .extract(ikm)
        .expand(&info, OkmLen(out.len()))
        .and_then(|okm| okm.fill(out))
        .map_err(|_| Error::crypto("hkdf: output too long"))
}

/// HKDF-SHA512 (RFC 5869): fills `out` (at most 255 * 64 bytes).
pub fn hkdf_sha512(salt: &[u8], ikm: &[u8], info: &[u8], out: &mut [u8]) -> Result<()> {
    hkdf_fill(hkdf::HKDF_SHA512, salt, ikm, info, out)
}

/// HKDF-SHA512 into a fixed-size array, e.g. a 32-byte key.
pub fn hkdf_sha512_array<const N: usize>(salt: &[u8], ikm: &[u8], info: &[u8]) -> Result<[u8; N]> {
    let mut out = [0u8; N];
    hkdf_sha512(salt, ikm, info, &mut out)?;
    Ok(out)
}

/// HKDF-SHA256 (RFC 5869): fills `out` (at most 255 * 32 bytes).
pub fn hkdf_sha256(salt: &[u8], ikm: &[u8], info: &[u8], out: &mut [u8]) -> Result<()> {
    hkdf_fill(hkdf::HKDF_SHA256, salt, ikm, info, out)
}

// ---------------------------------------------------------------------------
// SHA and HMAC

fn to_array<const N: usize>(d: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    out.copy_from_slice(d);
    out
}

macro_rules! hash {
    ($(#[$m:meta])* $ty:ident, $func:ident, $alg:expr, $len:expr) => {
        $(#[$m])*
        #[derive(Clone)]
        pub struct $ty(digest::Context);

        impl $ty {
            pub const LEN: usize = $len;

            pub fn new() -> $ty {
                $ty(digest::Context::new(&$alg))
            }

            pub fn update(&mut self, data: &[u8]) -> &mut Self {
                self.0.update(data);
                self
            }

            pub fn finish(self) -> [u8; $len] {
                to_array(self.0.finish().as_ref())
            }
        }

        impl Default for $ty {
            fn default() -> $ty {
                $ty::new()
            }
        }

        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($ty), " { .. }"))
            }
        }

        /// One-shot digest.
        pub fn $func(data: &[u8]) -> [u8; $len] {
            to_array(digest::digest(&$alg, data).as_ref())
        }
    };
}

hash!(
    /// Incremental SHA-1 (legacy protocols only).
    Sha1,
    sha1,
    digest::SHA1_FOR_LEGACY_USE_ONLY,
    20
);
hash!(
    /// Incremental SHA-256.
    Sha256,
    sha256,
    digest::SHA256,
    32
);
hash!(
    /// Incremental SHA-512.
    Sha512,
    sha512,
    digest::SHA512,
    64
);

fn hmac_sign<const N: usize>(alg: hmac::Algorithm, key: &[u8], msg: &[u8]) -> [u8; N] {
    to_array(hmac::sign(&hmac::Key::new(alg, key), msg).as_ref())
}

pub fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    hmac_sign(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, key, msg)
}

pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    hmac_sign(hmac::HMAC_SHA256, key, msg)
}

pub fn hmac_sha512(key: &[u8], msg: &[u8]) -> [u8; 64] {
    hmac_sign(hmac::HMAC_SHA512, key, msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::vectors;

    #[test]
    fn sha_vectors() {
        let blocks = vectors::load("crypto/sha.txt");
        assert!(blocks.len() >= 12);
        for b in blocks {
            let msg = b.hex("msg");
            let md = b.hex("md");
            let (oneshot, incremental): (Vec<u8>, Vec<u8>) = match b.str("alg") {
                "sha1" => {
                    let mut h = Sha1::new();
                    msg.chunks(7).for_each(|c| {
                        h.update(c);
                    });
                    (sha1(&msg).to_vec(), h.finish().to_vec())
                }
                "sha256" => {
                    let mut h = Sha256::new();
                    msg.chunks(7).for_each(|c| {
                        h.update(c);
                    });
                    (sha256(&msg).to_vec(), h.finish().to_vec())
                }
                "sha512" => {
                    let mut h = Sha512::new();
                    msg.chunks(7).for_each(|c| {
                        h.update(c);
                    });
                    (sha512(&msg).to_vec(), h.finish().to_vec())
                }
                a => panic!("unknown alg {a}"),
            };
            assert_eq!(oneshot, md, "{}", b.name());
            assert_eq!(incremental, md, "{} incremental", b.name());
        }
    }

    #[test]
    fn hmac_vectors() {
        let blocks = vectors::load("crypto/hmac.txt");
        assert!(blocks.len() >= 12);
        for b in blocks {
            let (key, msg) = (b.hex("key"), b.hex("msg"));
            let got = match b.str("alg") {
                "sha1" => hmac_sha1(&key, &msg).to_vec(),
                "sha256" => hmac_sha256(&key, &msg).to_vec(),
                "sha512" => hmac_sha512(&key, &msg).to_vec(),
                a => panic!("unknown alg {a}"),
            };
            assert_eq!(got, b.hex("mac"), "{}", b.name());
        }
    }

    #[test]
    fn hkdf_vectors() {
        let blocks = vectors::load("crypto/hkdf.txt");
        assert!(blocks.len() >= 8);
        for b in blocks {
            let mut out = vec![0u8; b.parse("len")];
            let (salt, ikm, info) = (b.hex("salt"), b.hex("ikm"), b.hex("info"));
            match b.str("alg") {
                "sha256" => hkdf_sha256(&salt, &ikm, &info, &mut out).unwrap(),
                "sha512" => hkdf_sha512(&salt, &ikm, &info, &mut out).unwrap(),
                a => panic!("unknown alg {a}"),
            }
            assert_eq!(out, b.hex("okm"), "{}", b.name());
        }
        let a: [u8; 32] = hkdf_sha512_array(b"s", b"k", b"i").unwrap();
        let mut v = [0u8; 32];
        hkdf_sha512(b"s", b"k", b"i", &mut v).unwrap();
        assert_eq!(a, v);
        let mut too_long = vec![0u8; 255 * 64 + 1];
        assert!(hkdf_sha512(b"", b"", b"", &mut too_long).is_err());
    }

    #[test]
    fn ed25519_vectors() {
        for b in vectors::load("crypto/ed25519.txt") {
            let seed: [u8; 32] = b.hex("seed").try_into().unwrap();
            let kp = Ed25519KeyPair::from_seed(&seed).unwrap();
            assert_eq!(kp.public_key().to_vec(), b.hex("pk"), "{}", b.name());
            assert_eq!(kp.seed(), &seed);
            let msg = b.hex("msg");
            let sig = kp.sign(&msg);
            assert_eq!(sig.to_vec(), b.hex("sig"), "{}", b.name());
            ed25519_verify(&kp.public_key(), &msg, &sig).unwrap();
            let mut bad = sig;
            bad[0] ^= 1;
            assert!(ed25519_verify(&kp.public_key(), &msg, &bad).is_err());
            assert!(ed25519_verify(&kp.public_key(), b"other", &sig).is_err());
            assert!(ed25519_verify(&kp.public_key()[..31], &msg, &sig).is_err());
            assert!(ed25519_verify(&kp.public_key(), &msg, &sig[..63]).is_err());
        }
        let kp = Ed25519KeyPair::generate().unwrap();
        let again = Ed25519KeyPair::from_seed_slice(kp.seed()).unwrap();
        assert_eq!(kp.public_key(), again.public_key());
        assert!(Ed25519KeyPair::from_seed_slice(&[0; 31]).is_err());
    }

    #[test]
    fn x25519_agreement() {
        let a = X25519Ephemeral::generate().unwrap();
        let b = X25519Ephemeral::generate().unwrap();
        let (pa, pb) = (a.public_key(), b.public_key());
        assert_ne!(pa, pb);
        let sa = a.agree(&pb).unwrap();
        let sb = b.agree(&pa).unwrap();
        assert_eq!(sa, sb);
        let c = X25519Ephemeral::generate().unwrap();
        assert!(c.agree(&[1; 31]).is_err());
    }

    #[test]
    fn chacha_vectors() {
        for b in vectors::load("crypto/chacha20poly1305.txt") {
            let key = ChaCha20Poly1305::from_slice(&b.hex("key")).unwrap();
            let nonce: [u8; 12] = b.hex("nonce").try_into().unwrap();
            let (aad, pt) = (b.hex("aad"), b.hex("pt"));
            let mut expected = b.hex("ct");
            expected.extend_from_slice(&b.hex("tag"));
            let sealed = key.seal(&nonce, &aad, &pt);
            assert_eq!(sealed, expected, "{}", b.name());
            assert_eq!(key.open(&nonce, &aad, &sealed).unwrap(), pt);

            let mut buf = sealed.clone();
            assert_eq!(key.open_in_place(&nonce, &aad, &mut buf).unwrap(), &pt[..]);

            let mut tampered = sealed.clone();
            tampered[3] ^= 0x80;
            assert!(key.open(&nonce, &aad, &tampered).is_err());
            assert!(key.open(&nonce, b"other aad", &sealed).is_err());
            assert!(key.open(&nonce, &aad, &sealed[..15]).is_err());
        }
        assert!(ChaCha20Poly1305::from_slice(&[0; 16]).is_err());
    }

    #[test]
    fn hap_nonces() {
        assert_eq!(
            nonce_counter(1),
            [0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
            "counter is little-endian after 4 zero bytes"
        );
        assert_eq!(
            nonce_counter(0x0102030405060708),
            [0, 0, 0, 0, 8, 7, 6, 5, 4, 3, 2, 1]
        );
        assert_eq!(
            &nonce_label(b"PS-Msg05").unwrap(),
            b"\0\0\0\0PS-Msg05",
            "label is left-padded"
        );
        assert_eq!(&nonce_label(b"").unwrap(), &[0u8; 12]);
        assert!(nonce_label(b"0123456789abc").is_err());
    }

    #[test]
    fn misc() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
        let a: [u8; 32] = random_array().unwrap();
        let b: [u8; 32] = random_array().unwrap();
        assert_ne!(a, b);
    }
}
