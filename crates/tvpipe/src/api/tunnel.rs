//! Public links for any loopback port (`tvpipe debug tunnel PORT`,
//! docs/tunnels.md) and QR codes as data (`tvpipe debug qr`).

use std::fmt;

use super::TunnelProvider;
use crate::error::Result;
use crate::net::tunnel;

/// A public HTTPS link to a server on `127.0.0.1:PORT`: visitors reach
/// `https://HOST/` and the provider connects to the port for each of
/// their connections (plain HTTP/1.1, WebSocket upgrades passed through).
/// Reconnects keep the hostname. Dropping it closes the link.
///
/// The tunnel only carries requests: **the server at the port decides
/// who may do what**. Requests arrive from loopback with `Host` set to
/// the public name.
pub struct Tunnel {
    inner: Box<dyn tunnel::Tunnel>,
    provider: TunnelProvider,
}

impl fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tunnel")
            .field("host", &self.host())
            .field("provider", &self.provider)
            .finish()
    }
}

impl Tunnel {
    /// Starts a tunnel to `127.0.0.1:port`; returns once its URL works
    /// (a Cloudflare quick tunnel takes several seconds). Needs the
    /// `tunnel` feature.
    pub fn start(port: u16, provider: TunnelProvider) -> Result<Tunnel> {
        Tunnel::start_with_stop(port, provider, &|| false)
    }

    /// [`start`](Tunnel::start), aborted when `stop` returns `true`
    /// (polled while the first setup is slow).
    pub fn start_with_stop(
        port: u16,
        provider: TunnelProvider,
        stop: &dyn Fn() -> bool,
    ) -> Result<Tunnel> {
        let inner = tunnel::start(port, provider, stop)?;
        Ok(Tunnel { inner, provider })
    }

    /// A Cloudflare quick tunnel, else (Cloudflare refused or failed, as
    /// trycloudflare does with `429` under load) OpenTunnel.
    pub fn start_auto(port: u16, stop: &dyn Fn() -> bool) -> Result<Tunnel> {
        match Tunnel::start_with_stop(port, TunnelProvider::Cloudflare, stop) {
            Ok(t) => Ok(t),
            Err(e) if stop() => Err(e),
            Err(e) => {
                warn!(
                    "tunnel: Cloudflare failed ({}); trying OpenTunnel",
                    e.message()
                );
                Tunnel::start_with_stop(port, TunnelProvider::OpenTunnel, stop)
            }
        }
    }

    /// The public hostname (HTTPS on port 443).
    pub fn host(&self) -> &str {
        self.inner.host()
    }

    /// `https://HOST/`.
    pub fn url(&self) -> String {
        format!("https://{}/", self.inner.host())
    }

    /// Who carries the link.
    pub fn provider(&self) -> TunnelProvider {
        self.provider
    }

    /// The provider for messages ("Cloudflare quick tunnel", "OpenTunnel").
    pub fn provider_name(&self) -> &'static str {
        self.inner.name()
    }

    /// The visitor's address behind a local connection (the peer address
    /// the server saw, from 127.0.0.1), when the provider knows it.
    /// Cloudflare's visitors also arrive with `cf-connecting-ip`.
    pub fn visitor(&self, local_peer: std::net::SocketAddr) -> Option<String> {
        self.inner.peer_lookup().and_then(|f| f(local_peer))
    }
}

/// A QR code (byte mode, error correction M, else L).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrCode(crate::util::qr::Qr);

impl QrCode {
    /// The code for `text` at the smallest version that fits, `None` when
    /// it is too long for version 40.
    pub fn encode(text: &str) -> Option<QrCode> {
        crate::util::qr::encode(text.as_bytes(), crate::util::qr::MAX_VERSION).map(QrCode)
    }

    /// The version (1–40).
    pub fn version(&self) -> u8 {
        self.0.version
    }

    /// Modules per side, without a quiet zone.
    pub fn size(&self) -> usize {
        self.0.size
    }

    /// Whether the module in column `x`, row `y` is dark (outside: light).
    pub fn dark(&self, x: usize, y: usize) -> bool {
        self.0.dark(x, y)
    }

    /// One string per module row, `1` dark and `0` light (`tvpipe debug qr
    /// --rows`, the party protocol's `join.qr.rows`).
    pub fn rows(&self) -> Vec<String> {
        self.0.rows()
    }

    /// A standalone SVG document, one user unit per module, `quiet` light
    /// modules around the symbol.
    pub fn to_svg(&self, quiet: usize) -> String {
        self.0.to_svg(quiet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_code() {
        let qr =
            QrCode::encode("https://example.trycloudflare.com/?r=BCDF#k=0123456789abcdefghijkl")
                .unwrap();
        assert_eq!(qr.size(), 4 * qr.version() as usize + 17);
        let rows = qr.rows();
        assert_eq!(rows.len(), qr.size());
        assert!(rows.iter().all(|r| r.len() == qr.size()));
        // The finder pattern's corner modules are dark, outside is light.
        assert!(qr.dark(0, 0) && qr.dark(qr.size() - 1, 0) && qr.dark(0, qr.size() - 1));
        assert!(!qr.dark(qr.size(), 0));
        assert!(qr.to_svg(4).starts_with("<svg"));
        assert!(QrCode::encode(&"x".repeat(4000)).is_none());
    }

    #[test]
    #[cfg(not(feature = "tunnel"))]
    fn tunnel_needs_the_feature() {
        assert!(Tunnel::start(1, TunnelProvider::Cloudflare).is_err());
    }
}
