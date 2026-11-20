//! Fetching games from a fake GitHub (web, API, asset host) and a fake
//! games list, all one local TLS server with the test CA: no network.

use std::sync::{Arc, Mutex};

use super::*;
use crate::net::fetch::tests::{response, TestServer};
use crate::net::http::Request;

pub(crate) const WEB: &str = "web.tvtest.opentunnel.test";
const API: &str = "api.tvtest.opentunnel.test";
const CDN: &str = "cdn.tvtest.opentunnel.test";
pub(crate) const LIST: &str = "list.tvtest.opentunnel.test";

pub(crate) fn series() -> String {
    check::short_version(env!("CARGO_PKG_VERSION"))
}

/// A game zip: `g/` with tvpipe.json (made for `tvpipe`) and its pages.
pub(crate) fn game(title: &str, tvpipe: &str) -> Vec<u8> {
    let manifest = format!(
        r#"{{"name":"g","title":"{title}","tvpipe":"{tvpipe}","pages":{{"host":"host.html","tv":"tv.html","phone":"phone.html"}}}}"#
    );
    let mut e = vec![("g/tvpipe.json".to_owned(), manifest.into_bytes())];
    for p in ["host", "tv", "phone"] {
        e.push((
            format!("g/{p}.html"),
            format!("<p>{p} {title}</p>").into_bytes(),
        ));
    }
    crate::util::zip_write::write(&e).unwrap()
}

/// A PNG's first bytes (enough for the picker's check).
pub(crate) const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

fn sha(b: &[u8]) -> String {
    hex::encode(&crate::crypto::ring::sha256(b))
}

