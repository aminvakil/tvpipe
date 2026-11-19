//! drawbluff's main screen as video frames: the screen view the web
//! screen shows (`{roster, screen, join}`), drawn with tvpipe-canvas (a
//! port of `screen-video.ts`; the same layout, in units of the height
//! (`vh`) and width (`vw`), so any size works).
//!
//! Differences from the Node renderer: no emoji (the fonts have none;
//! docs/multiscreen-design.md §10), ★ and ✓ drawn as shapes (the fonts
//! lack them too), a soft shadow made of a few translucent rectangles,
//! and the lobby's QR code does not pulse (a still lobby costs the
//! encoder nothing).

use tvpipe_canvas::{strip_emoji, Align, Baseline, Canvas, Color, Font, IRect, Text, TextStyle};
use tvpipe_party::Value;

use crate::rules::{GRID, PALETTE};

static PLEX: &[u8] = include_bytes!("../fonts/plex-sans-regular.ttf");
static PLEX_SEMI: &[u8] = include_bytes!("../fonts/plex-sans-semibold.ttf");
static MICHROMA: &[u8] = include_bytes!("../fonts/michroma.ttf");
static SIXTYFOUR: &[u8] = include_bytes!("../fonts/sixtyfour.ttf");

const BG: Color = Color::hex(0x2b1d5c);
const BG2: Color = Color::hex(0x4a2f99);
const INK: Color = Color::hex(0xfffdf6);
const ACCENT: Color = Color::hex(0xffd60a);
const HOT: Color = Color::hex(0xff5d8f);
const OK: Color = Color::hex(0x2ec4b6);
const DARK: Color = Color::hex(0x1d1d1f);
const GREY: Color = Color::hex(0x4a4a55);
const PAPER: Color = Color::hex(0xfffdf6);

/// The typefaces: the web pages' fonts (SIL OFL 1.1, `fonts/OFL.txt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Face {
    /// IBM Plex Sans, regular.
    Sans,
    /// IBM Plex Sans, semibold (the web's `font-weight: 600`).
    Semi,
    /// Michroma: headings, numbers.
    Display,
    /// Sixtyfour: the logo.
    Logo,
}

/// How a piece of text is drawn (`TextOpts` of the Node renderer).
#[derive(Debug, Clone, Copy)]
struct T {
    face: Face,
    size: f32,
    color: Color,
    align: Align,
    base: Baseline,
    max: Option<f32>,
}

fn t(face: Face, size: f32) -> T {
    T {
        face,
        size,
        color: INK,
        align: Align::Left,
        base: Baseline::Top,
        max: None,
    }
}

impl T {
    fn color(self, color: Color) -> T {
        T { color, ..self }
    }

    fn center(self) -> T {
        T {
            align: Align::Center,
            ..self
        }
    }

    fn right(self) -> T {
        T {
            align: Align::Right,
            ..self
        }
    }

    fn middle(self) -> T {
        T {
            base: Baseline::Middle,
            ..self
        }
    }

    fn max(self, max: f32) -> T {
        T {
            max: Some(max),
            ..self
        }
    }
}

/// The renderer: fonts, the glyph cache and the background.
pub struct Screen {
    fonts: Fonts,
    text: Text,
    /// The background gradient for one size.
    bg: Option<(u32, u32, Vec<u8>)>,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Screen").finish_non_exhaustive()
    }
}

impl Default for Screen {
    fn default() -> Screen {
        Screen::new()
    }
}

/// The fonts per face, with fallbacks.
struct Fonts {
    sans: Vec<Font>,
    semi: Vec<Font>,
    display: Vec<Font>,
    logo: Vec<Font>,
}

impl Fonts {
    fn style(&self, o: &T) -> TextStyle<'_> {
        let fonts = match o.face {
            Face::Sans => &self.sans,
            Face::Semi => &self.semi,
            Face::Display => &self.display,
            Face::Logo => &self.logo,
        };
        TextStyle::with_fonts(fonts, o.size.round().max(1.0))
            .color(o.color)
            .align(o.align)
            .baseline(o.base)
    }
}

