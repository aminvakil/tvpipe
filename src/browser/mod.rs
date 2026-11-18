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
//! - [`headless`]: `tvpipe share URL` (W13), a browser of tvpipe's own
//!   that shares its tab through the same capture script ([`launch`]
//!   starts it, [`cdp`] drives a Chromium-based one over the DevTools
//!   protocol, Firefox over WebDriver BiDi with [`bidi`]).
//! - [`screen`]: `tvpipe screen`, a browser of tvpipe's own that
//!   captures a screen or a window with the computer's sound.
//! - [`fonts`]: the fonts of the share and player pages, compiled in.
//! - [`platform`]: opening a URL with the OS's opener and the browsers'
//!   install paths (the one place with `cfg(target_os)` in this module,
//!   like a config path).

pub mod bidi;
pub mod cdp;
pub mod fonts;
pub mod headless;
pub mod launch;
pub mod layout;
pub mod platform;
pub mod player;
pub mod protocol;
pub mod screen;
pub mod share;
