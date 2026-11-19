//! Text: left-to-right layout with kerning, measuring, alignment, word
//! wrap, ellipsis, font fallback; glyphs rasterised once per size and
//! quarter-pixel offset and cached. No shaping (no ligatures, no
//! right-to-left or Indic scripts). Emoji no font has are left out (the
//! TV draws names without them), other missing characters are the first
//! font's missing-glyph box.

use std::collections::HashMap;
use std::sync::Arc;

use crate::canvas::{Canvas, Color};
use crate::font::Font;
use crate::raster::{IRect, Raster};

/// Horizontal alignment at the `x` given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    /// Text starts at `x`.
    #[default]
    Left,
    /// Text is centred on `x`.
    Center,
    /// Text ends at `x`.
    Right,
}

/// What `y` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Baseline {
    /// The alphabetic baseline.
    #[default]
    Alphabetic,
    /// The top of the ascender.
    Top,
    /// Halfway between ascender and descender.
    Middle,
    /// The bottom of the descender.
    Bottom,
}

/// How text is drawn.
#[derive(Debug, Clone)]
pub struct TextStyle<'a> {
    /// Fonts in order of preference (the first that has a character draws it).
    pub fonts: &'a [Font],
    /// Pixels per em.
    pub size: f32,
    /// The colour.
    pub color: Color,
    /// Horizontal alignment.
    pub align: Align,
    /// Vertical reference.
    pub baseline: Baseline,
    /// Extra pixels after each character.
    pub letter_spacing: f32,
    /// Pixels from one line's baseline to the next (`None`: the font's).
    pub leading: Option<f32>,
}

impl<'a> TextStyle<'a> {
    /// White, left-aligned, on the alphabetic baseline, in one font.
    pub fn new(font: &'a Font, size: f32) -> TextStyle<'a> {
        TextStyle::with_fonts(std::slice::from_ref(font), size)
    }

    /// The same with fallback fonts.
    pub fn with_fonts(fonts: &'a [Font], size: f32) -> TextStyle<'a> {
        TextStyle {
            fonts,
            size,
            color: Color::WHITE,
            align: Align::Left,
            baseline: Baseline::Alphabetic,
            letter_spacing: 0.0,
            leading: None,
        }
    }

    /// In this colour.
    pub fn color(mut self, c: Color) -> Self {
        self.color = c;
        self
    }

    /// Aligned so.
    pub fn align(mut self, a: Align) -> Self {
        self.align = a;
        self
    }

    /// With `y` meaning this.
    pub fn baseline(mut self, b: Baseline) -> Self {
        self.baseline = b;
        self
    }

    /// With extra space after each character.
    pub fn letter_spacing(mut self, px: f32) -> Self {
        self.letter_spacing = px;
        self
    }

    /// Lines this far apart (pixels, baseline to baseline).
    pub fn leading(mut self, px: f32) -> Self {
        self.leading = Some(px);
        self
    }

    fn scale(&self, f: &Font) -> f32 {
        self.size / f.units_per_em() as f32
    }

    /// The first font's ascent and descent in pixels (both positive).
    pub fn metrics(&self) -> (f32, f32) {
        match self.fonts.first() {
            Some(f) => {
                let s = self.scale(f);
                (f.ascender() as f32 * s, -(f.descender() as f32) * s)
            }
            None => (0.0, 0.0),
        }
    }

    /// The distance between lines in pixels: the leading set, else the
    /// font's ascent + descent + gap.
    pub fn line_height(&self) -> f32 {
        if let Some(l) = self.leading {
            return l;
        }
        match self.fonts.first() {
            Some(f) => {
                let s = self.scale(f);
                (f.ascender() as f32 - f.descender() as f32 + f.line_gap() as f32) * s
            }
            None => self.size,
        }
    }
}

/// Whether a character is part of an emoji (pictographs, flags, skin
/// tones, joiners, variation selectors, tags, keycaps).
pub fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1FAFF
        | 0x2600..=0x27BF
        | 0x2B00..=0x2BFF
        | 0x2300..=0x23FF
        | 0x200D
        | 0x20E3
        | 0xFE00..=0xFE0F
        | 0xE0000..=0xE007F
        | 0x3030 | 0x303D | 0x3297 | 0x3299)
}

