//! `tvpipe game play` without a game (docs/game-catalog.md): the window
//! opens on the games list first. A small server of its own on 127.0.0.1
//! (token and cookie as the game's local listener) serves the picker page,
//! the list with its screenshots (fetched by tvpipe, never by the window)
//! and the choice: a game of the list or a source typed in, fetched in the
//! background, asked about in the window when it is new (§18), then handed
//! to [`super::run`], which starts the game's own server and tells the
//! page where to go.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::browser::fonts;
use crate::codec::json::Value;
use crate::crypto::ring::ct_eq;
use crate::error::{Error, Result};
use crate::events::{Event, Events, Progress};
use crate::net::fetch::Url;
use crate::net::http::{Request, Response};
use crate::net::httpd::{self, Reply};

use super::bundle::{self, clean_path, percent_decode, Bundle};
use super::check;
use super::remote::{FetchOptions, Fetched, Fetcher};
use super::server::cookie_name;
use super::source::{self, Source};
use super::web;

/// Makes a fetcher whose progress goes to these events.
pub type MakeFetcher = Arc<dyn Fn(Events) -> Result<Fetcher> + Send + Sync>;

/// The picker's page.
pub const PAGE: &str = "/_tvpipe/picker/";

/// Where the choice is.
enum Job {
    Idle,
    Fetching {
        source: String,
        done: u64,
        total: Option<u64>,
    },
    /// A game nobody said yes to yet: the question in the window.
    Ask {
        fetcher: Box<Fetcher>,
        fetched: Box<Fetched>,
    },
    /// Chosen: the bundle until [`Picker::take`] takes it.
    Chosen {
        title: String,
        bundle: Option<Box<Bundle>>,
    },
    /// The game's server runs: the page goes there.
    Ready {
        url: String,
    },
    Failed {
        source: String,
        error: String,
    },
}

/// The games list as the page gets it, and the screenshots' URLs.
struct Catalog {
    value: Value,
    pictures: Vec<Url>,
}

struct St {
    job: Job,
    /// Counts choices: a finished fetch of an older one is dropped.
    choice: u64,
    catalog: Option<Arc<Catalog>>,
}

struct Shared {
    token: String,
    port: OnceLock<u16>,
    /// `--yes`: no question.
    yes: bool,
    make: MakeFetcher,
    st: Mutex<St>,
}

/// The running picker.
pub(crate) struct Picker {
    shared: Arc<Shared>,
    _server: httpd::Server,
}

impl Picker {
    /// Binds 127.0.0.1:`port` (0: any) with the real fetcher.
    pub(crate) fn start(
        fetch: FetchOptions,
        yes: bool,
        port: u16,
        stop: crate::util::stop::Stop,
    ) -> Result<Picker> {
        let make: MakeFetcher =
            Arc::new(move |events| Fetcher::from_env(fetch, events, Some(stop.clone())));
        Picker::start_with(make, yes, port)
    }

    pub(crate) fn start_with(make: MakeFetcher, yes: bool, port: u16) -> Result<Picker> {
        let token = crate::util::hex::encode(&crate::crypto::ring::random_array::<16>()?);
        let shared = Arc::new(Shared {
            token,
            port: OnceLock::new(),
            yes,
            make,
            st: Mutex::new(St {
                job: Job::Idle,
                choice: 0,
                catalog: None,
            }),
        });
        let s = shared.clone();
        let handler: httpd::Handler = Arc::new(move |req: &Request| Reply::Response(s.handle(req)));
        let config = httpd::Config {
            max_connections: 64,
            max_body: 16 * 1024,
            lan: false,
        };
        let server = httpd::Server::bind_with(
            std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            handler,
            config,
        )?;
        let _ = shared.port.set(server.local_addr().port());
        Ok(Picker {
            shared,
            _server: server,
        })
    }

    /// The window's first URL (it sets the cookie).
    pub(crate) fn start_url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/_tvpipe/start?t={}",
            self.shared.port(),
            self.shared.token
        )
    }

    /// The chosen game, once (then the page shows "starting").
    pub(crate) fn take(&self) -> Option<Bundle> {
        let mut st = self.shared.lock();
        match &mut st.job {
            Job::Chosen { bundle, .. } => bundle.take().map(|b| *b),
            _ => None,
        }
    }

    /// The game's server runs at `url` (its start URL): the page goes
    /// there.
    pub(crate) fn started(&self, url: String) {
        self.shared.lock().job = Job::Ready { url };
    }

    /// Starting the chosen game failed: the page shows it.
    pub(crate) fn failed(&self, source: &str, error: &Error) {
        self.shared.lock().job = Job::Failed {
            source: source.to_owned(),
            error: error.to_string(),
        };
    }
}

