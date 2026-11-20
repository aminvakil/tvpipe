//! Games from the internet (docs/playshare-design.md §18): resolving a
//! source (GitHub's releases, the games list), downloading it into the
//! cache, and what `game play` asks before running it.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::bundle::Bundle;
use super::cache::{self, Cache, Index, Record};
use super::check;
use super::source::{GitHub, Remote, Source, Tag, Where};
use crate::codec::json::Value;
use crate::crypto::ring::Sha256;
use crate::error::{Error, ErrorKind, Result};
use crate::events::{Events, Progress, Step};
use crate::net::fetch::{Client, Get, Reply, Url};
use crate::util::hex;
use crate::util::stop::Stop;

/// A game zip at most (the bundle's own limit, unpacked).
pub const MAX_GAME: usize = 256 << 20;
/// The games list and API answers at most.
const MAX_JSON: usize = 16 << 20;
/// A screenshot of the games list at most.
pub const MAX_PICTURE: usize = 2 << 20;
/// Where the games list is.
pub const GAMES_LIST: &str = "https://tvpipe.party/games.json";

/// The servers: GitHub's web and API, the games list; overridable for
/// tests (`TVPIPE_GITHUB_URL`, `TVPIPE_GITHUB_API`, `TVPIPE_GAMES_URL`).
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub web: String,
    pub api: String,
    pub list: String,
    /// `GITHUB_TOKEN` or `GH_TOKEN`: sent to the API's host only.
    pub token: Option<String>,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

impl Endpoints {
    pub fn from_env() -> Endpoints {
        Endpoints {
            web: env("TVPIPE_GITHUB_URL").unwrap_or_else(|| "https://github.com".into()),
            api: env("TVPIPE_GITHUB_API").unwrap_or_else(|| "https://api.github.com".into()),
            list: env("TVPIPE_GAMES_URL").unwrap_or_else(|| GAMES_LIST.into()),
            token: env("GITHUB_TOKEN").or_else(|| env("GH_TOKEN")),
        }
    }
}

/// `--refresh`, `--offline`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FetchOptions {
    /// Ask the server even for a tag that was fetched before.
    pub refresh: bool,
    /// Only the cache.
    pub offline: bool,
}

/// A remote game, downloaded (or found in the cache).
#[derive(Debug)]
pub struct Fetched {
    /// The source without a pin (`Remote::key`).
    pub key: String,
    /// What to show: the source, or the games list's name and its source.
    pub shown: String,
    /// The zip in the cache.
    pub path: PathBuf,
    /// Hex.
    pub sha256: String,
    pub size: u64,
    pub tag: Option<String>,
    /// The game, opened (named by its source).
    pub bundle: Bundle,
    /// The source's pin matched.
    pub pinned: bool,
    /// The person said yes to this hash before.
    pub trusted: bool,
    /// They said yes to another hash of this source before.
    pub changed: bool,
    /// The source with this hash pinned (for `--yes`-less scripts).
    pub pin_hint: String,
}

impl Fetched {
    /// The question before running it.
    pub fn question(&self) -> String {
        let m = &self.bundle.manifest;
        let (files, unpacked) = self.bundle.zip_stats().unwrap_or((0, 0));
        let mut q = String::new();
        if self.changed {
            q.push_str("This game changed since you last said yes to it.\n  ");
        }
        q.push_str(&format!("{} (\"{}\") from {}", m.name, m.title, self.shown));
        if let Some(t) = &self.tag {
            q.push_str(&format!(", release {t}"));
        }
        q.push_str(&format!(
            "\n  {}, {files} files ({} unpacked), made for tvpipe {}\n  sha256 {}\n  \
             It runs in tvpipe's window, can cast to the TVs on this network and serves \
             pages to phones.\n  Play it? [y/N]",
            check::human(self.size),
            check::human(unpacked),
            m.tvpipe.as_deref().unwrap_or("(any)"),
            self.sha256
        ));
        q
    }
}