/// The frame being drawn.
struct F<'a> {
    c: &'a mut Canvas,
    w: f32,
    h: f32,
    /// 1% of the height (the web screen's `vh`).
    vh: f32,
    vw: f32,
    /// Server time (ms), for countdowns.
    now: u64,
}

impl Screen {
    /// A renderer with the embedded fonts.
    pub fn new() -> Screen {
        let font = |data: &'static [u8]| Font::parse(data).expect("embedded font");
        let plex = font(PLEX);
        Screen {
            fonts: Fonts {
                sans: vec![plex.clone()],
                semi: vec![font(PLEX_SEMI), plex.clone()],
                display: vec![font(MICHROMA), plex.clone()],
                logo: vec![font(SIXTYFOUR), plex],
            },
            text: Text::new(),
            bg: None,
        }
    }

    /// Whether frames of this view change with time alone (a countdown):
    /// the countdown's second, to redraw when it changes.
    pub fn clock(view: &Value, now: u64) -> Option<u64> {
        let deadline = view.get("screen")?.get("deadline")?.as_u64()?;
        Some(deadline.saturating_sub(now).div_ceil(1000))
    }

    /// Draws one frame of the screen view; `now` is the party's clock
    /// ([`Party::now`](tvpipe_party::Party::now)), for countdowns.
    pub fn render(&mut self, canvas: &mut Canvas, view: &Value, now: u64) {
        let (w, h) = (canvas.width(), canvas.height());
        self.background(canvas);
        let mut f = F {
            c: canvas,
            w: w as f32,
            h: h as f32,
            vh: h as f32 / 100.0,
            vw: w as f32 / 100.0,
            now,
        };
        let s = &view["screen"];
        let phase = s.get("phase").and_then(Value::as_str);
        match phase {
            Some("lobby") => self.lobby(&mut f, view),
            Some("draw") => self.draw(&mut f, view),
            Some("lie" | "vote") => self.guess(&mut f, view),
            Some("reveal") => self.reveal(&mut f, view),
            Some("scores" | "final") => self.board(&mut f, view),
            _ => {
                let (x, y, vh) = (f.w / 2.0, f.h / 2.0, f.vh);
                self.text(
                    &mut f,
                    "drawbluff",
                    x,
                    y,
                    t(Face::Logo, 8.0 * vh).color(ACCENT).center().middle(),
                );
            }
        }
        let has_qr = view["join"].get("qr").is_some_and(|q| !q.is_null());
        if phase.is_some_and(|p| p != "lobby") && has_qr {
            self.corner(&mut f, view);
        }
    }

