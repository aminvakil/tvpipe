//! Who may use a page's server (the player's and the share page's): the
//! `Host`, `?t=` token and WebSocket `Origin` checks, for loopback and,
//! with `--tunnel` (docs/tunnels.md), for one public hostname.
//!
//! - **Local:** `Host` is `127.0.0.1:PORT`, `localhost:PORT` or
//!   `[::1]:PORT` (anything else is DNS rebinding), the token is the
//!   page's, and a browser's `Origin` is `http://` one of those.
//! - **Public:** `Host` is the tunnel's hostname exactly, the token is one
//!   made for this run only (a token that went over the internet does not
//!   outlive the run, and the local URL's lasting player token never
//!   leaves the machine), and a browser's `Origin` is `https://HOST`. The
//!   public link may expire (`--tunnel-ttl`): later requests get `410`,
//!   connections already open go on.
//!
//! Which way a request came is decided by `Host` alone: the tunnel client
//! connects from loopback like everything else. A request that names the
//! public host must also carry the public token, so the two never mix.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::crypto::ring::ct_eq;
use crate::error::{Error, Result};
use crate::net::http::{Request, Response};
use crate::net::httpd;
use crate::net::tunnel::PeerLookup;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Local,
    Public,
}

struct Public {
    host: String,
    token: String,
    expires: Option<Instant>,
    peers: Option<PeerLookup>,
    expiry_logged: AtomicBool,
}

/// The checks of one server (see the module docs).
pub struct Access {
    /// For messages.
    name: String,
    port: u16,
    token: String,
    public: OnceLock<Public>,
}

/// A request that passed [`Access::check`].
pub struct Checked<'a> {
    pub via: Via,
    pub path: &'a str,
    pub token_ok: bool,
}

