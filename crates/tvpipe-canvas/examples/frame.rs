//! Renders a busy 1280×720 party-game frame (panels, 40 lines of text,
//! two drawings of 6 000 points each, a QR-like grid) and prints how long
//! it takes; writes the last frame as `frame.ppm` with `--out`.
//!
//! ```text
//! cargo run --release -p tvpipe-canvas --example frame [-- --out frame.ppm]
//! ```

use std::time::Instant;

use tvpipe_canvas::{Align, Canvas, Color, Font, Text, TextStyle};

static FONT: &[u8] = include_bytes!("../tests/fonts/plex-sans.ttf");

fn main() {
    let out = std::env::args().skip_while(|a| a != "--out").nth(1);
    let font = Font::parse(FONT).expect("font");
    let mut text = Text::new();
    let mut canvas = Canvas::new(1280, 720);
    // Two drawings: wobbly strokes, 6 000 points each.
    let drawings: Vec<Vec<Vec<(f32, f32)>>> = (0..2)
        .map(|d| {
            (0..30)
                .map(|s| {
                    (0..200)
                        .map(|i| {
                            let t = i as f32 / 200.0;
                            let x = 80.0 + d as f32 * 600.0 + 40.0 * s as f32 % 400.0 + 120.0 * t;
                            let y = 140.0 + 10.0 * s as f32 + 60.0 * (t * 9.0 + s as f32).sin();
                            (x, y)
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    let frames = 60;
    let start = Instant::now();
    for _ in 0..frames {
        canvas.clear(Color::hex(0x14141c));
        for (d, strokes) in drawings.iter().enumerate() {
            let x = 60.0 + d as f32 * 600.0;
            canvas.fill_rounded_rect(x, 100.0, 560.0, 420.0, 24.0, Color::hex(0xfffdf6));
            for (i, s) in strokes.iter().enumerate() {
                let c = [
                    Color::hex(0x1d1d1f),
                    Color::hex(0xff4f6d),
                    Color::hex(0x3b82f6),
                ][i % 3];
                canvas.stroke_polyline(s, 6.0, c);
            }
        }
        for i in 0..40 {
            let style = TextStyle::new(&font, 18.0).color(Color::WHITE.alpha(0.8));
            let x = 60.0 + (i / 10) as f32 * 300.0;
            let y = 550.0 + (i % 10) as f32 * 17.0;
            text.draw(&mut canvas, "Player name: 1 234 points…", x, y, &style);
        }
        let title = TextStyle::new(&font, 44.0).align(Align::Center);
        text.draw(&mut canvas, "What is it called?", 640.0, 70.0, &title);
        canvas.draw_qr(
            33,
            |x, y| (x * 7 + y * 13) % 5 < 2,
            1100,
            20,
            4,
            2,
            Color::BLACK,
            Color::WHITE,
        );
    }
    let per = start.elapsed() / frames;
    println!("{:.2} ms per frame", per.as_secs_f64() * 1000.0);
    if let Some(path) = out {
        let mut ppm = format!("P6\n{} {}\n255\n", canvas.width(), canvas.height()).into_bytes();
        for p in canvas.data().chunks(4) {
            ppm.extend_from_slice(&p[..3]);
        }
        std::fs::write(&path, ppm).expect("write");
        println!("wrote {path}");
    }
}
