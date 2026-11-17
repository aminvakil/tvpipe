//! AirPlay mirroring sender (PLAN §7).
//!
//! Owners: W5 (pairing, secure channel, info, credentials), W6 (`fairplay/`,
//! clean room), W7 (mirror, audio, timing, sink).

pub mod credentials;
pub mod discovery;
pub mod fairplay;
pub mod info;
pub mod pair;
pub mod pairing;
pub mod secure;
