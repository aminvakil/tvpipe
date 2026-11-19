# tvpipe-canvas

A small software 2D renderer for RGBA frames, made for the TV picture of
games streamed with [tvpipe](https://crates.io/crates/tvpipe): fills,
rounded rectangles, circles, anti-aliased paths (exact area coverage)
and round-capped strokes, TrueType text (`glyf` outlines; no shaping),
images and QR codes. No dependencies.

```rust
use tvpipe_canvas::{Canvas, Color, Font, Text, TextStyle};

let font = Font::parse(std::fs::read("font.ttf")?)?;
let mut text = Text::new();
let mut canvas = Canvas::new(1280, 720);
canvas.clear(Color::rgb(0x1d, 0x1d, 0x1f));
canvas.fill_rounded_rect(40.0, 40.0, 400.0, 120.0, 16.0, Color::rgb(255, 255, 255));
text.draw(&mut canvas, "Hello", 60.0, 120.0, &TextStyle::new(&font, 48.0).color(Color::BLACK));
// canvas.data(): RGBA, row by row, for tvpipe's Stream::write_video
# Ok::<(), Box<dyn std::error::Error>>(())
```

Fonts: TrueType with `glyf` outlines (`.ttf`); CFF (`.otf`), WOFF and
collections are refused with an error. No hinting and no shaping:
left-to-right scripts without ligatures; emoji no font has are left out.
Broken font data is an error, never a panic.

`cargo run --release --example frame` renders a busy 1280×720 frame
(two drawings of 6 000 points, 40 lines of text, a QR grid): about
6.5 ms on an Apple M-series core.

Status: in development; see `docs/multiscreen-design.md` §5 in the
repository.
