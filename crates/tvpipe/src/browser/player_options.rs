//! How a `--to browser` receiver is set up ([`PlayerOptions`]) and its
//! lasting token: always compiled, so sessions can plan for browser
//! receivers without the `player` feature.

use std::time::Duration;

use crate::error::Result;
use crate::net::tunnel;
use crate::storage::{Storage, PLAYER_TOKEN};

/// The page's port without `--port`: the same every run, so the URL (with
/// the token kept in the config directory) stays the same.
pub const DEFAULT_PORT: u16 = 8765;
/// How a `--to browser` receiver is set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerOptions {
    /// Name in logs.
    pub name: String,
    /// Port on 127.0.0.1: `None` is [`DEFAULT_PORT`], or any free one
    /// (with a warning) when that is taken; `Some(0)`: any free one.
    pub port: Option<u16>,
    /// The URL's token; `None`: a new random one for this run.
    pub token: Option<String>,
    /// Open the page with the OS's opener.
    pub open: bool,
    /// Block in [`BrowserSink::bind`] until a tab is connected.
    pub wait_for_viewer: bool,
    /// Playout delay: the page shows a frame this long after its instant
    /// on the session's clock.
    pub latency_ms: u32,
    /// Offer audio.
    pub audio: bool,
    /// Also serve the page on a public URL (`--tunnel`, docs/tunnels.md).
    pub tunnel: Option<tunnel::Provider>,
    /// The public link lets new viewers in for this long (`--tunnel-ttl`).
    pub tunnel_ttl: Option<Duration>,
}

impl Default for PlayerOptions {
    fn default() -> PlayerOptions {
        PlayerOptions {
            name: "browser".into(),
            port: Some(0),
            token: None,
            open: false,
            wait_for_viewer: false,
            latency_ms: 100,
            audio: true,
            tunnel: None,
            tunnel_ttl: None,
        }
    }
}

/// A new random token (16 bytes, hex).
pub(crate) fn new_token() -> Result<String> {
    Ok(crate::util::hex::encode(
        &crate::crypto::ring::random_array::<16>()?,
    ))
}

pub(crate) fn valid_token(t: &str) -> bool {
    t.len() == 32
        && t.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The player page's token, kept in the storage's `player-token` so its
/// URL stays the same across runs (an open tab or an
/// OBS source reconnects to the next session). Made on first use, or
/// anew with `rotate` (`--new-token`); the file is private (0600 on
/// unix). Unlike the share page, which captures the user's screen, the
/// player only shows what tvpipe streams anyway, so a lasting token is
/// an acceptable trade.
pub fn persistent_token(storage: &Storage, rotate: bool) -> Result<String> {
    if !rotate {
        match storage.load(PLAYER_TOKEN)? {
            Some(t) if valid_token(t.trim()) => return Ok(t.trim().to_owned()),
            Some(_) => warn!(
                "{}: not a token; making a new one",
                storage.describe(PLAYER_TOKEN)
            ),
            None => {}
        }
    }
    let t = new_token()?;
    storage.save(PLAYER_TOKEN, &format!("{t}\n"))?;
    if rotate {
        info!("browser: new player token (old URLs stop working)");
    }
    Ok(t)
}
