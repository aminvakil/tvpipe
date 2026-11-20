//! The picker against the fake GitHub and games list of remote's tests:
//! the list with its pictures, a choice, the question, the start.

use std::time::{Duration, Instant};

use super::*;
use crate::net::fetch::Client;
use crate::playshare::remote::tests::{Fake, PNG};

struct T {
    picker: Picker,
    _fake: Arc<Fake>,
    client: Client,
    cookie: String,
}

fn start(name: &str, yes: bool) -> T {
    let fake = Arc::new(Fake::new(name));
    let f = fake.clone();
    let make: MakeFetcher = Arc::new(move |events| {
        let mut fetcher = f.fetcher(FetchOptions::default(), None);
        fetcher.events = events;
        Ok(fetcher)
    });
    let picker = Picker::start_with(make, yes, 0).unwrap();
    let cookie = format!(
        "{}={}",
        cookie_name(picker.shared.port()),
        picker.shared.token
    );
    T {
        picker,
        _fake: fake,
        client: Client::default(),
        cookie,
    }
}

impl T {
    fn url(&self, path: &str) -> Url {
        Url::parse(&format!(
            "http://127.0.0.1:{}{path}",
            self.picker.shared.port()
        ))
        .unwrap()
    }

    fn get(&self, path: &str) -> (u16, Vec<u8>) {
        self.client
            .send(
                "GET",
                &self.url(path),
                &[("Cookie", &self.cookie)],
                None,
                4 << 20,
            )
            .unwrap()
    }

    fn get_json(&self, path: &str) -> Value {
        let (status, body) = self.get(path);
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        Value::parse(&String::from_utf8_lossy(&body)).unwrap()
    }

    fn post(&self, body: &str) -> (u16, Value) {
        let (status, body) = self
            .client
            .send(
                "POST",
                &self.url("/_tvpipe/api/game"),
                &[("Cookie", &self.cookie)],
                Some(("application/json", body.as_bytes().to_vec())),
                1 << 20,
            )
            .unwrap();
        (
            status,
            Value::parse(&String::from_utf8_lossy(&body)).unwrap(),
        )
    }

    /// The state once it is not `fetching` any more.
    fn settled(&self) -> Value {
        let t0 = Instant::now();
        loop {
            let v = self.get_json("/_tvpipe/api/game");
            if v.get("state").and_then(Value::as_str) != Some("fetching") {
                return v;
            }
            assert!(t0.elapsed() < Duration::from_secs(20), "still fetching");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn state(v: &Value) -> &str {
    v.get("state").and_then(Value::as_str).unwrap_or("")
}

#[test]
fn the_list_with_its_pictures() {
    let t = start("picker-list", false);
    let c = t.get_json("/_tvpipe/api/catalog");
    let games = c.get("games").and_then(Value::as_array).unwrap();
    // "Bad Name" is no name.
    let names: Vec<&str> = games
        .iter()
        .filter_map(|g| g.get("name").and_then(Value::as_str))
        .collect();
    assert_eq!(names, ["g", "h"]);
    let g = &games[0];
    assert_eq!(g.get("summary").and_then(Value::as_str), Some("A game"));
    assert_eq!(g.get("min").and_then(Value::as_u64), Some(2));
    assert_eq!(g.get("downloaded").and_then(Value::as_bool), Some(false));
    // Plain http pictures are left out; the others are tvpipe's URLs.
    let shots = g.get("screenshots").and_then(Value::as_array).unwrap();
    assert_eq!(shots.len(), 2);
    let src = shots[0].get("src").and_then(Value::as_str).unwrap();
    assert_eq!(src, "/_tvpipe/catalog/0");
    let (status, body) = t.get(src);
    assert_eq!((status, body.as_slice()), (200, PNG));
    // Not a picture: refused.
    let src = shots[1].get("src").and_then(Value::as_str).unwrap();
    assert_eq!(t.get(src).0, 404);
    assert_eq!(t.get("/_tvpipe/catalog/9").0, 404);
}

#[test]
fn a_choice_asks_then_starts() {
    let t = start("picker-choice", false);
    assert_eq!(state(&t.get_json("/_tvpipe/api/game")), "idle");
    let (status, v) = t.post(r#"{"source":"g"}"#);
    assert_eq!(status, 200, "{v:?}");
    let v = t.settled();
    assert_eq!(state(&v), "ask", "{v:?}");
    let game = v.get("game").unwrap();
    assert_eq!(game.get("title").and_then(Value::as_str), Some("G nine"));
    assert_eq!(
        game.get("release").and_then(Value::as_str),
        Some("g-v1.9.0")
    );
    assert!(t.picker.take().is_none());
    // Back: the list again; then the same game, and yes.
    assert_eq!(state(&t.post(r#"{"back":true}"#).1), "idle");
    t.post(r#"{"source":"g"}"#);
    assert_eq!(state(&t.settled()), "ask");
    let (_, v) = t.post(r#"{"trust":true}"#);
    assert_eq!(state(&v), "starting");
    let b = t.picker.take().unwrap();
    assert_eq!(b.manifest.title, "G nine");
    assert!(t.picker.take().is_none());
    // Busy now.
    assert_eq!(t.post(r#"{"source":"g"}"#).0, 409);
    t.picker
        .started("http://127.0.0.1:1/_tvpipe/start?t=x".into());
    let v = t.get_json("/_tvpipe/api/game");
    assert_eq!(state(&v), "ready");
    assert_eq!(
        v.get("url").and_then(Value::as_str),
        Some("http://127.0.0.1:1/_tvpipe/start?t=x")
    );
}

#[test]
fn a_game_said_yes_to_or_yes_needs_no_question() {
    let t = start("picker-yes", true);
    t.post(r#"{"source":"gh:o/r/g@g-v*"}"#);
    assert_eq!(state(&t.settled()), "starting");
    assert!(t.picker.take().is_some());
}

#[test]
fn errors_and_refusals() {
    let t = start("picker-errors", false);
    // A path on this computer is the command line's.
    let (status, v) = t.post(r#"{"source":"./games/x"}"#);
    assert_eq!(status, 400, "{v:?}");
    t.post(r#"{"source":"nope"}"#);
    let v = t.settled();
    assert_eq!(state(&v), "failed");
    assert!(
        v.get("error")
            .and_then(Value::as_str)
            .unwrap()
            .contains("no game of that name"),
        "{v:?}"
    );
    assert_eq!(t.post(r#"{"trust":true}"#).0, 409);
    assert_eq!(state(&t.post(r#"{"back":true}"#).1), "idle");
    // No cookie: no API; the page itself is free.
    let anon = |path: &str| {
        t.client
            .send("GET", &t.url(path), &[], None, 1 << 20)
            .unwrap()
            .0
    };
    assert_eq!(anon("/_tvpipe/api/catalog"), 403);
    assert_eq!(anon("/"), 403);
    assert_eq!(anon("/_tvpipe/picker/"), 200);
    // Another site's page.
    let (status, _) = t
        .client
        .send(
            "POST",
            &t.url("/_tvpipe/api/game"),
            &[("Cookie", &t.cookie), ("Origin", "https://evil.test")],
            Some(("application/json", br#"{"back":true}"#.to_vec())),
            1 << 20,
        )
        .unwrap();
    assert_eq!(status, 403);
    // The start link sets the cookie.
    let start = t.picker.start_url();
    let path = &start[start.find("/_tvpipe").unwrap()..];
    let (status, _) = t
        .client
        .send("GET", &t.url(path), &[], None, 1 << 20)
        .unwrap();
    assert_eq!(status, 302);
}
