#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
// Much of the internal API is only used by some features or the tests.
#![allow(dead_code)]
// Builds without some features leave imports and parameters unused; the
// build with every feature is linted strictly.
#![cfg_attr(
    not(all(
        feature = "airplay",
        feature = "cast",
        feature = "aac",
        feature = "player",
        feature = "share",
        feature = "headless",
        feature = "tunnel",
        feature = "files",
        feature = "ffmpeg",
        feature = "h264"
    )),
    allow(unused)
)]

#[macro_use]
pub mod log;

mod airplay;
mod api;
mod browser;
mod cast;
// Reachable only through `__cli`, which is not public API.
#[allow(missing_docs)]
mod codec;
mod crypto;
// Reachable only through `__cli`, which is not public API.
#[allow(missing_docs)]
mod ctx;
mod error;
pub mod events;
mod media;
mod net;
// Reachable only through `__cli`, which is not public API.
#[allow(missing_docs)]
mod opts;
// Reachable only through `__cli`, which is not public API.
#[allow(missing_docs)]
mod session;
mod storage;
mod types;
// Reachable only through `__cli`, which is not public API.
#[allow(missing_docs)]
mod util;

pub mod recipes;

pub use api::{
    discover, discover_with, play, preview, share, stream, test_pattern, AudioWriter, ConfigFormat,
    Discovery, Frame, FrameBuffer, Input, Options, PairingState, PixelFormat, PlayerOptions,
    ReceiverInfo, Session, SessionState, Stopper, Stream, TunnelProvider, Written, H264,
};
#[cfg(feature = "airplay")]
pub use api::{pair, pair_with, unpair, Pairing};
pub use error::{Error, ErrorKind, Result};
pub use events::{EndInfo, Event, Paired, PinRequest, Started, Stats};
pub use net::discover::{AirPlayService, CastService, DeviceInfo as Device, Endpoint};
pub use storage::{storage_fn, Storage, StorageBackend, StorageFn};
pub use types::Encoder;

/// The game input format (docs/slrw.md): what `tvpipe play -` reads on
/// stdin, for programs that run the `tvpipe` command instead of linking
/// the library.
pub mod slrw {
    pub use crate::media::slrw::{
        write_control, write_h264, write_h264_config, write_magic, write_packet, Kind, Packet,
        SlrwReader, CONFIG_ANNEX_B, CONFIG_AVCC, CONTROL_KEYFRAME_REQUESTS, H264_ANNEX_B,
        H264_KEYFRAME, KEYFRAME_REQUEST_LINE, MAGIC,
    };
}

/// What the `tvpipe` command-line tool needs from the internals. Not part
/// of the public API: it changes with the CLI.
#[doc(hidden)]
#[allow(missing_docs)]
pub mod __cli {
    #[cfg(feature = "airplay")]
    pub use crate::airplay::pair::pair;
    pub use crate::api::__spawn_command as spawn;
    pub use crate::ctx::Ctx;

    /// `discover` with `--interface`s, `paired` from `storage`.
    pub fn discover(
        timeout: std::time::Duration,
        interfaces: &[std::net::Ipv4Addr],
        storage: &crate::Storage,
    ) -> crate::Result<Vec<crate::Device>> {
        let devices = crate::net::discover::discover(timeout, interfaces)?;
        let store = crate::net::discover::stored_pairings(storage);
        Ok(devices
            .iter()
            .map(|d| crate::Device::new(d, store.as_ref()))
            .collect())
    }

    /// The `devices` event.
    pub fn devices_event(devices: Vec<crate::Device>) -> crate::Event {
        crate::Event::Devices(crate::events::DevicesEvent { devices })
    }

    /// `discover --json`: a pretty-printed array.
    pub fn devices_json(devices: &[crate::Device]) -> String {
        let list = crate::codec::json::Value::array(devices.iter().map(|d| d.to_value()));
        format!("{list:#}\n")
    }
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
