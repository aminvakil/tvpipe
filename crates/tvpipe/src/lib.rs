//! tvpipe: low-latency mirroring of video and audio to AirPlay and Google
//! Cast receivers and browser tabs, as a library.
//!
//! The public API is being built (docs/rust-sdk-design.md); until then
//! the `tvpipe` command-line tool uses the internals through
//! [`__cli`], which is not part of the semver promise.

// Much of the internal API is only used by some features or the tests.
#![allow(dead_code)]

#[macro_use]
pub mod log;

mod airplay;
mod browser;
mod cast;
mod codec;
mod crypto;
mod ctx;
mod error;
mod events;
mod media;
mod net;
mod opts;
mod session;
mod storage;
mod types;
mod util;

/// What the `tvpipe` command-line tool needs from the internals. Not part
/// of the public API: it changes with the CLI.
#[doc(hidden)]
pub mod __cli {
    pub use crate::airplay::pair::pair;
    pub use crate::ctx::Ctx;
    pub use crate::session::{run, GameIo};
    pub use crate::util::stop::Stop;
    pub mod qr {
        pub use crate::util::qr::*;
    }
    pub use crate::util::timer::raise_resolution;

    pub mod error {
        pub use crate::error::*;
    }
    pub mod events {
        pub use crate::events::*;
    }
    pub mod json {
        pub use crate::codec::json::*;
    }
    pub mod opts {
        pub use crate::net::tunnel::Provider;
        pub use crate::opts::*;
        pub use crate::types::Encoder;
        pub use crate::util::ratio::Ratio;
    }
    pub mod slrw {
        pub use crate::media::slrw::*;
    }
}