/// Fetches games into the cache.
pub struct Fetcher {
    pub client: Client,
    pub endpoints: Endpoints,
    pub cache: Cache,
    pub opts: FetchOptions,
    pub events: Events,
}

/// One release from GitHub's API.
#[derive(Debug, Clone)]
struct Release {
    tag: String,
    body: String,
    assets: Vec<(String, Option<[u8; 32]>)>,
}

fn digest(v: &Value) -> Option<[u8; 32]> {
    let h = v.get("digest")?.as_str()?.strip_prefix("sha256:")?;
    let b = hex::decode(h).ok()?;
    b.try_into().ok()
}

impl Release {
    fn from_value(v: &Value) -> Option<Release> {
        Some(Release {
            tag: v.get("tag_name")?.as_str()?.to_owned(),
            body: v
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            assets: v
                .get("assets")
                .and_then(Value::as_array)
                .unwrap_or(&[])
                .iter()
                .filter_map(|a| Some((a.get("name")?.as_str()?.to_owned(), digest(a))))
                .collect(),
        })
    }

    /// `<!-- tvpipe: X.Y -->` in the notes: the series the game is made
    /// for.
    fn series(&self) -> Option<&str> {
        let i = self.body.find("<!-- tvpipe:")?;
        let rest = &self.body[i + "<!-- tvpipe:".len()..];
        let j = rest.find("-->")?;
        Some(rest[..j].trim()).filter(|s| !s.is_empty())
    }

    /// The asset: the named one, else the only zip, else `REPO.zip`.
    fn pick(&self, g: &GitHub) -> Result<(String, Option<[u8; 32]>)> {
        let zips: Vec<&(String, Option<[u8; 32]>)> = self
            .assets
            .iter()
            .filter(|(n, _)| n.to_ascii_lowercase().ends_with(".zip"))
            .collect();
        let found = match &g.asset {
            Some(a) => self.assets.iter().find(|(n, _)| n == a),
            None if zips.len() == 1 => zips.first().copied(),
            None => {
                let want = format!("{}.zip", g.repo);
                zips.iter().copied().find(|(n, _)| *n == want)
            }
        };
        found.cloned().ok_or_else(|| {
            let names: Vec<&str> = zips.iter().map(|(n, _)| n.as_str()).collect();
            let has = if names.is_empty() {
                "no zips".to_owned()
            } else {
                names.join(", ")
            };
            Error::not_found(format!(
                "{g}: release {} has {has}{}",
                self.tag,
                if g.asset.is_none() && names.len() > 1 {
                    " (name one: gh:OWNER/REPO/ASSET)"
                } else {
                    ""
                }
            ))
        })
    }
}