/// The text without emoji, spaces they leave behind collapsed and the
/// ends trimmed (`"🎉 Ann 🎉"` → `"Ann"`).
pub fn strip_emoji(text: &str) -> String {
    let kept: String = text.chars().filter(|&c| !is_emoji(c)).collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    font: u64,
    glyph: u16,
    /// Pixels per em × 64.
    size: u32,
    /// Quarter pixels of horizontal offset, 0..4.
    sub: u8,
}

struct Bitmap {
    left: i32,
    top: i32,
    w: i32,
    h: i32,
    alpha: Vec<u8>,
}

/// One glyph placed by the layout.
#[derive(Clone)]
struct Placed {
    font: usize,
    glyph: u16,
    x: f32,
}

/// The glyph cache and layout. Keep one for the program (or the render
/// thread): glyphs are rasterised once per font, size and quarter-pixel
/// offset.
#[derive(Default)]
pub struct Text {
    cache: HashMap<Key, Option<Arc<Bitmap>>>,
}

impl std::fmt::Debug for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Text")
            .field("cached", &self.cache.len())
            .finish()
    }
}

/// Most cached glyphs; the cache starts over past it.
const MAX_CACHED: usize = 8192;

impl Text {
    /// An empty cache.
    pub fn new() -> Text {
        Text::default()
    }

    /// Glyphs and their pen positions (from 0), and the advance.
    fn layout(&self, text: &str, style: &TextStyle<'_>) -> (Vec<Placed>, f32) {
        let mut out = Vec::new();
        let mut x = 0.0;
        let mut prev: Option<(usize, u16)> = None;
        if style.fonts.is_empty() || !(style.size.is_finite() && style.size > 0.0) {
            return (out, 0.0);
        }
        for c in text.chars() {
            if c == '\n' || c == '\r' || c == '\t' {
                // One line at a time: control characters are spaces.
                if let Some(g) = style.fonts[0].glyph_index(' ') {
                    x += style.fonts[0].advance(g) as f32 * style.scale(&style.fonts[0]);
                }
                prev = None;
                continue;
            }
            let found = style
                .fonts
                .iter()
                .enumerate()
                .find_map(|(i, f)| f.glyph_index(c).map(|g| (i, g)));
            let (fi, g) = match found {
                Some(fg) => fg,
                None if is_emoji(c) || c.is_control() => continue,
                None => (0, 0),
            };
            let font = &style.fonts[fi];
            let s = style.scale(font);
            if let Some((pf, pg)) = prev {
                if pf == fi {
                    x += font.kerning(pg, g) as f32 * s;
                }
            }
            out.push(Placed {
                font: fi,
                glyph: g,
                x,
            });
            x += font.advance(g) as f32 * s + style.letter_spacing;
            prev = Some((fi, g));
        }
        (out, x)
    }

    /// The advance width of a line of text, in pixels.
    pub fn measure(&self, text: &str, style: &TextStyle<'_>) -> f32 {
        self.layout(text, style).1
    }

    /// The text cut to `max` pixels wide, with `…` when it was cut.
    pub fn ellipsize(&self, text: &str, max: f32, style: &TextStyle<'_>) -> String {
        if self.measure(text, style) <= max {
            return text.to_owned();
        }
        let chars: Vec<char> = text.chars().collect();
        let (mut lo, mut hi) = (0, chars.len());
        // The longest prefix that fits with the ellipsis.
        while lo < hi {
            let m = (lo + hi).div_ceil(2);
            let s: String = chars[..m].iter().collect::<String>().trim_end().to_owned() + "…";
            if self.measure(&s, style) <= max {
                lo = m;
            } else {
                hi = m - 1;
            }
        }
        let head: String = chars[..lo].iter().collect();
        format!("{}…", head.trim_end())
    }

    /// Words wrapped into lines at most `max` pixels wide (a word longer
    /// than that is broken between characters).
    pub fn wrap(&self, text: &str, max: f32, style: &TextStyle<'_>) -> Vec<String> {
        let mut lines = Vec::new();
        for para in text.split('\n') {
            let mut line = String::new();
            for word in para.split_whitespace() {
                let candidate = if line.is_empty() {
                    word.to_owned()
                } else {
                    format!("{line} {word}")
                };
                if self.measure(&candidate, style) <= max {
                    line = candidate;
                    continue;
                }
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                // The word alone, broken if it must be.
                let mut part = String::new();
                for c in word.chars() {
                    part.push(c);
                    if self.measure(&part, style) > max && part.chars().count() > 1 {
                        part.pop();
                        lines.push(std::mem::take(&mut part));
                        part.push(c);
                    }
                }
                line = part;
            }
            lines.push(line);
        }
        lines
    }

