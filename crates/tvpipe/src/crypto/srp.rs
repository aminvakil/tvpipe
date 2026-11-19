//! SRP-6a client (RFC 5054 / RFC 2945) with the HAP pair-setup parameters:
//! SHA-512, the RFC 5054 3072-bit group, g = 5.
//!
//! ```text
//! k  = H(N | PAD(g))
//! x  = H(s | H(I | ":" | P))
//! A  = g^a % N
//! u  = H(PAD(A) | PAD(B))
//! S  = (B - k * g^x) ^ (a + u * x) % N
//! K  = H(S)
//! M1 = H(H(N) xor H(g) | H(I) | s | A | B | K)
//! M2 = H(A | M1 | K)
//! ```
//!
//! Byte conventions (those of srptools, which pyatv uses against Apple
//! devices): integers are hashed as minimal big-endian bytes except inside
//! `PAD()`, which left-pads to 384 bytes; the salt is hashed as received.
//!
//! Minimal and padded encodings differ only when a value has a leading
//! zero byte. [`SrpClient::new`] draws `a` after it has seen `B`, and draws
//! again until neither `A` nor `S` has a leading zero byte, so the only
//! value whose encoding could matter is the receiver's `B` (1 in 256).
//!
//! Usage (HAP pair-setup M2 → M3 → M4):
//!
//! ```ignore
//! let client = SrpClient::new(b"Pair-Setup", pin.as_bytes(), &salt, &server_b)?;
//! send_m3(client.public_key(), client.proof());
//! let session_key = client.verify(&server_m2)?; // 64 bytes
//! ```

use super::bigint::Uint;
use super::ring::{ct_eq, random_array, Sha512};
use crate::error::{Error, Result};

/// RFC 5054 Appendix A, 3072-bit group prime.
pub const N_3072: [u8; 384] = hex384(concat!(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74",
    "020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F1437",
    "4FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED",
    "EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF05",
    "98DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB",
    "9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3B",
    "E39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF695581718",
    "3995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D04507A33",
    "A85521ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7",
    "ABF5AE8CDB0933D71E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864",
    "D87602733EC86A64521F2B18177B200CBBE117577A615D6C770988C0BAD946E2",
    "08E24FA074E5AB3143DB5BFCE0FD108E4B82D120A93AD2CAFFFFFFFFFFFFFFFF",
));

/// RFC 5054 3072-bit group generator.
pub const G: u8 = 5;

/// Byte length of N and of padded group elements.
pub const N_LEN: usize = 384;

/// Length of K, M1 and M2 (SHA-512).
pub const HASH_LEN: usize = 64;

/// HAP's fixed SRP username.
pub const HAP_USERNAME: &[u8] = b"Pair-Setup";

/// Bytes of randomness in the client private exponent `a`.
const PRIVATE_LEN: usize = 64;

const fn hex384(s: &str) -> [u8; 384] {
    const fn nib(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("bad hex digit"),
        }
    }
    let s = s.as_bytes();
    assert!(s.len() == 768);
    let mut out = [0u8; 384];
    let mut i = 0;
    while i < 384 {
        out[i] = (nib(s[2 * i]) << 4) | nib(s[2 * i + 1]);
        i += 1;
    }
    out
}

fn hash(parts: &[&[u8]]) -> [u8; HASH_LEN] {
    let mut h = Sha512::new();
    for p in parts {
        h.update(p);
    }
    h.finish()
}

fn uint(bytes: &[u8]) -> Uint {
    // Only called with hashes and fixed-size group elements, which always
    // fit; fall back to zero rather than panic.
    Uint::from_be_bytes(bytes).unwrap_or_default()
}

fn pad(v: &Uint) -> Result<Vec<u8>> {
    v.to_be_bytes_padded(N_LEN)
}

/// A client exchange after receiving the server's salt and public key `B`.
pub struct SrpClient {
    /// A, padded to 384 bytes (as sent in HAP's M3).
    public: Vec<u8>,
    m1: [u8; HASH_LEN],
    m2: [u8; HASH_LEN],
    key: [u8; HASH_LEN],
}

impl std::fmt::Debug for SrpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SrpClient").finish_non_exhaustive()
    }
}

