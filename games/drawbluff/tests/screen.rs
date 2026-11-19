//! The video screen draws every phase (the Rust twin of
//! `test/video.test.ts`). `DRAWBLUFF_SCREEN_OUT=DIR` writes the frames
//! (`NAME.ppm`) and the views (`views.json`) there, to look at them or
//! to render the same views with the Node renderer.

use drawbluff::bots::doodle;
use drawbluff::rules::Rng;
use drawbluff::screen::Screen;
use tvpipe_canvas::Canvas;
use tvpipe_party::{json, Value};

const NOW: u64 = 1_000_000;

fn views() -> (Value, Value, Vec<(&'static str, Value)>) {
    let roster: Vec<Value> = ["Ada", "Bea", "Cy", "Dot with a really very long name"]
        .iter()
        .enumerate()
        .map(|(i, name)| {
            json!({ "id": format!("p{}", i + 1), "name": *name, "vip": i == 0, "connected": i != 2 })
        })
        .collect();
    // A 21x21 code: dark finder corners are enough to check placement.
    let rows: Vec<String> = (0..21)
        .map(|y| {
            (0..21)
                .map(|x| {
                    let dark = (x < 7 && y < 7) || (x > 13 && y < 7) || (x < 7 && y > 13);
                    if dark {
                        '1'
                    } else {
                        '0'
                    }
                })
                .collect()
        })
        .collect();
    let join = json!({
        "code": "BCDF",
        "url": "https://words-words-words-words.trycloudflare.com/?r=BCDF#k=abcdefghijklmnopqrstuv",
        "qr": { "size": 21, "rows": rows },
    });
    let drawing = doodle(&mut Rng::new(3));
    let artist = json!({ "id": "p1", "name": "Ada" });
    let lines = |delta: i64| -> Value {
        Value::Array(
            roster
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    json!({ "id": r["id"].clone(), "name": r["name"].clone(), "score": 3000 - i as i64 * 700, "delta": delta })
                })
                .collect(),
        )
    };
    let views = vec![
        (
            "lobby",
            json!({ "phase": "lobby", "minPlayers": 3, "maxPlayers": 8 }),
        ),
        (
            "draw",
            json!({ "phase": "draw", "round": 1, "rounds": 2, "deadline": NOW + 42_000, "waiting": ["p1", "p2", "p3", "p4"], "done": ["p2"] }),
        ),
        (
            "lie",
            json!({ "phase": "lie", "round": 1, "deadline": NOW + 5000, "drawing": drawing.clone(), "artist": artist.clone(), "waiting": ["p2", "p3"], "done": [] }),
        ),
        (
            "vote",
            json!({
                "phase": "vote", "round": 1, "deadline": NOW + 9000, "drawing": drawing.clone(), "artist": artist.clone(),
                "options": [
                    { "id": "o1", "text": "a haunted toaster" },
                    { "id": "o2", "text": "a sad potato" },
                    { "id": "o3", "text": "a convincing lie that is rather long indeed, yes" },
                ],
                "waiting": ["p2", "p3"], "done": ["p3"],
            }),
        ),
        (
            "reveal",
            json!({
                "phase": "reveal", "round": 2, "drawing": drawing, "artist": artist, "prompt": "a haunted toaster", "step": 3,
                "items": [
                    { "id": "o2", "text": "a sad potato", "truth": false, "decoy": false, "authors": ["Bea"], "voters": ["Cy"], "points": 500, "artistPoints": 0 },
                    { "id": "o3", "text": "a lie", "truth": false, "decoy": true, "authors": [], "voters": ["Dot"], "points": 0, "artistPoints": 0 },
                    { "id": "o1", "text": "a haunted toaster", "truth": true, "decoy": false, "authors": [], "voters": ["Bea"], "points": 1000, "artistPoints": 1000 },
                ],
            }),
        ),
        (
            "scores",
            json!({ "phase": "scores", "round": 1, "rounds": 2, "scores": lines(500), "last": false }),
        ),
        ("final", json!({ "phase": "final", "scores": lines(0) })),
        ("none", json!({})),
    ];
    (Value::Array(roster), join, views)
}

fn ppm(canvas: &Canvas) -> Vec<u8> {
    let mut out = format!("P6\n{} {}\n255\n", canvas.width(), canvas.height()).into_bytes();
    for p in canvas.data().chunks(4) {
        out.extend_from_slice(&p[..3]);
    }
    out
}

#[test]
fn the_video_screen_draws_every_phase() {
    let (roster, join, views) = views();
    let out = std::env::var_os("DRAWBLUFF_SCREEN_OUT").map(std::path::PathBuf::from);
    let mut screen = Screen::new();
    let mut canvas = Canvas::new(1280, 720);
    let mut all = tvpipe_party::Object::new();
    for (name, s) in &views {
        let view = json!({ "roster": roster.clone(), "screen": s.clone(), "join": join.clone() });
        screen.render(&mut canvas, &view, NOW);
        // Something besides the background was drawn.
        let bright = canvas
            .data()
            .chunks(4)
            .step_by(97)
            .filter(|p| p[0] > 200 && p[1] > 200)
            .count();
        assert!(bright > 50, "phase {name}: {bright}");
        if let Some(dir) = &out {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(format!("{name}.ppm")), ppm(&canvas)).unwrap();
            all.insert(*name, view);
        }
    }
    if let Some(dir) = &out {
        std::fs::write(dir.join("views.json"), Value::Object(all).to_string()).unwrap();
    }
}

