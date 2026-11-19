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

/// Who carries the link (`--tunnel[=PROVIDER]`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Provider {
    /// A Cloudflare quick tunnel (`net::cftunnel`): no setup, Cloudflare
    /// ends TLS.
    #[default]
    Cloudflare,
    /// OpenTunnel (`net::opentunnel`): TLS ends here.
    OpenTunnel,
}

impl Provider {
    /// `cloudflare` (`cf`) or `opentunnel` (`ot`).
    pub fn parse(s: &str) -> Option<Provider> {
        match s.trim().to_ascii_lowercase().as_str() {
            "cloudflare" | "cf" | "trycloudflare" => Some(Provider::Cloudflare),
            "opentunnel" | "ot" => Some(Provider::OpenTunnel),
            _ => None,
        }
    }

    /// The provider's name (`cloudflare`, `opentunnel`).
    pub fn name(self) -> &'static str {
        match self {
            Provider::Cloudflare => "cloudflare",
            Provider::OpenTunnel => "opentunnel",
        }
    }
}

/// Starts a tunnel to `127.0.0.1:port`; returns once its URL works.
/// `stop` aborts a slow first setup (Ctrl-C).
#[cfg(feature = "tunnel")]
pub fn start(port: u16, provider: Provider, stop: &dyn Fn() -> bool) -> Result<Box<dyn Tunnel>> {
    Ok(match provider {
        Provider::Cloudflare => Box::new(crate::net::cftunnel::Tunnel::start(port, stop)?),
        Provider::OpenTunnel => Box::new(crate::net::opentunnel::OpenTunnel::start(port, stop)?),
    })
}

/// Without the `tunnel` feature there are no public links.
#[cfg(not(feature = "tunnel"))]
pub fn start(_port: u16, provider: Provider, _stop: &dyn Fn() -> bool) -> Result<Box<dyn Tunnel>> {
    Err(crate::error::Error::unsupported(format!(
        "{}: public links need tvpipe's `tunnel` feature",
        provider.name()
    )))
}

#[cfg(feature = "tunnel")]
impl Tunnel for crate::net::opentunnel::OpenTunnel {
    fn host(&self) -> &str {
        crate::net::opentunnel::OpenTunnel::host(self)
    }

    fn name(&self) -> &'static str {
        "OpenTunnel"
    }

    fn peer_lookup(&self) -> Option<PeerLookup> {
        Some(crate::net::opentunnel::OpenTunnel::peer_lookup(self))
    }
}
