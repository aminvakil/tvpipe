//! Files tvpipe serves itself under `/_tvpipe/`: the game library
//! (`lib/`, built from `npm/party` by `npm run build`) and the standard
//! launcher (`launcher/`). Embedded in the binary; with
//! `TVPIPE_PLAYSHARE_WEB=DIR` they are read from `DIR` instead (working
//! on the library without rebuilding tvpipe).

use std::borrow::Cow;
use std::path::Path;

/// One embedded file: its path below `web/` and its bytes.
pub struct Asset {
    pub path: &'static str,
    pub bytes: &'static [u8],
}

macro_rules! web {
    ($($file:literal),* $(,)?) => {
        &[$(Asset {
            path: $file,
            bytes: include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/playshare/web/", $file)),
        }),*]
    };
}

/// Every embedded file (a test compares the list with the directory).
pub static FILES: &[Asset] = web![
    "lib/client.js",
    "lib/emitter.js",
    "lib/index.js",
    "lib/limits.js",
    "lib/names.js",
    "lib/party.js",
    "lib/relay.js",
    "lib/sync.js",
    "lib/tvpipe.js",
    "launcher/index.html",
    "launcher/launcher.css",
    "launcher/launcher.js",
];

/// The source directory (for the test and dev mode).
pub const SOURCE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/playshare/web");

/// The file at `path` (below `web/`, already a clean path).
pub fn get(path: &str) -> Option<Cow<'static, [u8]>> {
    if let Some(dir) = std::env::var_os("TVPIPE_PLAYSHARE_WEB").filter(|d| !d.is_empty()) {
        return std::fs::read(Path::new(&dir).join(path))
            .ok()
            .map(Cow::Owned);
    }
    FILES
        .iter()
        .find(|f| f.path == path)
        .map(|f| Cow::Borrowed(f.bytes))
}
