//! Streaming a browser.
//!
//! Owner: W12 (share a tab); W13 adds the headless browser on top of the
//! same capture script and WebSocket.
//!
//! - [`share`]: `tvpipe share`, the 127.0.0.1 page server and the input
//!   the page's encoded H.264 and PCM feed.
//! - [`player`]: `--to browser` (W14), a receiver that plays the stream
//!   in a browser tab (docs/player-protocol.md).
//! - [`protocol`]: the page's WebSocket messages (docs/share-protocol.md).
//! - [`platform`]: opening a URL with the OS's opener (the one place with
//!   `cfg(target_os)` in this module, like a config path).

pub mod layout;
pub mod platform;
pub mod player;
pub mod protocol;
pub mod share;
