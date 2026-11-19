//! Fonts and text against known answers from fontTools
//! (`tests/fonts/plex-sans.txt`, written by `scripts/fonts/ttf.py`): glyph
//! ids, advances, exact outline areas and bounds, GPOS kerning. And a
//! rendered line against a checked-in PGM (`TVPIPE_BLESS=1` rewrites it),
//! and broken font data that must not panic.

use std::path::Path as FsPath;

use tvpipe_canvas::{Align, Baseline, Canvas, Color, Font, Text, TextStyle};

fn dir() -> std::path::PathBuf {
    FsPath::new(env!("CARGO_MANIFEST_DIR")).join("tests/fonts")
}

fn font_bytes() -> Vec<u8> {
    std::fs::read(dir().join("plex-sans.ttf")).unwrap()
}

fn font() -> Font {
    Font::parse(font_bytes()).unwrap()
}

/// Blocks of `key = value` lines.
fn vectors() -> Vec<Vec<(String, String)>> {
    let text = std::fs::read_to_string(dir().join("plex-sans.txt")).unwrap();
    let mut out: Vec<Vec<(String, String)>> = vec![Vec::new()];
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        if line.trim().is_empty() {
            out.push(Vec::new());
            continue;
        }
        let (k, v) = line.split_once(" = ").unwrap();
        out.last_mut().unwrap().push((k.into(), v.into()));
    }
    out.retain(|b| !b.is_empty());
    out
}

fn get<'a>(b: &'a [(String, String)], k: &str) -> &'a str {
    &b.iter().find(|(key, _)| key == k).unwrap().1
}

/// Coverage sum and ink box (x0, y0, x1, y1) of a canvas's alpha.
fn ink(c: &Canvas) -> (f64, (i32, i32, i32, i32)) {
    let mut sum = 0.0;
    let mut b = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for y in 0..c.height() as i32 {
        for x in 0..c.width() as i32 {
            let a = c.pixel(x, y).a;
            if a > 0 {
                sum += a as f64 / 255.0;
                b = (b.0.min(x), b.1.min(y), b.2.max(x + 1), b.3.max(y + 1));
            }
        }
    }
    (sum, b)
}

#[test]
fn metrics_glyphs_areas_and_bounds() {
    let f = font();
    let v = vectors();
    let head = &v[0];
    assert_eq!(f.units_per_em().to_string(), get(head, "units_per_em"));
    assert_eq!(f.ascender().to_string(), get(head, "ascender"));
    assert_eq!(f.descender().to_string(), get(head, "descender"));
    let mut text = Text::new();
    let mut checked = 0;
    for b in v.iter().filter(|b| b[0].0 == "char") {
        let cp = u32::from_str_radix(get(b, "char").trim_start_matches("U+"), 16).unwrap();
        let ch = char::from_u32(cp).unwrap();
        let g = f.glyph_index(ch).unwrap();
        assert_eq!(g.to_string(), get(b, "glyph"), "{ch}");
        assert_eq!(f.advance(g).to_string(), get(b, "advance"), "{ch}");
        // At 1000 px/em one font unit is one pixel: the coverage sum is the
        // outline's area, the ink box its bounds.
        let mut c = Canvas::new(1000, 1300);
        text.draw(
            &mut c,
            &ch.to_string(),
            100.0,
            1000.0,
            &TextStyle::new(&f, 1000.0),
        );
        let (sum, ink_box) = ink(&c);
        let area: f64 = get(b, "area").parse().unwrap();
        assert!(
            (sum - area).abs() / area < 0.003,
            "{ch}: area {sum} vs {area}"
        );
        let bounds: Vec<i32> = get(b, "bounds")
            .split(' ')
            .map(|n| n.parse().unwrap())
            .collect();
        let want = (
            100 + bounds[0],
            1000 - bounds[3],
            100 + bounds[2],
            1000 - bounds[1],
        );
        for (got, want) in [
            (ink_box.0, want.0),
            (ink_box.1, want.1),
            (ink_box.2, want.2),
            (ink_box.3, want.3),
        ] {
            assert!(
                (got - want).abs() <= 1,
                "{ch}: ink {ink_box:?} vs bounds {want:?}"
            );
        }
        // Small sizes keep the area too.
        let mut c = Canvas::new(60, 60);
        text.draw(
            &mut c,
            &ch.to_string(),
            10.0,
            40.0,
            &TextStyle::new(&f, 32.0),
        );
        let small = area * (32.0f64 / 1000.0).powi(2);
        let (sum, _) = ink(&c);
        assert!(
            (sum - small).abs() / small < 0.02,
            "{ch}: area {sum} vs {small} at 32 px"
        );
        checked += 1;
    }
    assert_eq!(checked, 10);
}

#[test]
fn kerning_from_gpos() {
    let f = font();
    let text = Text::new();
    let style = TextStyle::new(&f, 1000.0);
    for b in vectors().iter().filter(|b| b[0].0 == "pair") {
        let pair: Vec<char> = get(b, "pair").chars().collect();
        let (l, r) = (
            f.glyph_index(pair[0]).unwrap(),
            f.glyph_index(pair[1]).unwrap(),
        );
        let kern: i16 = get(b, "kern").parse().unwrap();
        assert_eq!(f.kerning(l, r), kern, "{pair:?}");
        let want = f.advance(l) as f32 + kern as f32 + f.advance(r) as f32;
        let s: String = pair.iter().collect();
        assert_eq!(text.measure(&s, &style), want, "{pair:?}");
    }
}