/// A picture's type by its first bytes: PNG, JPEG or WebP.
pub fn image_type(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// The version after a tag prefix, for ordering (`0.10.1` after `0.9`).
fn version_key(rest: &str) -> Vec<u64> {
    rest.trim_start_matches('v')
        .split(['.', '-', '+'])
        .map_while(|p| p.parse().ok())
        .collect()
}

/// Percent-encodes a path segment.
fn seg(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Whether an error is the network's (the cache may stand in): not a
/// server's answer, not a stop request.
fn offline_kind(e: &Error) -> bool {
    let m = e.message();
    (matches!(
        e.kind(),
        ErrorKind::Io | ErrorKind::Timeout | ErrorKind::Closed
    ) || m.contains("no address"))
        && !m.contains("stopped")
}

/// The source a download resolved to.
struct Resolved {
    url: Url,
    tag: Option<String>,
    digest: Option<[u8; 32]>,
    /// The URL names one release's asset: its bytes do not change (the
    /// cached copy is used unless `--refresh`).
    fixed: bool,
}

impl Fetcher {
    /// For the CLI: the real servers, the platform's cache.
    pub fn from_env(opts: FetchOptions, events: Events, stop: Option<Stop>) -> Result<Fetcher> {
        Ok(Fetcher {
            client: Client::from_env(stop),
            endpoints: Endpoints::from_env(),
            cache: Cache::open()?,
            opts,
            events,
        })
    }

    fn api(&self, path: &str, what: &str) -> Result<Value> {
        let url = Url::parse(&format!(
            "{}{path}",
            self.endpoints.api.trim_end_matches('/')
        ))?;
        let mut get = Get::new(url.clone(), MAX_JSON)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(t) = &self.endpoints.token {
            get.auth = Some((url.host.clone(), format!("Bearer {t}")));
        }
        let (reply, body) = self.client.get_bytes(&get)?;
        if !reply.is_success() {
            return Err(api_error(&reply, what));
        }
        Value::parse(&String::from_utf8_lossy(&body))
            .map_err(|e| Error::protocol(format!("{what}: GitHub's answer: {e}")))
    }

    fn resolve(&self, r: &Remote) -> Result<Resolved> {
        let g = match &r.at {
            Where::Url(u) => {
                return Ok(Resolved {
                    url: u.clone(),
                    tag: None,
                    digest: None,
                    fixed: false,
                })
            }
            Where::GitHub(g) => g,
        };
        let web = self.endpoints.web.trim_end_matches('/');
        let base = format!("{web}/{}/{}/releases", seg(&g.owner), seg(&g.repo));
        match (&g.tag, &g.asset) {
            (Tag::Latest, Some(a)) => {
                return Ok(Resolved {
                    url: Url::parse(&format!("{base}/latest/download/{}", seg(a)))?,
                    tag: None,
                    digest: None,
                    fixed: false,
                })
            }
            (Tag::Exact(t), Some(a)) => {
                return Ok(Resolved {
                    url: Url::parse(&format!("{base}/download/{}/{}", seg(t), seg(a)))?,
                    tag: Some(t.clone()),
                    digest: None,
                    fixed: true,
                })
            }
            _ => {}
        }
        let repo = format!("/repos/{}/{}", seg(&g.owner), seg(&g.repo));
        let release = match &g.tag {
            Tag::Latest => self.release(&format!("{repo}/releases/latest"), g)?,
            Tag::Exact(t) => self.release(&format!("{repo}/releases/tags/{}", seg(t)), g)?,
            Tag::Prefix(p) => self.newest(&repo, p, g)?,
        };
        let (asset, digest) = release.pick(g)?;
        Ok(Resolved {
            url: Url::parse(&format!(
                "{base}/download/{}/{}",
                seg(&release.tag),
                seg(&asset)
            ))?,
            tag: Some(release.tag),
            digest,
            fixed: true,
        })
    }

    fn release(&self, path: &str, g: &GitHub) -> Result<Release> {
        let v = self.api(path, &g.to_string())?;
        Release::from_value(&v)
            .ok_or_else(|| Error::protocol(format!("{g}: GitHub's answer has no release")))
    }

    /// The newest release whose tag starts with `prefix` and fits this
    /// tvpipe (its notes' marker; no marker counts as fitting).
    fn newest(&self, repo: &str, prefix: &str, g: &GitHub) -> Result<Release> {
        let v = self.api(&format!("{repo}/releases?per_page=100"), &g.to_string())?;
        let mut list: Vec<(Vec<u64>, Release)> = v
            .as_array()
            .unwrap_or(&[])
            .iter()
            .filter(|r| {
                !r.get("draft").and_then(Value::as_bool).unwrap_or(false)
                    && !r
                        .get("prerelease")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
            })
            .filter_map(Release::from_value)
            .filter_map(|r| {
                let rest = r.tag.strip_prefix(prefix)?;
                Some((version_key(rest), r))
            })
            .collect();
        // Newest first; the API's order (newest created first) breaks ties.
        list.sort_by(|a, b| b.0.cmp(&a.0));
        let ours = env!("CARGO_PKG_VERSION");
        let mut skipped = None;
        for (_, r) in list {
            match r.series() {
                Some(s) if check::version_compatible(s, ours).is_err() => {
                    if skipped.is_none() {
                        skipped = Some((r.tag.clone(), s.to_owned()));
                    }
                }
                _ => return Ok(r),
            }
        }
        Err(match skipped {
            Some((tag, series)) => Error::not_found(format!(
                "{g}: no release fits tvpipe {} (the newest, {tag}, is made for tvpipe {series})",
                check::short_version(ours)
            )),
            None => Error::not_found(format!(
                "{g}: no release has a tag starting with {prefix:?}"
            )),
        })
    }

    /// The games list: fetched again (its ETag) unless offline; the
    /// cached copy when the network fails.
    fn list(&self, idx: &mut Index) -> Result<Value> {
        let url = self.endpoints.list.clone();
        let cached = self
            .cache
            .read("list.json")
            .filter(|_| idx.list.as_ref().is_some_and(|(u, _)| *u == url));
        let parse = |b: &[u8]| {
            Value::parse(&String::from_utf8_lossy(b))
                .map_err(|e| Error::protocol(format!("the games list ({url}): {e}")))
        };
        if self.opts.offline {
            return match &cached {
                Some(b) => parse(b),
                None => Err(Error::not_found(
                    "the games list is not in the cache (--offline)",
                )),
            };
        }
        let mut get = Get::new(Url::parse(&url)?, MAX_JSON);
        if let (Some(_), Some((_, Some(etag)))) = (&cached, &idx.list) {
            get = get.header("If-None-Match", etag.clone());
        }
        match self.client.get_bytes(&get) {
            Ok((reply, _)) if reply.status == 304 && cached.is_some() => {
                parse(cached.as_deref().unwrap_or_default())
            }
            Ok((reply, body)) if reply.is_success() => {
                let v = parse(&body)?;
                self.cache.write("list.json", &body)?;
                idx.list = Some((url.clone(), reply.headers.get("etag").map(str::to_owned)));
                Ok(v)
            }
            Ok((reply, _)) => Err(Error::not_found(format!(
                "the games list ({url}): {} from the server",
                reply.status
            ))),
            Err(e) => match &cached {
                Some(b) if offline_kind(&e) => {
                    warn!("the games list: {e}; using the copy from before");
                    parse(b)
                }
                _ => Err(e),
            },
        }
    }

    /// The games list for the picker (docs/game-catalog.md): fetched
    /// again (its ETag) unless offline, the cached copy when the network
    /// fails.
    pub fn catalog(&self) -> Result<Value> {
        let mut idx = self.cache.index();
        let list = self.list(&mut idx)?;
        self.cache.save(&idx)?;
        Ok(list)
    }

    /// The list's URL (screenshots' `src` are relative to it).
    pub fn list_url(&self) -> Result<Url> {
        Url::parse(&self.endpoints.list)
    }

    /// Whether this source was downloaded before (its zip is in the
    /// cache).
    pub fn downloaded(&self, source: &str) -> bool {
        let Ok(r) = Remote::parse(source) else {
            return false;
        };
        self.cache
            .index()
            .sources
            .get(&r.key())
            .is_some_and(|rec| self.cache.blob(&rec.sha256).is_some())
    }

    /// A screenshot of the games list (`https://`, at most
    /// [`MAX_PICTURE`], PNG, JPEG or WebP), kept in the cache
    /// (`pics/`) and asked again with its ETag; the cached copy offline or
    /// when the network fails.
    pub fn picture(&self, url: &Url) -> Result<(Vec<u8>, &'static str)> {
        let key = hex::encode(&crate::crypto::ring::sha256(url.to_string().as_bytes()));
        let file = format!("pics/{key}");
        let etag_file = format!("pics/{key}.etag");
        let cached = self.cache.read(&file).filter(|b| image_type(b).is_some());
        let from_cache = |b: Vec<u8>| {
            let t = image_type(&b).unwrap_or("application/octet-stream");
            (b, t)
        };
        if self.opts.offline {
            return cached
                .map(from_cache)
                .ok_or_else(|| Error::not_found(format!("{url}: not in the cache (--offline)")));
        }
        let mut get = Get::new(url.clone(), MAX_PICTURE);
        if cached.is_some() {
            if let Some(e) = self.cache.read(&etag_file) {
                get = get.header("If-None-Match", String::from_utf8_lossy(&e).into_owned());
            }
        }
        match self.client.get_bytes(&get) {
            Ok((reply, _)) if reply.status == 304 && cached.is_some() => {
                Ok(from_cache(cached.unwrap_or_default()))
            }
            Ok((reply, body)) if reply.is_success() => {
                let t = image_type(&body).ok_or_else(|| {
                    Error::protocol(format!("{url}: not a PNG, JPEG or WebP picture"))
                })?;
                self.cache.write(&file, &body)?;
                match reply.headers.get("etag") {
                    Some(e) => self.cache.write(&etag_file, e.as_bytes())?,
                    None => {
                        let _ = std::fs::remove_file(self.cache.dir().join(&etag_file));
                    }
                }
                Ok((body, t))
            }
            Ok((reply, _)) => match cached {
                Some(b) => Ok(from_cache(b)),
                None => Err(Error::not_found(format!(
                    "{url}: {} from the server",
                    reply.status
                ))),
            },
            Err(e) => match cached {
                Some(b) if offline_kind(&e) => Ok(from_cache(b)),
                _ => Err(e),
            },
        }
    }

    /// A game: a name from the games list, or a remote source.
    pub fn fetch(&self, src: &Source) -> Result<Fetched> {
        match src {
            Source::Remote(r) => self.fetch_remote(r, None),
            Source::Name(n) => {
                let mut idx = self.cache.index();
                let list = self.list(&mut idx)?;
                self.cache.save(&idx)?;
                let games = list.get("games").and_then(Value::as_array).unwrap_or(&[]);
                let entry = games
                    .iter()
                    .find(|g| g.get("name").and_then(Value::as_str) == Some(n.as_str()));
                let Some(entry) = entry else {
                    let names: Vec<&str> = games
                        .iter()
                        .filter_map(|g| g.get("name").and_then(Value::as_str))
                        .collect();
                    return Err(Error::not_found(format!(
                        "{n}: no such file or folder, and no game of that name in the games list ({}{})",
                        self.endpoints.list,
                        if names.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", names.join(", "))
                        }
                    )));
                };
                let source = entry
                    .get("source")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::protocol(format!("the games list: {n} has no source")))?;
                let r = Remote::parse(source)
                    .map_err(|e| Error::protocol(format!("the games list: {n}: {e}")))?;
                self.fetch_remote(&r, Some(n))
            }
            Source::Local(p) => Err(Error::usage(format!(
                "{}: a local game, not one to fetch",
                p.display()
            ))),
        }
    }

    fn fetch_remote(&self, r: &Remote, name: Option<&str>) -> Result<Fetched> {
        let key = r.key();
        let shown = match name {
            Some(n) => format!("{key} (\"{n}\" in the games list)"),
            None => key.clone(),
        };
        let mut idx = self.cache.index();
        let rec = idx.sources.get(&key).cloned();
        let cached = rec
            .as_ref()
            .and_then(|rec| Some((rec.clone(), self.cache.blob(&rec.sha256)?)));
        // A pin whose bytes are here: no network at all.
        if let Some(pin) = &r.sha256 {
            let h = hex::encode(pin);
            if let Some(path) = self.cache.blob(&h) {
                let rec = rec.clone().filter(|x| x.sha256 == h);
                return self.finish(r, &key, shown, path, rec, &mut idx);
            }
        }
        if self.opts.offline {
            return match cached {
                Some((rec, path)) => self.finish(r, &key, shown, path, Some(rec), &mut idx),
                None => Err(Error::not_found(format!(
                    "{key}: not in the cache (--offline)"
                ))),
            };
        }
        if !r.moves() && !self.opts.refresh {
            if let Some((rec, path)) = cached {
                return self.finish(r, &key, shown, path, Some(rec), &mut idx);
            }
        }
        let fallback = |e: Error, idx: &mut Index| -> Result<Fetched> {
            match &cached {
                Some((rec, path)) if offline_kind(&e) => {
                    warn!("{key}: {e}; playing the copy from before");
                    self.finish(r, &key, shown.clone(), path.clone(), Some(rec.clone()), idx)
                }
                _ => Err(e),
            }
        };
        let res = match self.resolve(r) {
            Ok(res) => res,
            Err(e) => return fallback(e, &mut idx),
        };
        // The same release's asset as before: its bytes are here.
        if let Some((rec, path)) = &cached {
            if res.fixed && rec.url == res.url.to_string() && !self.opts.refresh {
                return self.finish(r, &key, shown, path.clone(), Some(rec.clone()), &mut idx);
            }
        }
        let etag = cached
            .as_ref()
            .filter(|(rec, _)| rec.url == res.url.to_string())
            .and_then(|(rec, _)| rec.etag.clone());
        let got = match self.download(r, &res, etag.as_deref()) {
            Ok(got) => got,
            Err(e) => return fallback(e, &mut idx),
        };
        let (path, sha, size, etag) = match got {
            Download::NotModified => {
                let Some((rec, path)) = cached else {
                    return Err(Error::protocol(format!("{key}: 304 without a cached copy")));
                };
                (path, rec.sha256.clone(), rec.size, rec.etag.clone())
            }
            Download::New {
                path,
                sha256,
                size,
                etag,
            } => (path, sha256, size, etag),
        };
        let mut new = rec.clone().unwrap_or_default();
        new.url = res.url.to_string();
        new.tag = res.tag.clone();
        new.etag = etag;
        new.sha256 = sha;
        new.size = size;
        new.fetched = cache::now();
        self.finish(r, &key, shown, path, Some(new), &mut idx)
    }

    /// Opens the zip, records it, and says whether it is trusted.
    fn finish(
        &self,
        r: &Remote,
        key: &str,
        shown: String,
        path: PathBuf,
        rec: Option<Record>,
        idx: &mut Index,
    ) -> Result<Fetched> {
        let bytes =
            std::fs::read(&path).map_err(|e| Error::io(format!("read {}", path.display()), e))?;
        let sha = crate::crypto::ring::sha256(&bytes);
        let sha_hex = hex::encode(&sha);
        if let Some(pin) = &r.sha256 {
            if *pin != sha {
                return Err(Error::auth(format!(
                    "{key}: its sha256 is {sha_hex}, not {} as pinned",
                    hex::encode(pin)
                )));
            }
        }
        let bundle = Bundle::open_zip_as(&path, key)?;
        let mut rec = rec.unwrap_or_else(|| Record {
            url: key.to_owned(),
            fetched: cache::now(),
            ..Record::default()
        });
        if rec.sha256 != sha_hex {
            rec.sha256 = sha_hex.clone();
        }
        rec.size = bytes.len() as u64;
        rec.name = bundle.manifest.name.clone();
        rec.title = bundle.manifest.title.clone();
        rec.series = bundle.manifest.tvpipe.clone();
        rec.used = cache::now();
        let trusted = rec.trusted.as_deref() == Some(sha_hex.as_str());
        let changed = rec.trusted.is_some() && !trusted;
        idx.sources.insert(key.to_owned(), rec.clone());
        self.cache.save(idx)?;
        self.cache.prune(idx);
        Ok(Fetched {
            key: key.to_owned(),
            shown,
            path,
            size: rec.size,
            tag: rec.tag.clone(),
            bundle,
            pinned: r.sha256.is_some(),
            trusted,
            changed,
            pin_hint: r.pinned(&sha),
            sha256: sha_hex,
        })
    }

    /// Remembers that the person said yes to this game's hash.
    pub fn trust(&self, f: &Fetched) -> Result<()> {
        let mut idx = self.cache.index();
        if let Some(rec) = idx.sources.get_mut(&f.key) {
            rec.trusted = Some(f.sha256.clone());
            self.cache.save(&idx)?;
        }
        Ok(())
    }

    fn download(&self, r: &Remote, res: &Resolved, etag: Option<&str>) -> Result<Download> {
        let what = res
            .url
            .path()
            .rsplit('/')
            .next()
            .filter(|n| !n.is_empty())
            .unwrap_or("the game")
            .to_owned();
        let step = Step::Download(what.clone());
        self.events.progress(Progress::Begin(step.clone()));
        let out = self.download_inner(r, res, etag);
        match &out {
            Ok(_) => self.events.progress(Progress::Done(step, None)),
            Err(e) => self.events.progress(Progress::Failed(step, e.to_string())),
        }
        out
    }

    fn download_inner(&self, r: &Remote, res: &Resolved, etag: Option<&str>) -> Result<Download> {
        let mut get = Get::new(res.url.clone(), MAX_GAME);
        get.allow_http = r.sha256.is_some();
        if let Some(e) = etag {
            get = get.header("If-None-Match", e);
        }
        let (tmp, mut file) = self.cache.temp()?;
        let mut hash = Sha256::new();
        let mut last = Instant::now();
        let events = &self.events;
        let reply = self.client.get(
            &get,
            &mut |done, total| {
                if last.elapsed() >= Duration::from_millis(100) || Some(done) == total {
                    last = Instant::now();
                    events.progress(Progress::Download { done, total });
                }
            },
            &mut |d| {
                hash.update(d);
                file.write_all(d)
                    .map_err(|e| Error::io(format!("write {}", tmp.display()), e))
            },
        );
        let reply: Reply = match reply {
            Ok(reply) => reply,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        };
        drop(file);
        if reply.status == 304 && etag.is_some() {
            let _ = std::fs::remove_file(&tmp);
            return Ok(Download::NotModified);
        }
        if !reply.is_success() {
            let _ = std::fs::remove_file(&tmp);
            return Err(match reply.status {
                404 => Error::not_found(format!(
                    "{}: not found (404){}",
                    r.key(),
                    match &r.at {
                        Where::GitHub(_) => ": no such repository, release or asset",
                        Where::Url(_) => "",
                    }
                )),
                s => Error::protocol(format!("{}: {s} from {}", r.key(), reply.url.host)),
            });
        }
        let sha = hash.finish();
        if let Some(d) = res.digest {
            if d != sha {
                let _ = std::fs::remove_file(&tmp);
                return Err(Error::auth(format!(
                    "{}: the download's sha256 is {}, GitHub says {}",
                    r.key(),
                    hex::encode(&sha),
                    hex::encode(&d)
                )));
            }
        }
        if let Some(pin) = &r.sha256 {
            if *pin != sha {
                let _ = std::fs::remove_file(&tmp);
                return Err(Error::auth(format!(
                    "{}: its sha256 is {}, not {} as pinned",
                    r.key(),
                    hex::encode(&sha),
                    hex::encode(pin)
                )));
            }
        }
        let size = std::fs::metadata(&tmp).map_or(0, |m| m.len());
        let sha256 = hex::encode(&sha);
        let path = self.cache.keep(&tmp, &sha256)?;
        Ok(Download::New {
            path,
            sha256,
            size,
            etag: reply.headers.get("etag").map(str::to_owned),
        })
    }
}

enum Download {
    NotModified,
    New {
        path: PathBuf,
        sha256: String,
        size: u64,
        etag: Option<String>,
    },
}

fn api_error(reply: &Reply, what: &str) -> Error {
    let msg = Value::parse(&String::from_utf8_lossy(&reply.error_body))
        .ok()
        .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    let limited = reply.headers.get("x-ratelimit-remaining") == Some("0");
    match reply.status {
        403 | 429 if limited || msg.contains("rate limit") => Error::protocol(format!(
            "{what}: GitHub's API allows 60 requests an hour without a token: set GITHUB_TOKEN, \
             or name the asset and the tag (gh:OWNER/REPO/ASSET@TAG), which needs no API"
        )),
        404 => Error::not_found(format!(
            "{what}: no such repository or release on GitHub (a private one needs GITHUB_TOKEN)"
        )),
        s => Error::protocol(format!("{what}: GitHub's API answered {s} {msg}")),
    }
}

#[cfg(test)]
pub(crate) mod tests;
