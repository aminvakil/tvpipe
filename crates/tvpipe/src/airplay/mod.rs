//! AirPlay mirroring sender (PLAN §7).
//!
//! Owners: W5 (pairing, secure channel, info, credentials), W6 (FairPlay:
//! the LGPL `tvpipe-fairplay` crate), W7 (mirror, audio, timing, keys,
//! sink).

#[cfg(feature = "airplay")]
pub mod audio;
pub mod credentials;
pub mod discovery;
#[cfg(feature = "airplay")]
pub mod events;
#[cfg(feature = "airplay")]
pub mod fake_mirror;
#[cfg(feature = "airplay")]
pub mod info;
#[cfg(feature = "airplay")]
pub mod keys;
#[cfg(feature = "airplay")]
pub mod legacy_pair;
#[cfg(feature = "airplay")]
pub mod mdc;
#[cfg(feature = "airplay")]
pub mod mirror;
mod options;
#[cfg(feature = "airplay")]
pub mod pair;
#[cfg(feature = "airplay")]
pub mod pairing;
#[cfg(feature = "airplay")]
pub mod policy;
#[cfg(feature = "airplay")]
pub mod secure;
#[cfg(feature = "airplay")]
pub mod session;
#[cfg(feature = "airplay")]
pub mod setup;
#[cfg(feature = "airplay")]
pub mod sink;
#[cfg(feature = "airplay")]
pub mod timing;

pub use options::{MirrorOptions, ALAC_FRAME_SAMPLES};
