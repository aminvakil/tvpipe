//! AirPlay mirroring sender (PLAN §7).
//!
//! Owners: W5 (pairing, secure channel, info, credentials), W6 (`fairplay/`,
//! LGPL port), W7 (mirror, audio, timing, keys, sink).

pub mod audio;
pub mod credentials;
pub mod discovery;
pub mod fairplay;
pub mod info;
pub mod keys;
pub mod mirror;
pub mod pair;
pub mod pairing;
pub mod secure;
pub mod timing;