/// Values shared by all attempts with the same inputs.
struct Setup<'a> {
    n: Uint,
    user: &'a [u8],
    salt: &'a [u8],
    b_min: Vec<u8>,
    b_pad: Vec<u8>,
    /// (B - k*v) mod N.
    base: Uint,
    x: Uint,
}

impl<'a> Setup<'a> {
    fn new(user: &'a [u8], password: &[u8], salt: &'a [u8], server_public: &[u8]) -> Result<Self> {
        let n = uint(&N_3072);
        let g = Uint::from_u64(G as u64);
        let b_pub = Uint::from_be_bytes(server_public)
            .map_err(|_| Error::crypto("srp: server public key too large"))?;
        if b_pub >= n {
            return Err(Error::crypto("srp: server public key out of range"));
        }
        if b_pub.is_zero() {
            // B % N == 0 would make S predictable (RFC 5054 §2.5.4).
            return Err(Error::crypto("srp: server public key is zero mod N"));
        }
        let k = uint(&hash(&[&N_3072, &pad(&g)?]));
        let x = uint(&hash(&[salt, &hash(&[user, b":", password])]));
        let v = g.modpow(&x, &n)?;
        let base = b_pub.mod_sub(&k.mod_mul(&v, &n)?, &n)?;
        Ok(Setup {
            b_min: b_pub.to_be_bytes(),
            b_pad: pad(&b_pub)?,
            n,
            user,
            salt,
            base,
            x,
        })
    }

    /// Runs the exchange for private exponent `a`. Returns the client
    /// state and whether A or S had a leading zero byte.
    fn attempt(&self, a: &Uint) -> Result<(SrpClient, bool)> {
        let n = &self.n;
        let a_pub = Uint::from_u64(G as u64).modpow(a, n)?;
        if a_pub.is_zero() {
            return Err(Error::crypto("srp: degenerate private key"));
        }
        let a_pad = pad(&a_pub)?;
        let a_min = a_pub.to_be_bytes();
        let u = uint(&hash(&[&a_pad, &self.b_pad]));
        if u.is_zero() {
            return Err(Error::crypto("srp: scrambling parameter is zero"));
        }
        // a + u*x: at most 512 + 512 + 1 bits, far below the 4096-bit width.
        let exp = u
            .checked_mul(&self.x)
            .and_then(|ux| ux.checked_add(a))
            .ok_or_else(|| Error::crypto("srp: exponent overflow"))?;
        let s = self.base.modpow(&exp, n)?;
        let s_min = s.to_be_bytes();
        let key = hash(&[&s_min]);

        let hn = hash(&[&N_3072]);
        let hg = hash(&[&[G]]);
        let mut hx = [0u8; HASH_LEN];
        for (o, (p, q)) in hx.iter_mut().zip(hn.iter().zip(&hg)) {
            *o = p ^ q;
        }
        let m1 = hash(&[
            &hx,
            &hash(&[self.user]),
            self.salt,
            &a_min,
            &self.b_min,
            &key,
        ]);
        let m2 = hash(&[&a_min, &m1, &key]);
        let ambiguous = a_min.len() < N_LEN || s_min.len() < N_LEN;
        Ok((
            SrpClient {
                public: a_pad,
                m1,
                m2,
                key,
            },
            ambiguous,
        ))
    }
}

impl SrpClient {
    /// Starts the client side with a random private exponent, given the
    /// username (HAP: [`HAP_USERNAME`]), the password (HAP: the PIN as
    /// ASCII), and the server's salt and public key `B` from the network.
    pub fn new(user: &[u8], password: &[u8], salt: &[u8], server_public: &[u8]) -> Result<Self> {
        let setup = Setup::new(user, password, salt, server_public)?;
        // Each draw has a ~2/256 chance of a leading-zero A or S; 64 draws
        // all failing is not going to happen with a working RNG.
        for _ in 0..64 {
            let a: [u8; PRIVATE_LEN] = random_array()?;
            let (client, ambiguous) = setup.attempt(&uint(&a))?;
            if !ambiguous {
                return Ok(client);
            }
        }
        Err(Error::crypto("srp: could not pick a private key"))
    }

