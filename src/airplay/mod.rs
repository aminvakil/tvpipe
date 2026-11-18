//! AirPlay mirroring sender (PLAN §7).
//!
//! Owners: W5 (pairing, secure channel, info, credentials), W6 (`fairplay/`,
//! LGPL port), W7 (mirror, audio, timing, keys, sink).

pub mod audio;
pub mod credentials;
pub mod discovery;
pub mod events;
pub mod fairplay;
pub mod fake_mirror;
pub mod info;
pub mod keys;
pub mod legacy_pair;
pub mod mdc;
pub mod mirror;
pub mod pair;
pub mod pairing;
pub mod policy;
pub mod secure;
pub mod session;
pub mod setup;
pub mod sink;
pub mod timing;
