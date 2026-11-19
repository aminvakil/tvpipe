//! A tiny HTTP/1.1 server for loopback use (the browser share page).
//!
//! One thread accepts, one thread per connection reads requests with
//! `net::http` framing (keep-alive) and asks the handler for a [`Reply`]:
//! a response, or a response followed by handing the socket over (the
//! WebSocket upgrade), in which case the connection thread becomes the
//! upgraded connection's thread. Only loopback addresses can be bound
//! unless [`Config::lan`] says otherwise (the party server's LAN mode,
//! docs/multiscreen-design.md §3.2): tvpipe's own pages never face a
//! network (a `--tunnel` reaches them from loopback too; the pages' own
//! checks decide, `browser::access`).
//!
//! Sources: RFC 9112.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::net::http::{self, Conn, Request, Response, Version};

/// Connections served at once by default; more are closed right away.
const MAX_CONNECTIONS: usize = 32;
/// An idle keep-alive connection (or a slow request) is closed after this.
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const ACCEPT_POLL: Duration = Duration::from_millis(20);

/// Takes over an upgraded connection: the socket and any bytes the client
/// sent after its request.
pub type Upgrade = Box<dyn FnOnce(TcpStream, Vec<u8>) + Send>;

pub enum Reply {
    Response(Response),
    /// Send the response (`101`), then call the upgrade with the socket.
    Upgrade(Response, Upgrade),
}

pub type Handler = Arc<dyn Fn(&Request) -> Reply + Send + Sync>;

thread_local! {
    /// The client of the request being handled (each connection has its
    /// own thread).
    static PEER: std::cell::Cell<Option<SocketAddr>> = const { std::cell::Cell::new(None) };
}

/// Inside a [`Handler`]: the address the request came from.
pub fn current_peer() -> Option<SocketAddr> {
    PEER.with(|p| p.get())
}

/// A response with a body and the headers every reply here carries.
pub fn response(status: u16, reason: &str, content_type: &str, body: Vec<u8>) -> Response {
    let mut r = Response::new(Version::Http11, status, reason);
    r.headers
        .add("Content-Type", content_type)
        .add("Cache-Control", "no-store")
        .add("X-Content-Type-Options", "nosniff");
    r.body = body;
    r
}

/// A plain-text error response.
pub fn error_response(status: u16, reason: &str) -> Response {
    response(
        status,
        reason,
        "text/plain; charset=utf-8",
        format!("{status} {reason}\n").into_bytes(),
    )
}

/// How a [`Server`] runs.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Connections served at once; more are closed right away.
    pub max_connections: usize,
    /// The largest request body.
    pub max_body: usize,
    /// Allow a non-loopback unicast address (one interface; the
    /// unspecified address is always refused). The handler must then check
    /// `Host` and `Origin` for that address itself.
    pub lan: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            max_connections: MAX_CONNECTIONS,
            max_body: 1 << 20,
            lan: false,
        }
    }
}

pub struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// Binds `addr` (loopback only; port 0 picks one) and serves `handler`.
    pub fn bind(addr: SocketAddr, handler: Handler) -> Result<Server> {
        Server::bind_with(addr, handler, Config::default())
    }

    /// [`bind`](Server::bind) with a [`Config`].
    pub fn bind_with(addr: SocketAddr, handler: Handler, config: Config) -> Result<Server> {
        let ip = addr.ip();
        if !ip.is_loopback() && !(config.lan && !ip.is_unspecified() && !ip.is_multicast()) {
            return Err(Error::usage(format!(
                "httpd: refusing to listen on non-loopback {addr}"
            )));
        }
        let listener =
            TcpListener::bind(addr).map_err(|e| Error::io(format!("httpd: bind {addr}"), e))?;
        let addr = listener
            .local_addr()
            .map_err(|e| Error::io("httpd: local address", e))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| Error::io("httpd: nonblocking listener", e))?;
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let thread = std::thread::Builder::new()
            .name("httpd".into())
            .spawn(move || accept_loop(listener, handler, st, config))
            .map_err(|e| Error::io("httpd: spawn", e))?;
        Ok(Server {
            addr,
            stop,
            thread: Some(thread),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stops accepting (connections in progress end on their own).
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn accept_loop(listener: TcpListener, handler: Handler, stop: Arc<AtomicBool>, config: Config) {
    let active = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::SeqCst) {
        let (tcp, peer) = match listener.accept() {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_POLL);
                continue;
            }
            Err(e) => {
                debug!("httpd: accept: {e}");
                std::thread::sleep(ACCEPT_POLL);
                continue;
            }
        };
        if active.load(Ordering::SeqCst) >= config.max_connections {
            debug!("httpd: too many connections; closing {peer}");
            continue;
        }
        active.fetch_add(1, Ordering::SeqCst);
        let (h, a) = (handler.clone(), active.clone());
        let spawned = std::thread::Builder::new()
            .name("httpd-conn".into())
            .spawn(move || {
                serve(tcp, peer, &h, config.max_body);
                a.fetch_sub(1, Ordering::SeqCst);
            });
        if let Err(e) = spawned {
            active.fetch_sub(1, Ordering::SeqCst);
            debug!("httpd: spawn: {e}");
        }
    }
}

