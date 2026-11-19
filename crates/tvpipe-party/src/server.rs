//! The party's HTTP server: the phone pages at `/`, the screen pages at
//! `/screen/`, the browser client at `/party/client.js`, and the
//! WebSocket at `/party/ws` (docs/party-design.md §4, §6).
//!
//! It binds loopback (a tunnel reaches it from there), so only these
//! `Host`s are served: `127.0.0.1:PORT`, `localhost:PORT`, `[::1]:PORT`
//! and the tunnel's name (421 otherwise: DNS rebinding). WebSockets need
//! a matching `Origin` (403: cross-site hijacking; clients without one
//! still need a key).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock, RwLock};
use std::time::Instant;

use tvpipe::__web::http::{Request, Response, Version};
use tvpipe::__web::httpd::{self, Handler, Reply};
use tvpipe::__web::websocket;

use crate::assets::{lookup, Assets, Lookup};
use crate::conn::{self, Inbound};
use crate::party::Joins;

/// The default Content-Security-Policy of HTML pages.
pub const DEFAULT_CSP: &str = "default-src 'self'; img-src 'self' data: blob:; media-src 'self' data: blob:; \
     style-src 'self' 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// Looks up the visitor behind a local connection (OpenTunnel).
pub(crate) type Visitor = Box<dyn Fn(SocketAddr) -> Option<String> + Send + Sync>;

pub(crate) struct Shared {
    pub port: OnceLock<u16>,
    /// Public host names (a tunnel's), lowercase.
    pub hosts: RwLock<Vec<String>>,
    pub visitor: RwLock<Option<Visitor>>,
    pub joins: Joins,
    pub sockets: Arc<AtomicUsize>,
    pub next_conn: AtomicU64,
    pub inbox: Mutex<mpsc::Sender<Inbound>>,
    pub phone: Option<&'static Assets>,
    pub screen: Option<&'static Assets>,
    pub csp: Option<String>,
    pub max_sockets: usize,
    pub max_message: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Via {
    Local,
    Public,
}

impl Shared {
    fn port(&self) -> u16 {
        self.port.get().copied().unwrap_or(0)
    }

    fn via(&self, req: &Request) -> Option<Via> {
        let host = req.headers.get("host")?.trim().to_ascii_lowercase();
        let port = self.port();
        if [
            format!("127.0.0.1:{port}"),
            format!("localhost:{port}"),
            format!("[::1]:{port}"),
        ]
        .contains(&host)
        {
            return Some(Via::Local);
        }
        let bare = host.strip_suffix(":443").unwrap_or(&host);
        let hosts = self.hosts.read().unwrap_or_else(|e| e.into_inner());
        hosts.iter().any(|h| h == bare).then_some(Via::Public)
    }

    fn origin_ok(&self, req: &Request) -> bool {
        // Not a browser: it still needs a key.
        let Some(origin) = req.headers.get("origin") else {
            return true;
        };
        let o = origin.trim().to_ascii_lowercase();
        let port = self.port();
        if [
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
            format!("http://[::1]:{port}"),
        ]
        .contains(&o)
        {
            return true;
        }
        let hosts = self.hosts.read().unwrap_or_else(|e| e.into_inner());
        o.strip_prefix("https://")
            .is_some_and(|h| hosts.iter().any(|x| x == h))
    }