fn json(status: u16, v: &Value) -> Response {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Error",
    };
    let mut r = httpd::response(
        status,
        reason,
        "application/json; charset=utf-8",
        v.to_json().into_bytes(),
    );
    r.headers.set("Cache-Control", "no-store");
    r
}

fn error_json(status: u16, msg: &str) -> Response {
    json(status, &Value::object().with("error", msg))
}

fn redirect(location: &str) -> Response {
    let mut r = httpd::response(302, "Found", "text/plain; charset=utf-8", Vec::new());
    r.headers.add("Location", location);
    r
}

fn file(rel: &str, bytes: Vec<u8>) -> Response {
    let mut r = httpd::response(200, "OK", bundle::mime(rel), bytes);
    r.headers
        .set("Cache-Control", "no-cache")
        .add("Referrer-Policy", "no-referrer");
    r
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

/// The games list's entries the page can show: a name, a title and a
/// source each; screenshots resolved against the list's URL (`https://`
/// only).
fn catalog_of(list: &Value, base: &Url, fetcher: &Fetcher) -> Catalog {
    let mut pictures = Vec::new();
    let mut games = Vec::new();
    for g in list.get("games").and_then(Value::as_array).unwrap_or(&[]) {
        let s = |k: &str| g.get(k).and_then(Value::as_str);
        let (Some(name), Some(title), Some(src)) = (s("name"), s("title"), s("source")) else {
            continue;
        };
        if !source::is_name(name) {
            continue;
        }
        let mut shots = Vec::new();
        for p in g
            .get("screenshots")
            .and_then(Value::as_array)
            .unwrap_or(&[])
        {
            let Some(url) = p
                .get("src")
                .and_then(Value::as_str)
                .and_then(|src| base.join(src).ok())
                .filter(|u| u.https || u.is_loopback())
            else {
                continue;
            };
            let kind = match p.get("kind").and_then(Value::as_str) {
                Some("phone") => "phone",
                _ => "tv",
            };
            shots.push(
                Value::object()
                    .with("src", format!("/_tvpipe/catalog/{}", pictures.len()))
                    .with("alt", p.get("alt").and_then(Value::as_str).unwrap_or(""))
                    .with("kind", kind),
            );
            pictures.push(url);
        }
        let players = g.get("players");
        let num = |k: &str| players.and_then(|p| p.get(k)).and_then(Value::as_u64);
        let languages: Vec<Value> = g
            .get("languages")
            .and_then(Value::as_array)
            .unwrap_or(&[])
            .iter()
            .filter_map(Value::as_str)
            .map(Value::from)
            .collect();
        games.push(
            Value::object()
                .with("name", name)
                .with("title", title)
                .with("summary", s("summary").unwrap_or(""))
                .with_opt("min", num("min"))
                .with_opt("max", num("max"))
                .with("languages", Value::Array(languages))
                .with_opt("home", s("home").filter(|h| h.starts_with("https://")))
                .with("downloaded", fetcher.downloaded(src))
                .with("screenshots", Value::Array(shots)),
        );
    }
    Catalog {
        value: Value::object().with("games", Value::Array(games)),
        pictures,
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, St> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn port(&self) -> u16 {
        self.port.get().copied().unwrap_or(0)
    }

    fn host_ok(&self, req: &Request) -> bool {
        let port = self.port();
        req.headers.get("host").is_some_and(|h| {
            let h = h.trim().to_ascii_lowercase();
            [
                format!("127.0.0.1:{port}"),
                format!("localhost:{port}"),
                format!("[::1]:{port}"),
            ]
            .contains(&h)
        })
    }

    fn token_ok(&self, req: &Request, query: &str) -> bool {
        let want = self.token.as_bytes();
        let name = cookie_name(self.port());
        query_param(query, "t").is_some_and(|t| ct_eq(t.as_bytes(), want))
            || req.headers.get_all("cookie").any(|h| {
                h.split(';').any(|kv| {
                    kv.trim()
                        .split_once('=')
                        .is_some_and(|(k, v)| k == name && ct_eq(v.as_bytes(), want))
                })
            })
    }

    /// A POST from this origin only (a page elsewhere has no cookie to
    /// send anyway: `SameSite=Strict`).
    fn origin_ok(&self, req: &Request) -> bool {
        let port = self.port();
        req.headers.get("origin").is_none_or(|o| {
            let o = o.trim().to_ascii_lowercase();
            o == format!("http://127.0.0.1:{port}") || o == format!("http://localhost:{port}")
        })
    }

    fn handle(self: &Arc<Shared>, req: &Request) -> Response {
        if !self.host_ok(req) {
            return httpd::error_response(421, "Misdirected Request");
        }
        let (path, query) = req.target.split_once('?').unwrap_or((&req.target, ""));
        let post = req.method == "POST";
        if !post && req.method != "GET" && req.method != "HEAD" {
            let mut r = httpd::error_response(405, "Method Not Allowed");
            r.headers.add("Allow", "GET, HEAD, POST");
            return r;
        }
        if post && path != "/_tvpipe/api/game" {
            return httpd::error_response(405, "Method Not Allowed");
        }
        match path {
            "/" => {
                return if self.token_ok(req, "") {
                    redirect(PAGE)
                } else {
                    httpd::error_response(403, "Forbidden: open the link tvpipe printed")
                }
            }
            "/_tvpipe/start" => {
                if !self.token_ok(req, query) {
                    return httpd::error_response(403, "Forbidden");
                }
                let mut r = redirect(PAGE);
                r.headers.add(
                    "Set-Cookie",
                    format!(
                        "{}={}; Path=/; HttpOnly; SameSite=Strict",
                        cookie_name(self.port()),
                        self.token
                    ),
                );
                return r;
            }
            "/_tvpipe/ui.css" => return file("ui.css", fonts::UI_CSS.as_bytes().to_vec()),
            _ => {}
        }
        if let Some(name) = path.strip_prefix("/_tvpipe/fonts/") {
            return fonts::font_response(fonts::font(name));
        }
        if let Some(rest) = path.strip_prefix("/_tvpipe/picker/") {
            let rel = if rest.is_empty() {
                "index.html".to_owned()
            } else {
                rest.to_owned()
            };
            let Some(rel) = percent_decode(&rel).and_then(|r| clean_path(&r)) else {
                return httpd::error_response(404, "Not Found");
            };
            return match web::get(&format!("picker/{rel}")) {
                Some(b) => file(&rel, b.into_owned()),
                None => httpd::error_response(404, "Not Found"),
            };
        }
        if !self.token_ok(req, "") {
            return httpd::error_response(403, "Forbidden");
        }
        if path == "/_tvpipe/api/catalog" {
            return match self.catalog() {
                Ok(c) => json(200, &c.value),
                Err(e) => json(
                    200,
                    &Value::object()
                        .with("games", Value::array(Vec::<Value>::new()))
                        .with("error", e.to_string()),
                ),
            };
        }
        if let Some(n) = path.strip_prefix("/_tvpipe/catalog/") {
            return self.picture(n);
        }
        if path == "/_tvpipe/api/game" {
            if !post {
                return json(200, &self.state());
            }
            if !self.origin_ok(req) {
                return httpd::error_response(403, "Forbidden");
            }
            let body = match Value::parse(&String::from_utf8_lossy(&req.body)) {
                Ok(v) => v,
                Err(_) => return error_json(400, "a JSON object"),
            };
            return match self.post(&body) {
                Ok(()) => json(200, &self.state()),
                Err((status, msg)) => error_json(status, &msg),
            };
        }
        httpd::error_response(404, "Not Found")
    }

    fn catalog(&self) -> Result<Arc<Catalog>> {
        if let Some(c) = &self.lock().catalog {
            return Ok(c.clone());
        }
        let fetcher = (self.make)(Events::none())?;
        let list = fetcher.catalog()?;
        let c = Arc::new(catalog_of(&list, &fetcher.list_url()?, &fetcher));
        self.lock().catalog = Some(c.clone());
        Ok(c)
    }

    fn picture(&self, n: &str) -> Response {
        let url = self.lock().catalog.as_ref().and_then(|c| {
            n.parse::<usize>()
                .ok()
                .and_then(|i| c.pictures.get(i).cloned())
        });
        let Some(url) = url else {
            return httpd::error_response(404, "Not Found");
        };
        let got = (self.make)(Events::none()).and_then(|f| f.picture(&url));
        match got {
            Ok((bytes, mime)) => {
                let mut r = httpd::response(200, "OK", mime, bytes);
                r.headers.set("Cache-Control", "private, max-age=3600");
                r
            }
            Err(e) => {
                debug!("picker: {url}: {e}");
                httpd::error_response(404, "Not Found")
            }
        }
    }

    /// The choice's state for the page.
    fn state(&self) -> Value {
        let st = self.lock();
        match &st.job {
            Job::Idle => Value::object().with("state", "idle"),
            Job::Fetching {
                source,
                done,
                total,
            } => Value::object()
                .with("state", "fetching")
                .with("source", source.as_str())
                .with("done", *done)
                .with_opt("total", *total),
            Job::Ask { fetched: f, .. } => {
                let m = &f.bundle.manifest;
                let (files, unpacked) = f.bundle.zip_stats().unwrap_or((0, 0));
                Value::object().with("state", "ask").with(
                    "game",
                    Value::object()
                        .with("name", m.name.as_str())
                        .with("title", m.title.as_str())
                        .with("from", f.shown.as_str())
                        .with_opt("release", f.tag.as_deref())
                        .with("size", check::human(f.size))
                        .with("files", files)
                        .with("unpacked", check::human(unpacked))
                        .with_opt("tvpipe", m.tvpipe.as_deref())
                        .with("sha256", f.sha256.as_str())
                        .with("changed", f.changed),
                )
            }
            Job::Chosen { title, .. } => Value::object()
                .with("state", "starting")
                .with("title", title.as_str()),
            Job::Ready { url } => Value::object()
                .with("state", "ready")
                .with("url", url.as_str()),
            Job::Failed { source, error } => Value::object()
                .with("state", "failed")
                .with("source", source.as_str())
                .with("error", error.as_str()),
        }
    }

    /// `{"source": S}` (choose), `{"trust": true}` (yes to the question),
    /// `{"back": true}` (no, or after an error: the list again).
    fn post(self: &Arc<Shared>, body: &Value) -> std::result::Result<(), (u16, String)> {
        let busy = |st: &St| {
            matches!(
                st.job,
                Job::Fetching { .. } | Job::Chosen { .. } | Job::Ready { .. }
            )
        };
        if let Some(s) = body.get("source").and_then(Value::as_str) {
            let src = Source::parse_remote(s).map_err(|e| (400, e.to_string()))?;
            let mut st = self.lock();
            if busy(&st) {
                return Err((409, "a game is being fetched or started".into()));
            }
            st.choice += 1;
            let choice = st.choice;
            st.job = Job::Fetching {
                source: s.trim().to_owned(),
                done: 0,
                total: None,
            };
            drop(st);
            let me = self.clone();
            let shown = s.trim().to_owned();
            std::thread::Builder::new()
                .name("picker-fetch".into())
                .spawn(move || me.fetch(choice, &shown, &src))
                .map_err(|e| (500, format!("spawn: {e}")))?;
            return Ok(());
        }
        if body.get("trust").and_then(Value::as_bool) == Some(true) {
            let mut st = self.lock();
            let job = std::mem::replace(&mut st.job, Job::Idle);
            let Job::Ask { fetcher, fetched } = job else {
                st.job = job;
                return Err((409, "no question to answer".into()));
            };
            if let Err(e) = fetcher.trust(&fetched) {
                warn!("picker: remembering the answer: {e}");
            }
            st.job = chosen(*fetched);
            return Ok(());
        }
        if body.get("back").and_then(Value::as_bool) == Some(true) {
            let mut st = self.lock();
            if busy(&st) {
                return Err((409, "a game is being fetched or started".into()));
            }
            st.choice += 1;
            st.job = Job::Idle;
            return Ok(());
        }
        Err((400, "expected source, trust or back".into()))
    }

    /// Fetches a choice (on its own thread).
    fn fetch(self: Arc<Shared>, choice: u64, shown: &str, src: &Source) {
        let me = Arc::downgrade(&self);
        let events = Events::to(move |e| {
            if let Event::Progress(Progress::Download { done, total }) = e {
                if let Some(me) = me.upgrade() {
                    let mut st = me.lock();
                    if st.choice == choice {
                        if let Job::Fetching {
                            done: d, total: t, ..
                        } = &mut st.job
                        {
                            *d = done;
                            *t = total;
                        }
                    }
                }
            }
        });
        let res = (self.make)(events).and_then(|fetcher| {
            let f = fetcher.fetch(src)?;
            Ok((fetcher, f))
        });
        let mut st = self.lock();
        if st.choice != choice {
            return;
        }
        st.job = match res {
            Ok((_, f)) if f.pinned || f.trusted || self.yes => chosen(f),
            Ok((fetcher, f)) => Job::Ask {
                fetcher: Box::new(fetcher),
                fetched: Box::new(f),
            },
            Err(e) => Job::Failed {
                source: shown.to_owned(),
                error: e.to_string(),
            },
        };
    }
}

fn chosen(f: Fetched) -> Job {
    let title = f.bundle.manifest.title.clone();
    info!("playshare: picked {} from {}", title, f.shown);
    Job::Chosen {
        title,
        bundle: Some(Box::new(f.bundle)),
    }
}

#[cfg(test)]
mod tests;