#[test]
fn missing_characters_and_emoji() {
    let f = font();
    let text = Text::new();
    let style = TextStyle::new(&f, 100.0);
    // Not in the font: the missing-glyph box (glyph 0's advance).
    let boxed = text.measure("\u{4e00}", &style);
    assert_eq!(boxed, f.advance(0) as f32 / 10.0);
    // Emoji: left out.
    assert_eq!(text.measure("A🎉", &style), text.measure("A", &style));
    assert_eq!(text.measure("👍🏽", &style), 0.0);
}

#[test]
fn wrapping_and_ellipsis() {
    let f = font();
    let text = Text::new();
    let style = TextStyle::new(&f, 20.0);
    let w = text.measure("Hello world", &style);
    assert_eq!(text.wrap("Hello world", w, &style), vec!["Hello world"]);
    assert_eq!(
        text.wrap("Hello world", w - 1.0, &style),
        vec!["Hello", "world"]
    );
    assert_eq!(text.wrap("a\nb", 1000.0, &style), vec!["a", "b"]);
    let long = text.wrap("Supercalifragilistic", 60.0, &style);
    assert!(
        long.len() > 1 && long.iter().all(|l| text.measure(l, &style) <= 60.0),
        "{long:?}"
    );
    assert_eq!(text.ellipsize("Hello world", w, &style), "Hello world");
    let cut = text.ellipsize("Hello world", w - 1.0, &style);
    assert!(
        cut.ends_with('…') && text.measure(&cut, &style) <= w - 1.0,
        "{cut}"
    );
    assert_eq!(text.ellipsize("Hello", 0.0, &style), "…");
}

#[test]
fn alignment_and_baselines() {
    let f = font();
    let mut text = Text::new();
    let style = TextStyle::new(&f, 40.0).color(Color::WHITE);
    let mut c = Canvas::new(400, 100);
    let w = text.draw(&mut c, "H", 200.0, 50.0, &style.clone().align(Align::Right));
    let (_, b) = ink(&c);
    assert!(b.2 <= 200 && b.2 as f32 >= 200.0 - w, "{b:?}");
    let mut c = Canvas::new(400, 100);
    text.draw(
        &mut c,
        "H",
        200.0,
        50.0,
        &style
            .clone()
            .align(Align::Center)
            .baseline(Baseline::Middle),
    );
    let (_, b) = ink(&c);
    assert!(((b.0 + b.2) / 2 - 200).abs() <= 1, "{b:?}");
    let mut c = Canvas::new(400, 100);
    text.draw(&mut c, "H", 10.0, 0.0, &style.baseline(Baseline::Top));
    let (_, b) = ink(&c);
    // The ascender (1025/1000) is above the cap height (698/1000).
    assert!(b.1 > 5 && b.1 < 20, "{b:?}");
}

#[test]
fn a_line_against_its_pgm() {
    let f = font();
    let mut text = Text::new();
    let mut c = Canvas::new(240, 40);
    c.clear(Color::BLACK);
    text.draw(
        &mut c,
        "Hamburgefonstiv ÄÖé…",
        4.0,
        28.0,
        &TextStyle::new(&f, 20.0),
    );
    let gray: Vec<u8> = c.data().chunks(4).map(|p| p[0]).collect();
    let path = dir().join("line.pgm");
    let pgm = [
        format!("P5\n{} {}\n255\n", c.width(), c.height()).into_bytes(),
        gray,
    ]
    .concat();
    if std::env::var("TVPIPE_BLESS").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &pgm).unwrap();
    }
    let want = std::fs::read(&path).expect("tests/fonts/line.pgm (TVPIPE_BLESS=1 writes it)");
    assert!(
        want == pgm,
        "the rendered line differs from tests/fonts/line.pgm"
    );
}

#[test]
fn broken_fonts_are_errors() {
    let bytes = font_bytes();
    let mut text = Text::new();
    let mut c = Canvas::new(64, 64);
    let mut try_font = |data: Vec<u8>| {
        if let Ok(f) = Font::parse(data) {
            for g in 0..f.glyph_count() {
                let _ = f.outline(g);
                let _ = f.kerning(g, g.wrapping_add(1));
            }
            text.draw(&mut c, "AVé…Hg&", 2.0, 40.0, &TextStyle::new(&f, 24.0));
        }
    };
    // Truncated anywhere.
    for len in (0..bytes.len()).step_by(97) {
        try_font(bytes[..len].to_vec());
    }
    // Bytes changed (a fixed pseudo-random sequence).
    let mut seed = 0x2545_f491_u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for _ in 0..300 {
        let mut b = bytes.clone();
        for _ in 0..1 + next() % 8 {
            let i = next() as usize % b.len();
            b[i] = next() as u8;
        }
        try_font(b);
    }
}

#[test]
fn wrapped_paragraphs() {
    let f = font();
    let mut text = Text::new();
    let mut c = Canvas::new(200, 200);
    let style = TextStyle::new(&f, 20.0).leading(30.0);
    let n = text.draw_wrapped(
        &mut c,
        "one two three four five six",
        (10.0, 30.0),
        80.0,
        &style,
    );
    assert!(n >= 3, "{n}");
    let (_, b) = ink(&c);
    // The last line's baseline is (n − 1) leadings below the first.
    assert!(
        b.3 >= 30 + (n as i32 - 1) * 30 - 2 && b.3 <= 30 + (n as i32 - 1) * 30 + 6,
        "{b:?}"
    );
}