    fn bitmap(&mut self, font: &Font, glyph: u16, size: f32, sub: u8) -> Option<Arc<Bitmap>> {
        let key = Key {
            font: font.id(),
            glyph,
            size: (size * 64.0).round() as u32,
            sub,
        };
        if let Some(b) = self.cache.get(&key) {
            return b.clone();
        }
        if self.cache.len() >= MAX_CACHED {
            self.cache.clear();
        }
        let b = rasterize(font, glyph, key.size as f32 / 64.0, sub as f32 / 4.0).map(Arc::new);
        self.cache.insert(key, b.clone());
        b
    }

    /// Draws one line of text; returns its advance width.
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        x: f32,
        y: f32,
        style: &TextStyle<'_>,
    ) -> f32 {
        let (glyphs, width) = self.layout(text, style);
        let x = match style.align {
            Align::Left => x,
            Align::Center => x - width / 2.0,
            Align::Right => x - width,
        };
        let (ascent, descent) = style.metrics();
        let y = match style.baseline {
            Baseline::Alphabetic => y,
            Baseline::Top => y + ascent,
            Baseline::Middle => y + (ascent - descent) / 2.0,
            Baseline::Bottom => y - descent,
        };
        if !(x.is_finite() && y.is_finite()) {
            return width;
        }
        let base = y.round() as i32;
        for p in glyphs {
            let font = &style.fonts[p.font];
            let px = x + p.x;
            let whole = px.floor();
            let sub = (((px - whole) * 4.0).round() as u8).min(3);
            let (whole, sub) = if ((px - whole) * 4.0).round() >= 4.0 {
                (whole + 1.0, 0)
            } else {
                (whole, sub)
            };
            if let Some(b) = self.bitmap(font, p.glyph, style.size, sub) {
                let gx = whole as i32 + b.left;
                let gy = base + b.top;
                canvas.fill_alpha(gx, gy, b.w, b.h, &b.alpha, style.color);
            }
        }
        width
    }

    /// Draws text wrapped to `max` pixels, from `y` down (`y` as the
    /// style's baseline says, for the first line), the style's
    /// [`line_height`](TextStyle::line_height) apart. Returns the number
    /// of lines.
    pub fn draw_wrapped(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        (x, y): (f32, f32),
        max: f32,
        style: &TextStyle<'_>,
    ) -> usize {
        let lh = style.line_height();
        let lines = self.wrap(text, max, style);
        for (i, l) in lines.iter().enumerate() {
            self.draw(canvas, l, x, y + i as f32 * lh, style);
        }
        lines.len()
    }
}

/// A glyph's coverage at `size` px/em, shifted right by `dx` (0..1).
fn rasterize(font: &Font, glyph: u16, size: f32, dx: f32) -> Option<Bitmap> {
    let outline = font.outline(glyph).ok()??;
    let s = size / font.units_per_em() as f32;
    // y up in the font, down on the canvas; the pen at (dx, 0).
    let path = outline.transformed(s, -s, dx, 0.0);
    let (x0, y0, x1, y1) = path.bounds()?;
    let rect = IRect::around(x0, y0, x1, y1);
    if rect.is_empty() || rect.w > 4096 || rect.h > 4096 {
        return None;
    }
    let mut raster = Raster::new(rect);
    for poly in path.flatten() {
        let p = &poly.points;
        for i in 0..p.len() {
            raster.line(p[i], p[(i + 1) % p.len()]);
        }
    }
    let m = raster.finish();
    Some(Bitmap {
        left: rect.x,
        top: rect.y,
        w: rect.w,
        h: rect.h,
        alpha: m.cov.iter().map(|&c| (c * 255.0 + 0.5) as u8).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji() {
        assert_eq!(strip_emoji("🎉 Ann 🎉"), "Ann");
        assert_eq!(strip_emoji("Bob👍🏽"), "Bob");
        assert_eq!(strip_emoji("👨‍👩‍👧 Fam"), "Fam");
        assert_eq!(strip_emoji("🇩🇪 Jö"), "Jö");
        assert_eq!(strip_emoji("❤️ x"), "x");
        assert_eq!(strip_emoji("Zoë – ok"), "Zoë – ok");
    }
}
