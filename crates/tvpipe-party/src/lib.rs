//! Party games on tvpipe, hosted in Rust (docs/multiscreen-design.md).
//!
//! Phones join with a QR code on the TV, the host keeps rooms, players
//! and their views in sync over WebSockets, speaking the protocol of
//! `@tvpipe/party` (the same browser client, the same vectors).

// Until the server (step 3 of the plan) uses every piece.
#![allow(dead_code)]

mod names;
mod sync;
#[cfg(test)]
mod testutil;
mod value;

pub use value::{Number, Object, Value};

/// The browser client (`/party/client.js`), shared with `@tvpipe/party`.
pub const CLIENT_JS: &str = include_str!("../web/party-client.js");

#[cfg(test)]
mod tests {
    #[test]
    fn client_matches_the_npm_copy() {
        let npm = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../npm/party/client/party-client.js"
        );
        // Outside the repository (the published crate) there is no npm copy.
        if let Ok(other) = std::fs::read_to_string(npm) {
            assert!(
                other == super::CLIENT_JS,
                "npm/party/client/party-client.js differs from web/party-client.js (the canonical copy)"
            );
        }
    }
}
