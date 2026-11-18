//! Streaming a browser (decision 0012).
//!
//! Owner: W12 (share a tab, option C); W13 adds the headless browser
//! (option A) on top of the same capture script and WebSocket.
//!
//! - [`share`]: `tvpipe share`, the 127.0.0.1 page server and the input
//!   the page's encoded H.264 and PCM feed.
//! - [`protocol`]: the page's WebSocket messages (docs/share-protocol.md).
//! - [`platform`]: opening a URL with the OS's opener (the one place with
//!   `cfg(target_os)` in this module, like a config path).

pub mod platform;
pub mod protocol;
pub mod share;