    /// The radial gradient (`bg2` at the top left to `bg`), made once per size.
    fn background(&mut self, c: &mut Canvas) {
        let (w, h) = (c.width(), c.height());
        if !matches!(&self.bg, Some((bw, bh, _)) if *bw == w && *bh == h) {
            let mut data = vec![255u8; w as usize * h as usize * 4];
            let (cx, cy, r) = (w as f32 * 0.15, h as f32 * 0.1, w as f32 * 0.9);
            for y in 0..h {
                for x in 0..w {
                    let d = ((x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy) / r).min(1.0);
                    let k = (d / 0.65).min(1.0);
                    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * k).round() as u8;
                    let i = (y as usize * w as usize + x as usize) * 4;
                    data[i] = mix(BG2.r, BG.r);
                    data[i + 1] = mix(BG2.g, BG.g);
                    data[i + 2] = mix(BG2.b, BG.b);
                }
            }
            self.bg = Some((w, h, data));
        }
        if let Some((_, _, data)) = &self.bg {
            c.data_mut().copy_from_slice(data);
        }
    }

    // -----------------------------------------------------------------------
    // Text

    fn measure(&self, s: &str, o: T) -> f32 {
        self.text.measure(&strip_emoji(s), &self.fonts.style(&o))
    }

    /// One line, shortened with … to `max` px.
    fn text(&mut self, f: &mut F<'_>, s: &str, x: f32, y: f32, o: T) {
        let style = self.fonts.style(&o);
        let mut s = strip_emoji(s);
        if let Some(max) = o.max {
            // A little slack: widths measured for this very text fit.
            s = self.text.ellipsize(&s, max + 0.5, &style);
        }
        self.text.draw(f.c, &s, x, y, &style);
    }

    /// Wrapped text (at most `max_lines`); returns the height used.
    #[allow(clippy::too_many_arguments)]
    fn para(
        &mut self,
        f: &mut F<'_>,
        s: &str,
        x: f32,
        y: f32,
        width: f32,
        o: T,
        line_height: f32,
        max_lines: usize,
    ) -> f32 {
        let lines = self
            .text
            .wrap(&strip_emoji(s), width, &self.fonts.style(&o));
        let lh = o.size * line_height;
        let shown: Vec<String> = lines
            .into_iter()
            .filter(|l| !l.is_empty())
            .take(max_lines)
            .collect();
        for (i, l) in shown.iter().enumerate() {
            self.text(f, l, x, y + i as f32 * lh, o.max(width));
        }
        shown.len() as f32 * lh
    }

    // -----------------------------------------------------------------------
    // Phases

    fn lobby(&mut self, f: &mut F<'_>, view: &Value) {
        let (vh, vw) = (f.vh, f.vw);
        let join = &view["join"];
        let s = &view["screen"];
        let roster = list(&view["roster"]);
        let cx = 4.0 * vw + 21.0 * vw;
        self.text(
            f,
            "drawbluff",
            cx,
            6.0 * vh,
            t(Face::Logo, 6.5 * vh).color(ACCENT).center(),
        );
        self.text(
            f,
            "SCAN TO JOIN WITH YOUR PHONE",
            cx,
            17.0 * vh,
            t(Face::Semi, 2.4 * vh).center().color(INK.alpha(0.75)),
        );
        let q = 46.0 * vh;
        f.c.fill_rounded_rect(
            cx - q / 2.0 - vh,
            21.0 * vh,
            q + 2.0 * vh,
            q + 2.0 * vh,
            2.0 * vh,
            Color::WHITE,
        );
        qr(f.c, &join["qr"], cx - q / 2.0, 22.0 * vh, q, 2);
        let url = join["url"].as_str().unwrap_or_default();
        let o = t(Face::Sans, 2.1 * vh).center().color(INK.alpha(0.85));
        self.para(f, url, cx, 72.0 * vh, 40.0 * vw, o, 1.25, 2);
        let code: Vec<String> = join["code"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .map(String::from)
            .collect();
        self.text(
            f,
            &code.join(" "),
            cx,
            80.0 * vh,
            t(Face::Display, 3.0 * vh).color(ACCENT).center(),
        );

        let x0 = 50.0 * vw;
        let col_w = 22.0 * vw;
        self.text(f, "Players", x0, 6.0 * vh, t(Face::Display, 4.2 * vh));
        let max = s["maxPlayers"].as_u64().unwrap_or(8) as usize;
        let min = s["minPlayers"].as_u64().unwrap_or(3) as usize;
        for i in 0..max.min(12) {
            let x = x0 + (i % 2) as f32 * (col_w + 1.6 * vw);
            let y = 16.0 * vh + (i / 2) as f32 * 10.5 * vh;
            let r = roster.get(i);
            let connected = r.is_some_and(|r| r["connected"].as_bool() == Some(true));
            let fill = match r {
                Some(_) if connected => Color::WHITE.alpha(0.12),
                Some(_) => Color::WHITE.alpha(0.06),
                None => Color::WHITE.alpha(0.04),
            };
            f.c.fill_rounded_rect(x, y, col_w, 8.6 * vh, 1.6 * vh, fill);
            if let Some(r) = r {
                let vip = r["vip"].as_bool() == Some(true);
                let color = if connected { INK } else { INK.alpha(0.45) };
                let max_w = col_w - if vip { 9.0 } else { 3.0 } * vw;
                let name = r["name"].as_str().unwrap_or("?");
                self.text(
                    f,
                    name,
                    x + 1.6 * vw,
                    y + 2.4 * vh,
                    t(Face::Semi, 3.4 * vh).max(max_w).color(color),
                );
                if vip {
                    let o = t(Face::Sans, 2.4 * vh).color(ACCENT).right();
                    let right = x + col_w - 1.4 * vw;
                    self.text(f, "VIP", right, y + 3.1 * vh, o);
                    let tw = self.measure("VIP", o);
                    let size = 2.4 * vh;
                    star(
                        f.c,
                        right - tw - 0.35 * size - 0.5 * size,
                        y + 3.1 * vh + 0.45 * size,
                        0.42 * size,
                        ACCENT,
                    );
                }
            } else {
                let label = if i < min {
                    "waiting…"
                } else {
                    "room for one more"
                };
                self.text(
                    f,
                    label,
                    x + 1.6 * vw,
                    y + 2.8 * vh,
                    t(Face::Sans, 2.6 * vh).color(INK.alpha(0.3)),
                );
            }
        }
        let ready = roster
            .iter()
            .filter(|r| r["connected"].as_bool() == Some(true))
            .count();
        let vip = roster
            .iter()
            .find(|r| r["vip"].as_bool() == Some(true))
            .and_then(|r| r["name"].as_str())
            .unwrap_or("The VIP");
        let hint = if ready < min {
            let n = min - ready;
            format!("{n} more player{} needed.", if n == 1 { "" } else { "s" })
        } else {
            format!("{vip} starts the game on their phone.")
        };
        self.para(
            f,
            &hint,
            x0,
            61.0 * vh,
            46.0 * vw,
            t(Face::Sans, 3.0 * vh),
            1.3,
            9,
        );
    }

    /// Who the phase waits for: a chip per player, ✓ when done.
    fn chips(&mut self, f: &mut F<'_>, view: &Value, x: f32, y: f32, width: f32, center: bool) {
        let s = &view["screen"];
        let roster = list(&view["roster"]);
        let done: Vec<&str> = list(&s["done"]).iter().filter_map(|v| v.as_str()).collect();
        let (vh, vw) = (f.vh, f.vw);
        let o = t(Face::Sans, 2.4 * vh);
        let pad = 1.6 * vw;
        let gap = vh;
        let check = 2.4 * vh;
        let mut rows: Vec<Vec<(String, bool, f32)>> = vec![Vec::new()];
        let mut line_w = 0.0;
        for id in list(&s["waiting"]).iter().filter_map(|v| v.as_str()) {
            let name = roster
                .iter()
                .find(|r| r["id"].as_str() == Some(id))
                .and_then(|r| r["name"].as_str())
                .unwrap_or("?")
                .to_owned();
            let is_done = done.contains(&id);
            let icon = if is_done { check } else { 0.0 };
            let w = width.min(self.measure(&name, o) + icon + 2.0 * pad);
            if line_w + w > width && rows.last().is_some_and(|r| !r.is_empty()) {
                rows.push(Vec::new());
                line_w = 0.0;
            }
            if let Some(row) = rows.last_mut() {
                row.push((name, is_done, w));
            }
            line_w += w + gap;
        }
        for (ri, row) in rows.iter().enumerate() {
            let total: f32 = row.iter().map(|c| c.2 + gap).sum::<f32>() - gap;
            let mut cx = if center { x + (width - total) / 2.0 } else { x };
            let cy = y + ri as f32 * 5.6 * vh;
            for (name, is_done, w) in row {
                let fill = if *is_done {
                    OK
                } else {
                    Color::WHITE.alpha(0.1)
                };
                f.c.fill_rounded_rect(cx, cy, *w, 4.4 * vh, 2.2 * vh, fill);
                let color = if *is_done { Color::hex(0x10302d) } else { INK };
                let mut tx = cx + pad;
                if *is_done {
                    tick(f.c, tx, cy + 2.2 * vh, 0.8 * check, color);
                    tx += check;
                }
                let max = w - 2.0 * pad - if *is_done { check } else { 0.0 };
                self.text(f, name, tx, cy + 0.9 * vh, o.color(color).max(max));
                cx += w + gap;
            }
        }
    }

    fn timer(&mut self, f: &mut F<'_>, deadline: Option<u64>, cx: f32, cy: f32) {
        let Some(deadline) = deadline else {
            return;
        };
        let left = deadline.saturating_sub(f.now).div_ceil(1000);
        let r = 6.5 * f.vh;
        let fill = if left <= 10 {
            HOT
        } else {
            Color::WHITE.alpha(0.12)
        };
        f.c.fill_circle(cx, cy, r, fill);
        let size = 5.4 * f.vh;
        self.text(
            f,
            &left.to_string(),
            cx,
            cy,
            t(Face::Display, size).center().middle(),
        );
    }

    fn draw(&mut self, f: &mut F<'_>, view: &Value) {
        let s = &view["screen"];
        let (w, vh, vw) = (f.w, f.vh, f.vw);
        let round = format!("ROUND {} OF {}", num(&s["round"]), num(&s["rounds"]));
        self.text(
            f,
            &round,
            w / 2.0,
            16.0 * vh,
            t(Face::Semi, 2.4 * vh).center().color(INK.alpha(0.7)),
        );
        let o = t(Face::Display, 6.0 * vh).center();
        self.para(
            f,
            "Draw your prompt on your phone!",
            w / 2.0,
            24.0 * vh,
            70.0 * vw,
            o,
            1.3,
            9,
        );
        self.timer(f, s["deadline"].as_u64(), w / 2.0, 56.0 * vh);
        self.chips(f, view, 15.0 * vw, 68.0 * vh, 70.0 * vw, true);
    }

    /// The drawing on paper at the left; returns where the right column starts.
    fn drawing_box(&mut self, f: &mut F<'_>, drawing: &Value) -> f32 {
        let (vh, vw) = (f.vh, f.vw);
        let size = 86.0 * vh;
        let (x, y) = (4.0 * vw, 5.0 * vh);
        // A soft shadow below.
        for i in 0..4 {
            let g = (i + 1) as f32 * vh;
            let a = 0.35 / 4.0;
            f.c.fill_rounded_rect(
                x - g / 2.0,
                y + 2.0 * vh - g / 2.0,
                size + g,
                size + g,
                2.0 * vh + g / 2.0,
                Color::BLACK.alpha(a * (1.0 - i as f32 / 5.0)),
            );
        }
        f.c.fill_rounded_rect(x, y, size, size, 2.0 * vh, PAPER);
        draw_drawing(f.c, drawing, x, y, size);
        x + size + 4.0 * vw
    }

    fn guess(&mut self, f: &mut F<'_>, view: &Value) {
        let s = &view["screen"];
        let (w, vh, vw) = (f.w, f.vh, f.vw);
        let x = self.drawing_box(f, &s["drawing"]);
        let width = w - x - 4.0 * vw;
        let label = t(Face::Semi, 2.4 * vh).color(INK.alpha(0.7));
        if s["phase"].as_str() == Some("lie") {
            let artist = s["artist"]["name"].as_str().unwrap_or("Someone");
            let head = format!("{} DREW THIS", artist.to_uppercase());
            self.text(f, &head, x, 6.0 * vh, label.max(width));
            let o = t(Face::Display, 4.2 * vh);
            let used = self.para(
                f,
                "What is it called? Write a fake title on your phone.",
                x,
                11.0 * vh,
                width,
                o,
                1.35,
                9,
            );
            self.timer(
                f,
                s["deadline"].as_u64(),
                x + 6.5 * vh,
                11.0 * vh + used + 9.0 * vh,
            );
            self.chips(f, view, x, 11.0 * vh + used + 19.0 * vh, width, false);
            return;
        }
        self.text(f, "WHICH ONE IS THE REAL TITLE?", x, 6.0 * vh, label);
        let opts = list(&s["options"]);
        let oh = (9.0 * vh).min(60.0 * vh / opts.len().max(1) as f32);
        for (i, o) in opts.iter().enumerate() {
            let y = 11.0 * vh + i as f32 * (oh + 1.4 * vh);
            f.c.fill_rounded_rect(x, y, width, oh, 1.6 * vh, PAPER);
            let size = (3.4 * vh).min(oh * 0.45);
            let text = o["text"].as_str().unwrap_or_default();
            self.text(
                f,
                text,
                x + 1.6 * vw,
                y + oh / 2.0,
                t(Face::Semi, size)
                    .color(DARK)
                    .middle()
                    .max(width - 3.2 * vw),
            );
        }
        let y = (11.0 * vh + opts.len() as f32 * (oh + 1.4 * vh) + 7.0 * vh).min(87.0 * vh);
        self.timer(f, s["deadline"].as_u64(), x + 6.5 * vh, y);
        self.chips(
            f,
            view,
            x + 15.0 * vh,
            y - 2.2 * vh,
            width - 15.0 * vh,
            false,
        );
    }

    fn reveal(&mut self, f: &mut F<'_>, view: &Value) {
        let s = &view["screen"];
        let (w, vh, vw) = (f.w, f.vh, f.vw);
        let x = self.drawing_box(f, &s["drawing"]);
        let width = w - x - 4.0 * vw;
        self.text(
            f,
            "THE ANSWERS",
            x,
            6.0 * vh,
            t(Face::Semi, 2.4 * vh).color(INK.alpha(0.7)),
        );
        let items = list(&s["items"]);
        let step = s["step"].as_u64().unwrap_or(0) as usize;
        let ih = (13.0 * vh).min(78.0 * vh / items.len().max(1) as f32 - 1.4 * vh);
        let artist = s["artist"]["name"].as_str().unwrap_or("?");
        for (i, it) in items.iter().take(step).enumerate() {
            let y = 11.0 * vh + i as f32 * (ih + 1.4 * vh);
            let truth = it["truth"].as_bool() == Some(true);
            let decoy = it["decoy"].as_bool() == Some(true);
            f.c.fill_rounded_rect(
                x,
                y,
                width,
                ih,
                1.6 * vh,
                if truth { ACCENT } else { PAPER },
            );
            let names =
                |k: &str| -> Vec<&str> { list(&it[k]).iter().filter_map(|v| v.as_str()).collect() };
            let tag = if truth {
                format!("THE TRUTH by {artist}")
            } else if decoy {
                "A LIE".to_owned()
            } else {
                format!("A LIE by {}", names("authors").join(" & "))
            };
            let max = width - 3.2 * vw;
            let tag_color = if truth { DARK } else { HOT };
            self.text(
                f,
                &tag,
                x + 1.6 * vw,
                y + 1.2 * vh,
                t(Face::Display, 1.9 * vh).color(tag_color).max(max),
            );
            let size = (3.4 * vh).min(ih * 0.3);
            let text = it["text"].as_str().unwrap_or_default();
            self.text(
                f,
                text,
                x + 1.6 * vw,
                y + 4.0 * vh,
                t(Face::Semi, size).color(DARK).max(max),
            );
            let voters = names("voters");
            let who = if truth {
                if voters.is_empty() {
                    "nobody found it".to_owned()
                } else {
                    format!(
                        "found by {} · +{} each, {artist} +{}",
                        voters.join(", "),
                        num(&it["points"]),
                        num(&it["artistPoints"])
                    )
                }
            } else if decoy {
                format!("{} fell for it (our lie)", voters.join(", "))
            } else {
                format!(
                    "{} fell for it · +{}",
                    voters.join(", "),
                    num(&it["points"])
                )
            };
            self.text(
                f,
                &who,
                x + 1.6 * vw,
                y + ih - 3.6 * vh,
                t(Face::Sans, 2.2 * vh).color(GREY).max(max),
            );
        }
    }

    fn board(&mut self, f: &mut F<'_>, view: &Value) {
        let s = &view["screen"];
        let (w, vh, vw) = (f.w, f.vh, f.vw);
        let last = s["phase"].as_str() == Some("final");
        let scores = list(&s["scores"]);
        let title = if last {
            let name = scores
                .first()
                .and_then(|l| l["name"].as_str())
                .unwrap_or("Nobody");
            format!("{name} wins!")
        } else if s["last"].as_bool() == Some(true) {
            "Final scores".to_owned()
        } else {
            format!("Scores after round {}", num(&s["round"]))
        };
        self.text(
            f,
            &title,
            w / 2.0,
            8.0 * vh,
            t(Face::Display, 6.0 * vh).center().max(90.0 * vw),
        );
        let lw = 70.0 * vw;
        let x = (w - lw) / 2.0;
        let lh = (8.4 * vh).min(66.0 * vh / scores.len().max(1) as f32);
        for (i, l) in scores.iter().enumerate() {
            let y = 20.0 * vh + i as f32 * (lh + 1.2 * vh);
            let first = i == 0;
            f.c.fill_rounded_rect(
                x,
                y,
                lw,
                lh,
                1.6 * vh,
                if first {
                    ACCENT
                } else {
                    Color::WHITE.alpha(0.08)
                },
            );
            let color = if first { DARK } else { INK };
            let mid = y + lh / 2.0;
            self.text(
                f,
                &format!("{}.", i + 1),
                x + 2.0 * vw,
                mid,
                t(Face::Display, 3.0 * vh).color(color).middle(),
            );
            let name = l["name"].as_str().unwrap_or("?");
            self.text(
                f,
                name,
                x + 7.0 * vw,
                mid,
                t(Face::Semi, 3.6 * vh).color(color).middle().max(36.0 * vw),
            );
            let delta = l["delta"].as_i64().unwrap_or(0);
            if !last && delta != 0 {
                let c = if first { DARK } else { OK };
                self.text(
                    f,
                    &format!("+{delta}"),
                    x + lw - 14.0 * vw,
                    mid,
                    t(Face::Sans, 2.6 * vh).color(c).middle().right(),
                );
            }
            self.text(
                f,
                &num(&l["score"]),
                x + lw - 2.0 * vw,
                mid,
                t(Face::Display, 3.4 * vh).color(color).middle().right(),
            );
        }
        if last {
            let o = t(Face::Sans, 3.0 * vh).center();
            self.text(
                f,
                "The VIP can start another game on their phone.",
                w / 2.0,
                92.0 * vh,
                o,
            );
        }
    }

    /// The rejoin QR code in the bottom right corner.
    fn corner(&mut self, f: &mut F<'_>, view: &Value) {
        let (w, h, vh, vw) = (f.w, f.h, f.vh, f.vw);
        let size = 12.0 * vh;
        let bw = size + 16.0 * vw;
        let x = w - 2.0 * vw - bw;
        let y = h - 3.0 * vh - size - 2.0 * vh;
        f.c.fill_rounded_rect(
            x,
            y,
            bw,
            size + 2.0 * vh,
            1.4 * vh,
            Color::BLACK.alpha(0.35),
        );
        qr(f.c, &view["join"]["qr"], x + vh, y + vh, size, 2);
        let o = t(Face::Sans, 1.8 * vh).color(INK.alpha(0.85));
        self.para(
            f,
            "Lost your phone's page? Scan to rejoin.",
            x + size + 2.0 * vh,
            y + 2.0 * vh,
            bw - size - 3.0 * vh,
            o,
            1.25,
            9,
        );
    }
}