impl Access {
    pub fn new(name: &str, port: u16, token: String) -> Access {
        Access {
            name: name.to_owned(),
            port,
            token,
            public: OnceLock::new(),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Accepts `host` (a tunnel's hostname) from now on, for `ttl` (or
    /// for good), with a new token; returns that token. Only one public
    /// host per run.
    pub fn publish(
        &self,
        host: &str,
        ttl: Option<Duration>,
        peers: Option<PeerLookup>,
    ) -> Result<String> {
        let token = crate::util::hex::encode(&crate::crypto::ring::random_array::<16>()?);
        let public = Public {
            host: host.to_ascii_lowercase(),
            token: token.clone(),
            expires: ttl.map(|t| Instant::now() + t),
            peers,
            expiry_logged: AtomicBool::new(false),
        };
        self.public
            .set(public)
            .map_err(|_| Error::usage("a page has one public host"))?;
        Ok(token)
    }

    /// The public host, once published.
    pub fn public_host(&self) -> Option<&str> {
        self.public.get().map(|p| p.host.as_str())
    }

    /// Whether the public link has expired.
    pub fn expired(&self) -> bool {
        self.public
            .get()
            .and_then(|p| p.expires)
            .is_some_and(|e| Instant::now() >= e)
    }

    fn is_loopback(&self, authority: &str) -> bool {
        let port = self.port;
        [
            format!("127.0.0.1:{port}"),
            format!("localhost:{port}"),
            format!("[::1]:{port}"),
        ]
        .iter()
        .any(|h| h.eq_ignore_ascii_case(authority))
    }

    fn is_public(&self, authority: &str) -> bool {
        self.public.get().is_some_and(|p| {
            let a = authority.strip_suffix(":443").unwrap_or(authority);
            a.eq_ignore_ascii_case(&p.host)
        })
    }

    /// How a request with this `Host` came, or `None`: refuse it (421).
    pub fn via(&self, host: &str) -> Option<Via> {
        if self.is_loopback(host) {
            Some(Via::Local)
        } else if self.is_public(host) {
            Some(Via::Public)
        } else {
            None
        }
    }

    /// Whether the query string (`a=b&t=TOKEN`) has the token for `via`.
    pub fn token_ok(&self, via: Via, query: &str) -> bool {
        let want = match via {
            Via::Local => self.token.as_str(),
            Via::Public => match self.public.get() {
                Some(p) => p.token.as_str(),
                None => return false,
            },
        };
        query.split('&').any(|kv| {
            kv.strip_prefix("t=")
                .is_some_and(|t| ct_eq(t.as_bytes(), want.as_bytes()))
        })
    }

    /// Whether a WebSocket's `Origin` is the page's own (`None`: not a
    /// browser, let in; the token still applies).
    pub fn origin_ok(&self, via: Via, origin: Option<&str>) -> bool {
        let Some(o) = origin else { return true };
        match via {
            Via::Local => o
                .strip_prefix("http://")
                .is_some_and(|a| self.is_loopback(a)),
            Via::Public => o
                .strip_prefix("https://")
                .is_some_and(|a| self.is_public(a)),
        }
    }

    /// The checks every request goes through: `Host` (421), the method
    /// (405), an expired public link (410). `free` paths need no token;
    /// a public request for another path without it is logged.
    pub fn check<'a>(
        &self,
        req: &'a Request,
        free: &[&str],
    ) -> std::result::Result<Checked<'a>, Response> {
        let Some(via) = self.via(req.headers.get("host").unwrap_or("")) else {
            return Err(httpd::error_response(421, "Misdirected Request"));
        };
        if req.method != "GET" && req.method != "HEAD" {
            return Err(httpd::error_response(405, "Method Not Allowed"));
        }
        if via == Via::Public && self.expired() {
            if let Some(p) = self.public.get() {
                if !p.expiry_logged.swap(true, Ordering::SeqCst) {
                    info!(
                        "{}: the public link expired; it no longer lets anyone in",
                        self.name
                    );
                }
            }
            return Err(httpd::error_response(410, "Gone (this link has expired)"));
        }
        let (path, query) = req.target.split_once('?').unwrap_or((&req.target, ""));
        let token_ok = self.token_ok(via, query);
        let free_path = free.contains(&path) || path.starts_with("/fonts/");
        if via == Via::Public && !token_ok && !free_path {
            info!(
                "{}: refused a public request without the token: {path}",
                self.name
            );
        }
        Ok(Checked {
            via,
            path,
            token_ok,
        })
    }

    /// Who is behind a public request (inside a handler): the visitor's
    /// address as the tunnel reports it.
    pub fn remote_peer(&self, local: Option<SocketAddr>) -> Option<String> {
        let lookup = self.public.get()?.peers.as_ref()?;
        local.and_then(|a| lookup(a)).filter(|p| !p.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::http::Version;

    fn req(target: &str, host: &str) -> Request {
        Request::new("GET", target, Version::Http11).header("Host", host)
    }

    #[test]
    fn local_and_public() {
        let a = Access::new("test", 8765, "local".into());
        assert_eq!(a.via("127.0.0.1:8765"), Some(Via::Local));
        assert_eq!(a.via("LOCALHOST:8765"), Some(Via::Local));
        assert_eq!(a.via("[::1]:8765"), Some(Via::Local));
        assert_eq!(a.via("127.0.0.1:8766"), None);
        assert_eq!(a.via("x.opentunnel.xyz"), None);
        assert!(!a.token_ok(Via::Public, "t="));
        let public = a.publish("X.opentunnel.xyz", None, None).unwrap();
        assert!(a.publish("y.opentunnel.xyz", None, None).is_err());
        assert_eq!(a.public_host(), Some("x.opentunnel.xyz"));
        assert_eq!(a.via("x.opentunnel.xyz"), Some(Via::Public));
        assert_eq!(a.via("x.opentunnel.xyz:443"), Some(Via::Public));
        assert_eq!(a.via("x.opentunnel.xyz:8765"), None);
        assert_eq!(a.via("y.opentunnel.xyz"), None);
        // Each way has its own token.
        assert!(a.token_ok(Via::Local, "delay=1&t=local"));
        assert!(!a.token_ok(Via::Public, "t=local"));
        assert!(a.token_ok(Via::Public, &format!("t={public}")));
        assert!(!a.token_ok(Via::Local, &format!("t={public}")));
        assert!(!a.token_ok(Via::Local, "t=loca"));
        // Origins.
        assert!(a.origin_ok(Via::Local, None));
        assert!(a.origin_ok(Via::Local, Some("http://localhost:8765")));
        assert!(!a.origin_ok(Via::Local, Some("https://x.opentunnel.xyz")));
        assert!(a.origin_ok(Via::Public, Some("https://x.opentunnel.xyz")));
        assert!(!a.origin_ok(Via::Public, Some("http://x.opentunnel.xyz")));
        assert!(!a.origin_ok(Via::Public, Some("https://evil.example")));
        assert!(!a.origin_ok(Via::Public, Some("http://127.0.0.1:8765")));
        assert!(!a.expired());
        assert!(a.remote_peer(None).is_none());
    }

    #[test]
    fn checks_and_expiry() {
        let a = Access::new("test", 1, "local".into());
        let lookup: PeerLookup = std::sync::Arc::new(|addr: SocketAddr| {
            (addr.port() == 5).then(|| "203.0.113.9:4000".to_owned())
        });
        let t = a
            .publish("p.example", Some(Duration::from_millis(200)), Some(lookup))
            .unwrap();
        let status = |r: &Request| a.check(r, &["/x.js"]).err().map(|e| e.status);
        assert_eq!(status(&req("/", "evil:1")), Some(421));
        let post = Request::new("POST", "/", Version::Http11).header("Host", "127.0.0.1:1");
        assert_eq!(status(&post), Some(405));
        let r = req(&format!("/ws?t={t}"), "p.example");
        let c = a.check(&r, &[]).ok().unwrap();
        assert!(c.via == Via::Public && c.token_ok && c.path == "/ws");
        let r = req("/x.js", "p.example");
        let c = a.check(&r, &["/x.js"]).ok().unwrap();
        assert!(!c.token_ok);
        assert_eq!(
            a.remote_peer(Some("127.0.0.1:5".parse().unwrap()))
                .as_deref(),
            Some("203.0.113.9:4000")
        );
        assert_eq!(a.remote_peer(Some("127.0.0.1:6".parse().unwrap())), None);
        std::thread::sleep(Duration::from_millis(250));
        assert!(a.expired());
        assert_eq!(status(&req(&format!("/?t={t}"), "p.example")), Some(410));
        // The local URL does not expire.
        assert!(a.check(&req("/?t=local", "127.0.0.1:1"), &[]).is_ok());
    }
}