    /// Like [`new`](Self::new) with a caller-chosen private exponent `a`
    /// (big-endian). For tests and transcripts; does not re-draw.
    pub fn with_private(
        user: &[u8],
        password: &[u8],
        salt: &[u8],
        server_public: &[u8],
        private: &[u8],
    ) -> Result<Self> {
        let a = Uint::from_be_bytes(private)
            .map_err(|_| Error::crypto("srp: private key too large"))?;
        let setup = Setup::new(user, password, salt, server_public)?;
        Ok(setup.attempt(&a)?.0)
    }

    /// The client public key `A`, padded to 384 bytes.
    pub fn public_key(&self) -> &[u8] {
        &self.public
    }

    /// The client proof `M1`.
    pub fn proof(&self) -> &[u8; HASH_LEN] {
        &self.m1
    }

    /// Checks the server proof `M2` (from the network) and returns the
    /// shared session key `K`.
    pub fn verify(self, server_proof: &[u8]) -> Result<[u8; HASH_LEN]> {
        if !ct_eq(server_proof, &self.m2) {
            return Err(Error::auth("srp: server proof mismatch (wrong PIN?)"));
        }
        Ok(self.key)
    }
}

/// The server side, for local test receivers and pairing unit tests (the
/// sender never needs it). Same conventions as [`SrpClient`].
pub struct SrpServer {
    user: Vec<u8>,
    salt: Vec<u8>,
    v: Uint,
    b: Uint,
    /// B, padded to 384 bytes.
    public: Vec<u8>,
}

impl std::fmt::Debug for SrpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SrpServer").finish_non_exhaustive()
    }
}

impl SrpServer {
    /// A server with a random 16-byte salt and private exponent.
    pub fn new(user: &[u8], password: &[u8]) -> Result<SrpServer> {
        let salt: [u8; 16] = random_array()?;
        let b: [u8; PRIVATE_LEN] = random_array()?;
        SrpServer::with_private(user, password, &salt, &b)
    }

    /// A server with a fixed salt and private exponent `b` (big-endian).
    pub fn with_private(
        user: &[u8],
        password: &[u8],
        salt: &[u8],
        private: &[u8],
    ) -> Result<SrpServer> {
        let n = uint(&N_3072);
        let g = Uint::from_u64(G as u64);
        let b = Uint::from_be_bytes(private)
            .map_err(|_| Error::crypto("srp: private key too large"))?;
        let k = uint(&hash(&[&N_3072, &pad(&g)?]));
        let x = uint(&hash(&[salt, &hash(&[user, b":", password])]));
        let v = g.modpow(&x, &n)?;
        let b_pub = k.mod_mul(&v, &n)?.mod_add(&g.modpow(&b, &n)?, &n)?;
        Ok(SrpServer {
            user: user.to_vec(),
            salt: salt.to_vec(),
            v,
            b,
            public: pad(&b_pub)?,
        })
    }

    pub fn salt(&self) -> &[u8] {
        &self.salt
    }

    /// `B`, padded to 384 bytes.
    pub fn public_key(&self) -> &[u8] {
        &self.public
    }

