//! A public HTTPS URL for a loopback page (`--tunnel`, docs/tunnels.md).
//!
//! A [`Tunnel`] gives the page's server (on `127.0.0.1:PORT`) a public
//! hostname: visitors reach `https://HOST/` and the provider connects to
//! `127.0.0.1:PORT` for each of their connections, plain HTTP/1.1 with
//! WebSocket upgrades passed through. The provider keeps the tunnel up
//! (reconnecting under the same hostname) until it is dropped.
//!
//! The tunnel only carries requests; what they may do is decided by the
//! page's server (`browser::access`): the tunnel's hostname is accepted
//! as `Host` (and `https://HOST` as `Origin`) with a token of its own.

use std::net::SocketAddr;
use std::sync::Arc;

use crate::error::Result;

/// The visitor's address behind a local connection: from the local
/// address of the provider's connection to `127.0.0.1:PORT`.
pub type PeerLookup = Arc<dyn Fn(SocketAddr) -> Option<String> + Send + Sync>;

/// A running tunnel; dropping it closes it.
pub trait Tunnel: Send {
    /// The public hostname (HTTPS on port 443).
    fn host(&self) -> &str;
    /// The service, for messages.
    fn name(&self) -> &'static str;
    /// Who is behind a local connection, if the provider knows.
    fn peer_lookup(&self) -> Option<PeerLookup> {
        None
    }
}

/// Starts a tunnel to `127.0.0.1:port`; returns once its URL works.
/// `stop` aborts a slow first setup (Ctrl-C).
pub fn start(_port: u16, _stop: &dyn Fn() -> bool) -> Result<Box<dyn Tunnel>> {
    Err(crate::error::Error::unsupported(
        "--tunnel: this build has no tunnel provider",
    ))
}