fn serve(tcp: TcpStream, peer: SocketAddr, handler: &Handler, max_body: usize) {
    PEER.with(|p| p.set(Some(peer)));
    // Accepted sockets may inherit the listener's non-blocking mode.
    if tcp.set_nonblocking(false).is_err() || tcp.set_read_timeout(Some(IDLE_TIMEOUT)).is_err() {
        return;
    }
    let _ = tcp.set_write_timeout(Some(IDLE_TIMEOUT));
    let _ = tcp.set_nodelay(true);
    let mut conn = Conn::with_limits(
        tcp,
        http::Limits {
            max_body,
            ..http::Limits::default()
        },
    );
    loop {
        let req = match conn.read_request() {
            Ok(r) => r,
            Err(e) => {
                trace!("httpd: {peer}: {e}");
                return;
            }
        };
        trace!("httpd: {peer}: {} {}", req.method, req.target);
        let close = req.version == Version::Http10
            || req
                .headers
                .get_all("connection")
                .any(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("close")));
        match handler(&req) {
            Reply::Response(mut resp) => {
                if close {
                    resp.headers.set("Connection", "close");
                }
                if req.method == "HEAD" {
                    resp.headers
                        .set("Content-Length", resp.body.len().to_string());
                    resp.body.clear();
                }
                if conn.send_response(&resp).is_err() || close {
                    return;
                }
            }
            Reply::Upgrade(resp, upgrade) => {
                if conn.send_response(&resp).is_err() {
                    return;
                }
                let (tcp, rest) = conn.into_parts();
                let _ = tcp.set_read_timeout(None);
                let _ = tcp.set_write_timeout(None);
                upgrade(tcp, rest);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn get(addr: SocketAddr, raw: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    #[test]
    fn serves_and_upgrades() {
        let handler: Handler = Arc::new(|req: &Request| match req.target.as_str() {
            "/" => Reply::Response(response(200, "OK", "text/plain", b"hi".to_vec())),
            "/up" => Reply::Upgrade(
                Response::new(Version::Http11, 101, "Switching Protocols"),
                Box::new(|mut tcp: TcpStream, rest: Vec<u8>| {
                    let _ = tcp.write_all(&rest);
                    let _ = tcp.write_all(b"|upgraded");
                }),
            ),
            _ => Reply::Response(error_response(404, "Not Found")),
        });
        let mut srv = Server::bind("127.0.0.1:0".parse().unwrap(), handler).unwrap();
        let addr = srv.local_addr();
        let r = get(
            addr,
            "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        );
        assert!(
            r.starts_with("HTTP/1.1 200 OK\r\n") && r.ends_with("\r\n\r\nhi"),
            "{r}"
        );
        assert!(r.contains("Connection: close"), "{r}");
        // Keep-alive: two requests on one connection, then HTTP/1.0 closes.
        let r = get(
            addr,
            "GET /nope HTTP/1.1\r\nHost: x\r\n\r\nHEAD / HTTP/1.0\r\nHost: x\r\n\r\n",
        );
        assert!(r.starts_with("HTTP/1.1 404 Not Found\r\n"), "{r}");
        assert!(
            r.contains("HTTP/1.1 200 OK\r\n") && r.ends_with("Content-Length: 2\r\n\r\n"),
            "{r}"
        );
        // The upgrade gets the bytes after the request.
        let r = get(addr, "GET /up HTTP/1.1\r\nHost: x\r\n\r\nearly");
        assert!(r.starts_with("HTTP/1.1 101 Switching Protocols\r\n"), "{r}");
        assert!(r.ends_with("\r\n\r\nearly|upgraded"), "{r}");
        // Garbage just closes.
        let r = get(addr, "\x00\x01\x02 nonsense\r\n\r\n");
        assert!(r.is_empty(), "{r}");
        srv.shutdown();
    }

    #[test]
    fn loopback_only() {
        let handler: Handler = Arc::new(|_: &Request| Reply::Response(error_response(404, "x")));
        assert!(Server::bind("0.0.0.0:0".parse().unwrap(), handler.clone()).is_err());
        let lan = Config {
            lan: true,
            ..Config::default()
        };
        // The unspecified address stays refused even for LAN servers.
        assert!(Server::bind_with("0.0.0.0:0".parse().unwrap(), handler.clone(), lan).is_err());
        assert!(Server::bind_with("[::]:0".parse().unwrap(), handler.clone(), lan).is_err());
        // A LAN server may still bind loopback.
        assert!(Server::bind_with("127.0.0.1:0".parse().unwrap(), handler, lan).is_ok());
    }

    #[test]
    fn connection_cap() {
        let handler: Handler = Arc::new(|_: &Request| {
            Reply::Response(response(200, "OK", "text/plain", b"ok".to_vec()))
        });
        let config = Config {
            max_connections: 1,
            ..Config::default()
        };
        let srv = Server::bind_with("127.0.0.1:0".parse().unwrap(), handler, config).unwrap();
        let addr = srv.local_addr();
        // The first connection stays open (keep-alive) and holds the slot.
        let mut first = TcpStream::connect(addr).unwrap();
        first
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        first
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut buf = [0u8; 256];
        let n = first.read(&mut buf).unwrap();
        assert!(buf[..n].starts_with(b"HTTP/1.1 200 OK"));
        // The second is closed without an answer.
        let r = get(
            addr,
            "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        );
        assert!(r.is_empty(), "{r}");
    }
}