    fn address(&self, req: &Request, via: Via) -> String {
        let peer = httpd::current_peer();
        if via == Via::Public {
            if let Some(cf) = req.headers.get("cf-connecting-ip") {
                if cf.len() < 64 {
                    return cf.trim().to_owned();
                }
            }
            let visitor = self.visitor.read().unwrap_or_else(|e| e.into_inner());
            if let (Some(f), Some(p)) = (visitor.as_ref(), peer) {
                if let Some(v) = f(p) {
                    return v;
                }
            }
        }
        peer.map_or_else(|| "?".to_owned(), |p| p.ip().to_string())
    }
}

fn plain(status: u16, reason: &str) -> Response {
    let mut r = Response::new(Version::Http11, status, reason);
    r.headers
        .add("Content-Type", "text/plain; charset=utf-8")
        .add("X-Content-Type-Options", "nosniff");
    r.body = format!("{status} {reason}\n").into_bytes();
    r
}

fn redirect(location: String) -> Response {
    let mut r = Response::new(Version::Http11, 301, "Moved Permanently");
    r.headers
        .add("Location", location)
        .add("Content-Length", "0");
    r
}

pub(crate) fn handler(shared: Arc<Shared>) -> Handler {
    Arc::new(move |req: &Request| request(&shared, req))
}

fn request(shared: &Arc<Shared>, req: &Request) -> Reply {
    let Some(via) = shared.via(req) else {
        return Reply::Response(plain(421, "Misdirected Request"));
    };
    let (path, query) = match req.target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (req.target.as_str(), None),
    };
    if websocket::is_upgrade(req) {
        return upgrade(shared, req, via, path);
    }
    if req.method != "GET" && req.method != "HEAD" {
        let mut r = plain(405, "Method Not Allowed");
        r.headers.add("Allow", "GET, HEAD");
        return Reply::Response(r);
    }
    let search = query.map(|q| format!("?{q}")).unwrap_or_default();
    if path == "/party/client.js" {
        let bytes = crate::CLIENT_JS.as_bytes();
        return Reply::Response(file(
            shared,
            req,
            bytes,
            client_etag(),
            "text/javascript; charset=utf-8",
        ));
    }
    if path.starts_with("/party/") {
        return Reply::Response(plain(404, "Not Found"));
    }
    if path == "/screen" {
        return Reply::Response(redirect(format!("/screen/{search}")));
    }
    let (assets, rel) = match path.strip_prefix("/screen") {
        Some(rest) if rest.starts_with('/') => (shared.screen, rest),
        _ => (shared.phone, path),
    };
    let Some(assets) = assets else {
        return Reply::Response(plain(404, "Not Found"));
    };
    Reply::Response(match lookup(assets, rel) {
        Lookup::File { bytes, etag, mime } => file(shared, req, &bytes, &etag, mime),
        Lookup::Redirect => redirect(format!("{path}/{search}")),
        Lookup::NotFound => plain(404, "Not Found"),
        Lookup::BadRequest => plain(400, "Bad Request"),
    })
}

fn client_etag() -> &'static str {
    static ETAG: OnceLock<String> = OnceLock::new();
    ETAG.get_or_init(|| crate::assets::etag_of(crate::CLIENT_JS.as_bytes()))
}

fn file(shared: &Shared, req: &Request, bytes: &[u8], etag: &str, mime: &str) -> Response {
    let not_modified = req.headers.get("if-none-match").map(str::trim) == Some(etag);
    let mut r = if not_modified {
        Response::new(Version::Http11, 304, "Not Modified")
    } else {
        Response::new(Version::Http11, 200, "OK")
    };
    r.headers
        .add("Content-Type", mime)
        .add("Cache-Control", "no-cache")
        .add("ETag", etag)
        .add("X-Content-Type-Options", "nosniff")
        .add("Referrer-Policy", "no-referrer");
    if mime.starts_with("text/html") {
        if let Some(csp) = &shared.csp {
            r.headers.add("Content-Security-Policy", csp.as_str());
        }
    }
    if !not_modified {
        r.body = bytes.to_vec();
    }
    r
}

/// Refuses an upgrade and closes the connection (as `@tvpipe/party`).
fn refuse(mut resp: Response) -> Reply {
    resp.headers.set("Connection", "close");
    Reply::Upgrade(resp, Box::new(|_, _| {}))
}

fn upgrade(shared: &Arc<Shared>, req: &Request, via: Via, path: &str) -> Reply {
    if path != "/party/ws" {
        return refuse(plain(404, "Not Found"));
    }
    if !shared.origin_ok(req) {
        let origin = req.headers.get("origin").unwrap_or("");
        tvpipe::log::write(
            tvpipe::log::Level::Warn,
            "tvpipe_party",
            format_args!("party server: WebSocket from origin {origin:?} refused"),
        );
        return refuse(plain(403, "Forbidden"));
    }
    if shared.sockets.load(Ordering::SeqCst) >= shared.max_sockets {
        return refuse(plain(503, "Service Unavailable"));
    }
    let address = shared.address(req, via);
    {
        let mut joins = shared.joins.lock().unwrap_or_else(|e| e.into_inner());
        if joins.over(&address, Instant::now()) {
            return refuse(plain(429, "Too Many Requests"));
        }
    }
    let resp = match websocket::server_response(req) {
        Ok(r) => r,
        Err(e) if e.kind() == tvpipe::ErrorKind::Unsupported => {
            return refuse(plain(426, "Upgrade Required"))
        }
        Err(_) => return refuse(plain(400, "Bad Request")),
    };
    let slot = conn::Slot::take(&shared.sockets);
    let id = shared.next_conn.fetch_add(1, Ordering::SeqCst) + 1;
    let inbox = shared
        .inbox
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let max_message = shared.max_message;
    Reply::Upgrade(
        resp,
        Box::new(move |tcp, rest| {
            conn::run(tcp, rest, id, address, max_message, inbox, slot);
        }),
    )
}