    /// Checks the client's `A` and `M1`; returns `(M2, K)`.
    pub fn verify(
        &self,
        client_public: &[u8],
        client_proof: &[u8],
    ) -> Result<([u8; HASH_LEN], [u8; HASH_LEN])> {
        let n = uint(&N_3072);
        let a_pub = Uint::from_be_bytes(client_public)
            .map_err(|_| Error::crypto("srp: client public key too large"))?;
        if a_pub >= n || a_pub.is_zero() {
            return Err(Error::crypto("srp: client public key out of range"));
        }
        let b_pub = uint(&self.public);
        let u = uint(&hash(&[&pad(&a_pub)?, &self.public]));
        let s = a_pub
            .mod_mul(&self.v.modpow(&u, &n)?, &n)?
            .modpow(&self.b, &n)?;
        let key = hash(&[&s.to_be_bytes()]);
        let hn = hash(&[&N_3072]);
        let hg = hash(&[&[G]]);
        let mut hx = [0u8; HASH_LEN];
        for (o, (p, q)) in hx.iter_mut().zip(hn.iter().zip(&hg)) {
            *o = p ^ q;
        }
        let a_min = a_pub.to_be_bytes();
        let m1 = hash(&[
            &hx,
            &hash(&[&self.user]),
            &self.salt,
            &a_min,
            &b_pub.to_be_bytes(),
            &key,
        ]);
        if !ct_eq(client_proof, &m1) {
            return Err(Error::auth("srp: client proof mismatch"));
        }
        Ok((hash(&[&a_min, &m1, &key]), key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::vectors;

    #[test]
    fn group_constants() {
        assert_eq!(N_3072[0], 0xff);
        assert_eq!(N_3072[383], 0xff);
        assert_eq!(N_3072[8], 0xc9);
        // N is the 3072-bit safe prime: 2^(N-1) == 1 mod N.
        let n = uint(&N_3072);
        let nm1 = n.checked_sub(&Uint::ONE).unwrap();
        assert_eq!(Uint::from_u64(2).modpow(&nm1, &n).unwrap(), Uint::ONE);
    }

    #[test]
    fn transcripts() {
        let blocks = vectors::load("crypto/srp.txt");
        assert!(blocks.len() >= 7);
        for b in blocks {
            let name = b.name();
            let client = SrpClient::with_private(
                b.str("user").as_bytes(),
                b.str("password").as_bytes(),
                &b.hex("salt"),
                &b.hex("B"),
                &b.hex("a"),
            )
            .unwrap();
            assert_eq!(client.public_key(), &b.hex("A")[..], "{name}: A");
            assert_eq!(client.proof().to_vec(), b.hex("M1"), "{name}: M1");
            let key = client.verify(&b.hex("M2")).unwrap();
            assert_eq!(key.to_vec(), b.hex("K"), "{name}: K");
        }
    }

    #[test]
    fn server_transcripts() {
        for b in vectors::load("crypto/srp.txt") {
            let name = b.name();
            let server = SrpServer::with_private(
                b.str("user").as_bytes(),
                b.str("password").as_bytes(),
                &b.hex("salt"),
                &b.hex("b"),
            )
            .unwrap();
            assert_eq!(server.public_key(), &b.hex("B")[..], "{name}: B");
            let (m2, key) = server.verify(&b.hex("A"), &b.hex("M1")).unwrap();
            assert_eq!(m2.to_vec(), b.hex("M2"), "{name}: M2");
            assert_eq!(key.to_vec(), b.hex("K"), "{name}: K");
        }
    }

    #[test]
    fn round_trip_and_wrong_pin() {
        let server = SrpServer::new(HAP_USERNAME, b"1234").unwrap();
        let client =
            SrpClient::new(HAP_USERNAME, b"1234", server.salt(), server.public_key()).unwrap();
        let (m2, server_key) = server.verify(client.public_key(), client.proof()).unwrap();
        assert_eq!(client.verify(&m2).unwrap(), server_key);

        let wrong =
            SrpClient::new(HAP_USERNAME, b"4321", server.salt(), server.public_key()).unwrap();
        let err = server
            .verify(wrong.public_key(), wrong.proof())
            .unwrap_err();
        assert_eq!(err.kind(), crate::error::ErrorKind::Auth);
        assert!(server.verify(&[0u8; 384], &[0u8; 64]).is_err());
    }

    #[test]
    fn rejects_bad_server_values() {
        let salt = [1u8; 16];
        let bad_proof = {
            let c = SrpClient::new(HAP_USERNAME, b"1234", &salt, &[2u8; 384]).unwrap();
            c.verify(&[0u8; 64]).unwrap_err()
        };
        assert_eq!(bad_proof.kind(), crate::error::ErrorKind::Auth);
        for bad in [
            vec![0u8; 384],
            N_3072.to_vec(),
            vec![0xffu8; 384],
            vec![1u8; 600],
        ] {
            assert!(SrpClient::new(HAP_USERNAME, b"1234", &salt, &bad).is_err());
        }
        let c = SrpClient::new(HAP_USERNAME, b"1234", &salt, &[2u8; 384]).unwrap();
        assert!(c.verify(&[]).is_err());
    }

    #[test]
    fn random_client_has_full_length_values() {
        let salt = [7u8; 16];
        let b_pub = [3u8; 384];
        for _ in 0..4 {
            let c = SrpClient::new(HAP_USERNAME, b"3939", &salt, &b_pub).unwrap();
            assert_eq!(c.public_key().len(), N_LEN);
            assert_ne!(c.public_key()[0], 0);
        }
    }
}