fn list(v: &Value) -> &[Value] {
    v.as_array().unwrap_or(&[])
}

/// A number as JavaScript prints it (`?` when missing).
fn num(v: &Value) -> String {
    match v {
        Value::Number(n) => n.to_string(),
        _ => "?".to_owned(),
    }
}

/// A QR code (`{size, rows}` of the join info) in a `size` px square with
/// `quiet` modules of margin: dark runs as rectangles snapped to pixels
/// (`drawQr` of `@tvpipe/party`).
fn qr(c: &mut Canvas, qr: &Value, x: f32, y: f32, size: f32, quiet: usize) {
    let Some(n) = qr["size"].as_u64() else {
        return;
    };
    let px = size / (n as f32 + 2.0 * quiet as f32);
    c.fill_rect(x, y, size, size, Color::WHITE);
    for (r, row) in list(&qr["rows"]).iter().enumerate() {
        let row = row.as_str().unwrap_or_default().as_bytes();
        let mut col = 0;
        while col < row.len() {
            if row[col] != b'1' {
                col += 1;
                continue;
            }
            let mut w = 1;
            while row.get(col + w) == Some(&b'1') {
                w += 1;
            }
            let x0 = (x + (col + quiet) as f32 * px).round();
            let x1 = (x + (col + w + quiet) as f32 * px).round();
            let y0 = (y + (r + quiet) as f32 * px).round();
            let y1 = (y + (r + 1 + quiet) as f32 * px).round();
            c.fill_rect(x0, y0, x1 - x0, y1 - y0, Color::BLACK);
            col += w;
        }
    }
}

