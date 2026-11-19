//! Mozilla's root list (CCADB, CDLA-Permissive-2.0) as trust anchors
//! (subject, public key, name constraints): `roots/mozilla.rs`, generated
//! from `roots/mozilla.pem` (a third of its size; scripts/update-roots.sh).
//! For services behind public CAs: OpenTunnel's API and relay, and
//! api.trycloudflare.com.

use rustls::pki_types::TrustAnchor;
use rustls::RootCertStore;

/// Subject, subjectPublicKeyInfo, nameConstraints (DER).
type Anchor = (&'static [u8], &'static [u8], Option<&'static [u8]>);

include!("roots/mozilla.rs");

/// The roots as a rustls store.
pub fn store() -> RootCertStore {
    RootCertStore {
        roots: ROOTS
            .iter()
            .map(|(subject, spki, nc)| TrustAnchor {
                subject: (*subject).into(),
                subject_public_key_info: (*spki).into(),
                name_constraints: nc.map(Into::into),
            })
            .collect(),
    }
}