#[test]
fn the_lobbys_qr_code_dark_finder_light_quiet_zone() {
    let (roster, join, views) = views();
    let mut screen = Screen::new();
    let mut canvas = Canvas::new(1280, 720);
    let view = json!({ "roster": roster, "screen": views[0].1.clone(), "join": join });
    screen.render(&mut canvas, &view, NOW);
    // Centred at x = 25vw, from y = 22vh, 46vh square, 2 modules of quiet zone (25 in all).
    let size = 0.46 * 720.0;
    let (x0, y0) = (0.25 * 1280.0 - size / 2.0, 0.22 * 720.0);
    let module = size / 25.0;
    let at = |mx: f32, my: f32| {
        let p = canvas.pixel(
            (x0 + (mx + 0.5) * module).round() as i32,
            (y0 + (my + 0.5) * module).round() as i32,
        );
        u32::from(p.r) + u32::from(p.g) + u32::from(p.b)
    };
    assert!(at(0.0, 0.0) > 700, "quiet zone is light");
    assert!(at(2.0, 2.0) < 100, "finder is dark");
    assert!(at(12.0, 12.0) > 700, "centre is light");
    assert!(at(22.0, 22.0) > 700, "no finder bottom right");
}

#[test]
fn odd_views_do_not_panic() {
    let mut screen = Screen::new();
    let mut canvas = Canvas::new(320, 180);
    for view in [
        json!(null),
        json!({ "screen": { "phase": "vote", "options": [1, null, "x"], "drawing": { "strokes": [[99, -5, 1e9, -1e9], "x", [0]] } } }),
        json!({ "screen": { "phase": "reveal", "items": [{}], "step": 5 }, "join": { "qr": { "size": 3, "rows": [1, "111111"] } } }),
        json!({ "screen": { "phase": "lobby", "maxPlayers": 1000 }, "roster": [{ "name": "😀😀" }] }),
        json!({ "screen": { "phase": "scores", "scores": [{}, {}], "deadline": "soon" } }),
    ] {
        screen.render(&mut canvas, &view, NOW);
    }
}

/// How long a frame takes per phase (`cargo test --release -p drawbluff
/// --test screen -- --ignored --nocapture`).
#[test]
#[ignore]
fn render_speed() {
    let (roster, join, views) = views();
    let mut screen = Screen::new();
    let mut canvas = Canvas::new(1280, 720);
    for (name, s) in &views {
        let view = json!({ "roster": roster.clone(), "screen": s.clone(), "join": join.clone() });
        screen.render(&mut canvas, &view, NOW);
        let t = std::time::Instant::now();
        for _ in 0..50 {
            screen.render(&mut canvas, &view, NOW);
        }
        println!(
            "{name}: {:.2} ms",
            t.elapsed().as_secs_f64() * 1000.0 / 50.0
        );
    }
}

#[test]
fn the_video_screen_with_50_players() {
    // The same views as test/video.test.ts draws (`node test/vectors.ts --bless`).
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/test/vectors/screens-50.json");
    let big = Value::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
    let out = std::env::var_os("DRAWBLUFF_SCREEN_OUT").map(std::path::PathBuf::from);
    let now = big["now"].as_u64().unwrap();
    let mut screen = Screen::new();
    let mut canvas = Canvas::new(1280, 720);
    for v in big["views"].as_array().unwrap() {
        let name = v[0].as_str().unwrap();
        let view = json!({ "roster": big["roster"].clone(), "screen": v[1].clone(), "join": big["join"].clone() });
        screen.render(&mut canvas, &view, now);
        let bright = canvas
            .data()
            .chunks(4)
            .step_by(97)
            .filter(|p| p[0] > 200 && p[1] > 200)
            .count();
        assert!(bright > 50, "{name}: {bright}");
        if let Some(dir) = &out {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(format!("rust-{name}.ppm")), ppm(&canvas)).unwrap();
        }
    }
}

#[test]
fn latecomers_lines() {
    use drawbluff::screen::{corner_line, joined_line, late_line};
    assert_eq!(
        late_line(Some("open")),
        "Join anytime, even during the game"
    );
    assert_eq!(late_line(None), "Latecomers join at the next round");
    assert_eq!(
        late_line(Some("closed")),
        "Joining closes when the game starts"
    );
    assert!(corner_line(Some("open")).contains("next drawing"));
    let s = json!({ "phase": "vote", "joined": ["Anna", "Bob"], "joinedMore": 2 });
    assert_eq!(joined_line(&s), "+ Anna · + Bob · + 2 more");
    assert_eq!(
        joined_line(&json!({ "phase": "lobby", "joined": ["Anna"] })),
        ""
    );
    assert_eq!(joined_line(&json!({ "phase": "draw", "joined": [] })), "");
}