/// A drawing (`{strokes}`) on a `size` px square at (x, y), clipped to it
/// (`drawDrawing` of `web/phone/strokes.js`, without the paper).
pub fn draw_drawing(c: &mut Canvas, drawing: &Value, x: f32, y: f32, size: f32) {
    let old = c.clip();
    let inner = IRect::new(
        x.ceil() as i32,
        y.ceil() as i32,
        size.floor() as i32,
        size.floor() as i32,
    );
    c.set_clip(Some(inner));
    let k = size / GRID as f32;
    let mut points = Vec::new();
    for s in list(&drawing["strokes"]) {
        let s: Vec<f32> = list(s)
            .iter()
            .filter_map(Value::as_f64)
            .map(|v| v as f32)
            .collect();
        if s.len() < 4 {
            continue;
        }
        let color = PALETTE
            .get(s[0] as usize)
            .map_or(DARK, |&rgb| Color::hex(rgb));
        let width = (s[1] * k).max(1.0);
        if s.len() == 4 {
            c.fill_circle(x + s[2] * k, y + s[3] * k, (width / 2.0).max(0.5), color);
            continue;
        }
        points.clear();
        points.extend(s[2..].chunks_exact(2).map(|p| (x + p[0] * k, y + p[1] * k)));
        c.stroke_polyline(&points, width, color);
    }
    c.set_clip(Some(old));
}

/// A five-pointed star (★) centred at (cx, cy).
fn star(c: &mut Canvas, cx: f32, cy: f32, r: f32, color: Color) {
    let mut p = tvpipe_canvas::Path::new();
    for i in 0..10 {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        let rr = if i % 2 == 0 { r } else { r * 0.45 };
        let (px, py) = (cx + a.cos() * rr, cy + a.sin() * rr);
        if i == 0 {
            p.move_to(px, py);
        } else {
            p.line_to(px, py);
        }
    }
    p.close();
    c.fill_path(&p, color);
}

/// A check mark (✓) starting at x, centred on cy, `w` wide.
fn tick(c: &mut Canvas, x: f32, cy: f32, w: f32, color: Color) {
    let pts = [
        (x + 0.1 * w, cy),
        (x + 0.38 * w, cy + 0.28 * w),
        (x + 0.9 * w, cy - 0.32 * w),
    ];
    c.stroke_polyline(&pts, (0.16 * w).max(1.5), color);
}