/// The fake: releases of `o/r`, their zips on the CDN, the games list.
pub(crate) struct Fake {
    server: TestServer,
    zips: Vec<(&'static str, Vec<u8>)>,
    cache: Cache,
}

impl Fake {
    pub(crate) fn new(name: &str) -> Fake {
        let v1 = game("G one", &series());
        let v19 = game("G nine", &series());
        let v110 = game("G ten", "9.9");
        let other = game("Other", &series());
        let zips = vec![
            ("g-v1.0.0", v1),
            ("g-v1.9.0", v19),
            ("g-v1.10.0", v110),
            ("v0.2.0", other),
        ];
        let state = Arc::new(Mutex::new((zips.clone(), 0u16, false)));
        let st = state.clone();
        let handler = Arc::new(move |r: &Request| {
            let (zips, port, rate_limited) = st.lock().unwrap().clone();
            let host = r
                .headers
                .get("host")
                .unwrap_or("")
                .split(':')
                .next()
                .unwrap_or("");
            let path = r.target.as_str();
            let zip_of = |tag: &str| zips.iter().find(|(t, _)| *t == tag).map(|(_, z)| z.clone());
            match host {
                WEB => {
                    let parts: Vec<&str> = path.split('/').collect();
                    match parts.as_slice() {
                        ["", "o", "r", "releases", "latest", "download", f] => {
                            let to = format!("/o/r/releases/download/g-v1.0.0/{f}");
                            response(302, &[("Location", to.as_str())], b"")
                        }
                        ["", "o", "r", "releases", "download", tag, "g.zip"]
                            if zip_of(tag).is_some() =>
                        {
                            let to = format!("https://{CDN}:{port}/blob/{tag}?sig=x");
                            response(302, &[("Location", to.as_str())], b"")
                        }
                        _ => response(404, &[], b"Not Found"),
                    }
                }
                CDN => {
                    let tag = path
                        .strip_prefix("/blob/")
                        .and_then(|p| p.split('?').next())
                        .unwrap_or("");
                    match zip_of(tag) {
                        Some(z) => {
                            let etag = format!("\"{}\"", &sha(&z)[..8]);
                            if r.headers.get("if-none-match") == Some(etag.as_str()) {
                                response(304, &[("ETag", etag.as_str())], b"")
                            } else {
                                response(200, &[("ETag", etag.as_str())], &z)
                            }
                        }
                        None => response(404, &[], b""),
                    }
                }
                API if rate_limited => response(
                    403,
                    &[("X-RateLimit-Remaining", "0")],
                    br#"{"message":"API rate limit exceeded"}"#,
                ),
                API => {
                    let rel = |tag: &str, body: &str, digest: Option<String>| {
                        let z = zip_of(tag).unwrap_or_default();
                        let d = digest.unwrap_or_else(|| format!("sha256:{}", sha(&z)));
                        format!(
                            r#"{{"tag_name":"{tag}","draft":false,"prerelease":false,"body":"{body}","assets":[{{"name":"g.zip","digest":"{d}"}},{{"name":"SHA256SUMS"}}]}}"#
                        )
                    };
                    let s = series();
                    match path {
                        "/repos/o/r/releases?per_page=100" => {
                            let list = [
                                rel("v0.2.0", "tvpipe itself", None),
                                rel("g-v1.10.0", "<!-- tvpipe: 9.9 -->", None),
                                rel("g-v1.0.0", &format!("<!-- tvpipe: {s} -->"), None),
                                rel("g-v1.9.0", "no marker", None),
                                r#"{"tag_name":"g-v2.0.0","draft":true,"prerelease":false,"body":"","assets":[]}"#.to_owned(),
                                r#"{"tag_name":"g-v3.0.0","draft":false,"prerelease":true,"body":"","assets":[]}"#.to_owned(),
                            ];
                            response(200, &[], format!("[{}]", list.join(",")).as_bytes())
                        }
                        "/repos/o/r/releases/tags/g-v1.0.0" => response(
                            200,
                            &[],
                            rel("g-v1.0.0", "", Some(format!("sha256:{}", "0".repeat(64))))
                                .as_bytes(),
                        ),
                        "/repos/o/r/releases/latest" => {
                            response(200, &[], rel("g-v1.9.0", "", None).as_bytes())
                        }
                        _ => response(404, &[], br#"{"message":"Not Found"}"#),
                    }
                }
                LIST if path == "/pics/g.png" => {
                    if r.headers.get("if-none-match") == Some("\"pic1\"") {
                        response(304, &[("ETag", "\"pic1\"")], b"")
                    } else {
                        response(200, &[("ETag", "\"pic1\"")], PNG)
                    }
                }
                LIST if path == "/pics/not-a-picture.png" => response(200, &[], b"<html>"),
                LIST => {
                    let list = r#"{"v":1,"games":[{"name":"g","title":"G","summary":"A game","players":{"min":2,"max":8},"languages":["en"],"source":"gh:o/r/g@g-v*","screenshots":[{"src":"pics/g.png","alt":"The TV","kind":"tv"},{"src":"pics/not-a-picture.png","alt":"x","kind":"phone"},{"src":"http://elsewhere.test/x.png","alt":"plain http","kind":"tv"}]},{"name":"h","title":"H","source":"gh:o/r/h@h-v*"},{"name":"Bad Name","title":"B","source":"gh:o/r"}]}"#;
                    if r.headers.get("if-none-match") == Some("\"list1\"") {
                        response(304, &[("ETag", "\"list1\"")], b"")
                    } else {
                        response(200, &[("ETag", "\"list1\"")], list.as_bytes())
                    }
                }
                _ => response(421, &[], b""),
            }
        });
        let server = TestServer::tls(handler);
        state.lock().unwrap().1 = server.addr.port();
        let dir = crate::storage::cache_dir()
            .unwrap()
            .join(format!("remote-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        let fake = Fake {
            server,
            zips,
            cache: Cache::at(dir),
        };
        STATES.lock().unwrap().push((name.to_owned(), state));
        fake
    }

    fn zip(&self, tag: &str) -> &[u8] {
        &self.zips.iter().find(|(t, _)| *t == tag).unwrap().1
    }

    pub(crate) fn fetcher(&self, opts: FetchOptions, token: Option<&str>) -> Fetcher {
        let port = self.server.addr.port();
        let mut client = self.server.client();
        for h in [WEB, API, CDN, LIST] {
            client.resolve.push((h.to_owned(), self.server.addr));
        }
        Fetcher {
            client,
            endpoints: Endpoints {
                web: format!("https://{WEB}:{port}"),
                api: format!("https://{API}:{port}"),
                list: format!("https://{LIST}:{port}/games.json"),
                token: token.map(str::to_owned),
            },
            cache: self.cache.clone(),
            opts,
            events: Events::none(),
        }
    }

    fn requests(&self) -> Vec<(String, String)> {
        let r = self.server.requests.lock().unwrap();
        r.iter()
            .map(|r| {
                let host = r
                    .headers
                    .get("host")
                    .unwrap_or("")
                    .split(':')
                    .next()
                    .unwrap_or("");
                (host.to_owned(), r.target.clone())
            })
            .collect()
    }

    pub(crate) fn count(&self, host: &str) -> usize {
        self.requests().iter().filter(|(h, _)| h == host).count()
    }

    fn set_rate_limited(&self, name: &str) {
        let states = STATES.lock().unwrap();
        let (_, s) = states.iter().find(|(n, _)| n == name).unwrap();
        s.lock().unwrap().2 = true;
    }
}

type State = Arc<Mutex<(Vec<(&'static str, Vec<u8>)>, u16, bool)>>;
static STATES: Mutex<Vec<(String, State)>> = Mutex::new(Vec::new());

fn remote(s: &str) -> Source {
    Source::parse(s).unwrap()
}

#[test]
fn a_tag_prefix_takes_the_newest_release_that_fits() {
    let f = Fake::new("prefix");
    let fetcher = f.fetcher(FetchOptions::default(), None);
    let got = fetcher.fetch(&remote("gh:o/r/g@g-v*")).unwrap();
    // 1.10.0 is made for another series, 2.0.0 a draft, 3.0.0 a
    // prerelease; 1.9.0 has no marker and counts as fitting.
    assert_eq!(got.tag.as_deref(), Some("g-v1.9.0"));
    assert_eq!(got.bundle.manifest.title, "G nine");
    assert_eq!(got.sha256, sha(f.zip("g-v1.9.0")));
    assert_eq!(got.key, "gh:o/r/g.zip@g-v*");
    assert!(!got.trusted && !got.pinned && !got.changed);
    assert!(got.pin_hint.ends_with(&format!("#sha256={}", got.sha256)));
    let q = got.question();
    assert!(
        q.contains("g (\"G nine\") from gh:o/r/g.zip@g-v*, release g-v1.9.0"),
        "{q}"
    );
    assert!(q.contains(&got.sha256) && q.contains("[y/N]"), "{q}");
    assert_eq!(f.count(CDN), 1);
    // Again: the API is asked (the prefix moves), the same asset is not
    // downloaded again.
    fetcher.trust(&got).unwrap();
    let again = fetcher.fetch(&remote("gh:o/r/g@g-v*")).unwrap();
    assert!(again.trusted);
    assert_eq!(f.count(API), 2);
    assert_eq!(f.count(CDN), 1);
    // --refresh downloads it again.
    let fetcher = f.fetcher(
        FetchOptions {
            refresh: true,
            ..FetchOptions::default()
        },
        None,
    );
    fetcher.fetch(&remote("gh:o/r/g@g-v*")).unwrap();
    assert_eq!(f.count(CDN), 2);
}

#[test]
fn an_exact_tag_with_an_asset_needs_no_api_and_no_second_download() {
    let f = Fake::new("exact");
    let fetcher = f.fetcher(FetchOptions::default(), None);
    let got = fetcher.fetch(&remote("gh:o/r/g@g-v1.0.0")).unwrap();
    assert_eq!(got.bundle.manifest.title, "G one");
    fetcher.fetch(&remote("gh:o/r/g@g-v1.0.0")).unwrap();
    assert_eq!(f.count(API), 0);
    assert_eq!((f.count(WEB), f.count(CDN)), (1, 1));
    // The bundle is named by its source.
    assert_eq!(
        got.bundle.origin,
        std::path::PathBuf::from("gh:o/r/g.zip@g-v1.0.0")
    );
}

#[test]
fn the_latest_asset_follows_redirects_and_revalidates() {
    let f = Fake::new("latest");
    let fetcher = f.fetcher(FetchOptions::default(), None);
    let got = fetcher.fetch(&remote("gh:o/r/g")).unwrap();
    assert_eq!(got.sha256, sha(f.zip("g-v1.0.0")));
    let got = fetcher.fetch(&remote("gh:o/r/g")).unwrap();
    assert_eq!(got.sha256, sha(f.zip("g-v1.0.0")));
    assert_eq!(f.count(API), 0);
    let reqs = f.server.requests.lock().unwrap();
    let cdn: Vec<_> = reqs
        .iter()
        .filter(|r| r.headers.get("host").unwrap_or("").starts_with(CDN))
        .collect();
    assert_eq!(cdn.len(), 2);
    assert!(cdn[1].headers.get("if-none-match").is_some());
}

#[test]
fn digests_and_pins_are_checked() {
    let f = Fake::new("digests");
    let fetcher = f.fetcher(FetchOptions::default(), None);
    // The API's digest for g-v1.0.0 is wrong.
    let e = fetcher.fetch(&remote("gh:o/r@g-v1.0.0")).unwrap_err();
    assert!(e.message().contains("GitHub says"), "{e}");
    let good = sha(f.zip("g-v1.9.0"));
    let port = f.server.addr.port();
    let url = format!("https://{CDN}:{port}/blob/g-v1.9.0");
    let e = fetcher
        .fetch(&remote(&format!("{url}#sha256={}", "1".repeat(64))))
        .unwrap_err();
    assert!(e.message().contains("as pinned"), "{e}");
    let got = fetcher
        .fetch(&remote(&format!("{url}#sha256={good}")))
        .unwrap();
    assert!(got.pinned);
    // Pinned and cached: no request at all.
    let before = f.requests().len();
    let got = fetcher
        .fetch(&remote(&format!("{url}#sha256={good}")))
        .unwrap();
    assert!(got.pinned);
    assert_eq!(f.requests().len(), before);
}

#[test]
fn offline_uses_the_cache_and_a_dead_network_falls_back() {
    let f = Fake::new("offline");
    let offline = f.fetcher(
        FetchOptions {
            offline: true,
            ..FetchOptions::default()
        },
        None,
    );
    let e = offline.fetch(&remote("gh:o/r/g@g-v*")).unwrap_err();
    assert!(e.message().contains("--offline"), "{e}");
    f.fetcher(FetchOptions::default(), None)
        .fetch(&remote("gh:o/r/g@g-v*"))
        .unwrap();
    let n = f.requests().len();
    let got = offline.fetch(&remote("gh:o/r/g@g-v*")).unwrap();
    assert_eq!(got.tag.as_deref(), Some("g-v1.9.0"));
    assert_eq!(f.requests().len(), n);
    // Nobody answers: the copy from before.
    let mut dead = f.fetcher(FetchOptions::default(), None);
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = closed.local_addr().unwrap();
    drop(closed);
    for r in &mut dead.client.resolve {
        r.1 = addr;
    }
    let got = dead.fetch(&remote("gh:o/r/g@g-v*")).unwrap();
    assert_eq!(got.tag.as_deref(), Some("g-v1.9.0"));
    // A server's no is not a dead network.
    f.set_rate_limited("offline");
    let e = f
        .fetcher(FetchOptions::default(), None)
        .fetch(&remote("gh:o/r/g@g-v*"))
        .unwrap_err();
    assert!(e.message().contains("GITHUB_TOKEN"), "{e}");
}

#[test]
fn names_come_from_the_games_list() {
    let f = Fake::new("list");
    let fetcher = f.fetcher(FetchOptions::default(), None);
    let got = fetcher.fetch(&Source::Name("g".into())).unwrap();
    assert_eq!(got.tag.as_deref(), Some("g-v1.9.0"));
    assert!(
        got.shown.contains("\"g\" in the games list"),
        "{}",
        got.shown
    );
    let e = fetcher.fetch(&Source::Name("nope".into())).unwrap_err();
    assert!(e.message().contains("g, h"), "{e}");
    // The list's ETag is sent again.
    let reqs = f.server.requests.lock().unwrap();
    let lists: Vec<_> = reqs
        .iter()
        .filter(|r| r.headers.get("host").unwrap_or("").starts_with(LIST))
        .collect();
    assert_eq!(lists.len(), 2);
    assert_eq!(lists[1].headers.get("if-none-match"), Some("\"list1\""));
    drop(reqs);
    // Offline: the list and the game from the cache.
    let offline = f.fetcher(
        FetchOptions {
            offline: true,
            ..FetchOptions::default()
        },
        None,
    );
    assert_eq!(
        offline
            .fetch(&Source::Name("g".into()))
            .unwrap()
            .tag
            .as_deref(),
        Some("g-v1.9.0")
    );
}

#[test]
fn the_token_goes_to_the_api_only() {
    let f = Fake::new("token");
    let fetcher = f.fetcher(FetchOptions::default(), Some("t0k"));
    fetcher.fetch(&remote("gh:o/r")).unwrap();
    let reqs = f.server.requests.lock().unwrap();
    assert!(!reqs.is_empty());
    for r in reqs.iter() {
        let host = r.headers.get("host").unwrap_or("");
        let auth = r.headers.get("authorization");
        if host.starts_with(API) {
            assert_eq!(auth, Some("Bearer t0k"));
            assert_eq!(r.headers.get("accept"), Some("application/vnd.github+json"));
        } else {
            assert_eq!(auth, None, "{host}");
        }
    }
}

#[test]
fn a_changed_hash_asks_again() {
    let f = Fake::new("changed");
    let fetcher = f.fetcher(FetchOptions::default(), None);
    let got = fetcher.fetch(&remote("gh:o/r/g@g-v1.0.0")).unwrap();
    fetcher.trust(&got).unwrap();
    // The record says another hash was approved.
    let mut idx = f.cache.index();
    idx.sources.get_mut(&got.key).unwrap().trusted = Some("f".repeat(64));
    f.cache.save(&idx).unwrap();
    let again = fetcher.fetch(&remote("gh:o/r/g@g-v1.0.0")).unwrap();
    assert!(!again.trusted && again.changed);
    assert!(again.question().starts_with("This game changed"));
}

#[test]
fn releases_and_versions() {
    assert!(version_key("1.10.0") > version_key("1.9.3"));
    assert!(version_key("v2") > version_key("1.99"));
    assert_eq!(version_key("x"), Vec::<u64>::new());
    let r = Release {
        tag: "t".into(),
        body: "Notes\n<!-- tvpipe: 0.3 -->\n".into(),
        assets: vec![("a.zip".into(), None), ("b.zip".into(), None)],
    };
    assert_eq!(r.series(), Some("0.3"));
    let g = |asset: Option<&str>, repo: &str| GitHub {
        owner: "o".into(),
        repo: repo.into(),
        asset: asset.map(str::to_owned),
        tag: Tag::Latest,
    };
    assert_eq!(r.pick(&g(None, "b")).unwrap().0, "b.zip");
    assert!(r
        .pick(&g(None, "c"))
        .unwrap_err()
        .message()
        .contains("a.zip, b.zip"));
    assert!(r.pick(&g(Some("c.zip"), "r")).is_err());
    assert_eq!(seg("a b/c+d"), "a%20b%2Fc%2Bd");
}

/// Opt-in (`TVPIPE_TEST_NET=1`): the real games list and GitHub.
#[test]
fn live_games_list() {
    if std::env::var_os("TVPIPE_TEST_NET").is_none() {
        return;
    }
    let dir = crate::storage::cache_dir().unwrap().join("remote-live");
    let fetcher = Fetcher {
        client: Client::from_env(None),
        endpoints: Endpoints::from_env(),
        cache: Cache::at(dir),
        opts: FetchOptions::default(),
        events: Events::none(),
    };
    let got = fetcher.fetch(&Source::Name("drawbluff".into())).unwrap();
    assert_eq!(got.bundle.manifest.name, "drawbluff");
}
