//! The canvas: an RGBA8 buffer (straight alpha) with a clip rectangle,
//! and source-over drawing.

use crate::path::Path;
use crate::raster::{IRect, Mask, Raster};
use crate::stroke;

/// A colour, straight (not premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Opacity (255: opaque).
    pub a: u8,
}

impl Color {
    /// Transparent black.
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);
    /// Opaque black.
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    /// Opaque white.
    pub const WHITE: Color = Color::rgb(255, 255, 255);

    /// An opaque colour.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    /// A colour with opacity.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r, g, b, a }
    }

    /// An opaque colour from `0xRRGGBB`.
    pub const fn hex(rgb: u32) -> Color {
        Color::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    }

    /// The same colour with its opacity scaled by `f` (0.0..=1.0).
    pub fn alpha(self, f: f32) -> Color {
        let a = (self.a as f32 * f.clamp(0.0, 1.0)).round() as u8;
        Color { a, ..self }
    }
}

/// An RGBA8 picture (straight alpha, rows top to bottom) with a clip
/// rectangle. Draws source-over, colours as stored (sRGB values blended
/// linearly, as browsers' canvases do).
#[derive(Clone)]
pub struct Canvas {
    width: u32,
    height: u32,
    data: Vec<u8>,
    clip: IRect,
}

impl std::fmt::Debug for Canvas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Canvas")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("clip", &self.clip)
            .finish_non_exhaustive()
    }
}

/// Composites `c` (scaled by coverage `cov`, 0..=255) over the pixel.
#[inline]
fn blend(px: &mut [u8], c: Color, cov: u32) {
    // Source alpha 0..=255·255.
    let sa = c.a as u32 * cov;
    if sa == 0 {
        return;
    }
    if sa >= 255 * 255 {
        px[0] = c.r;
        px[1] = c.g;
        px[2] = c.b;
        px[3] = 255;
        return;
    }
    let da = px[3] as u32;
    if da == 255 {
        // Over an opaque pixel: out = src·sa + dst·(1 − sa).
        let inv = 255 * 255 - sa;
        for (i, s) in [c.r, c.g, c.b].into_iter().enumerate() {
            px[i] = ((s as u32 * sa + px[i] as u32 * inv + 32512) / 65025) as u8;
        }
        return;
    }
    // General case in floats: out_a = sa + da(1 − sa).
    let sa = sa as f32 / 65025.0;
    let da = da as f32 / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        return;
    }
    for (i, s) in [c.r, c.g, c.b].into_iter().enumerate() {
        let v = (s as f32 * sa + px[i] as f32 * da * (1.0 - sa)) / oa;
        px[i] = v.round().clamp(0.0, 255.0) as u8;
    }
    px[3] = (oa * 255.0).round().clamp(0.0, 255.0) as u8;
}

impl Canvas {
    /// A transparent canvas. Sizes are capped at 16384 per side.
    pub fn new(width: u32, height: u32) -> Canvas {
        let (width, height) = (width.min(16384), height.min(16384));
        Canvas {
            width,
            height,
            data: vec![0; width as usize * height as usize * 4],
            clip: IRect::new(0, 0, width as i32, height as i32),
        }
    }

    /// A canvas over existing RGBA pixels (`None` when the length is not
    /// `width × height × 4`).
    pub fn from_rgba(width: u32, height: u32, data: Vec<u8>) -> Option<Canvas> {
        if width > 16384 || height > 16384 || data.len() != width as usize * height as usize * 4 {
            return None;
        }
        Some(Canvas {
            width,
            height,
            data,
            clip: IRect::new(0, 0, width as i32, height as i32),
        })
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixels: RGBA, row by row.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The pixels, writable.
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// The pixel buffer.
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// One pixel (transparent outside).
    pub fn pixel(&self, x: i32, y: i32) -> Color {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return Color::TRANSPARENT;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = &self.data[i..i + 4];
        Color::rgba(p[0], p[1], p[2], p[3])
    }

    /// The whole canvas as a rectangle.
    pub fn bounds(&self) -> IRect {
        IRect::new(0, 0, self.width as i32, self.height as i32)
    }

    /// Limits drawing to a rectangle (`None`: the whole canvas).
    pub fn set_clip(&mut self, clip: Option<IRect>) {
        self.clip = match clip {
            Some(c) => c.intersect(&self.bounds()),
            None => self.bounds(),
        };
    }

    /// The clip rectangle.
    pub fn clip(&self) -> IRect {
        self.clip
    }

    /// Sets every pixel (ignores the clip).
    pub fn clear(&mut self, c: Color) {
        for px in self.data.chunks_exact_mut(4) {
            px.copy_from_slice(&[c.r, c.g, c.b, c.a]);
        }
    }

    /// Replaces a rectangle of whole pixels with a colour (no blending).
    pub fn set_rect(&mut self, r: IRect, c: Color) {
        let r = r.intersect(&self.clip);
        for y in r.y..r.bottom() {
            let row = y as usize * self.width as usize;
            for x in r.x..r.right() {
                let i = (row + x as usize) * 4;
                self.data[i..i + 4].copy_from_slice(&[c.r, c.g, c.b, c.a]);
            }
        }
    }

    /// Composites a coverage mask in a colour.
    pub(crate) fn fill_mask(&mut self, m: &Mask, c: Color) {
        let r = m.rect.intersect(&self.clip);
        let stride = m.rect.w as usize;
        for y in r.y..r.bottom() {
            let mrow = (y - m.rect.y) as usize * stride;
            let row = y as usize * self.width as usize;
            for x in r.x..r.right() {
                let cov = m.cov[mrow + (x - m.rect.x) as usize];
                let cov = (cov * 255.0 + 0.5) as u32;
                if cov > 0 {
                    let i = (row + x as usize) * 4;
                    blend(&mut self.data[i..i + 4], c, cov.min(255));
                }
            }
        }
    }

    /// Composites 8-bit coverage (`w × h`, at `x`, `y`) in a colour.
    pub(crate) fn fill_alpha(&mut self, x: i32, y: i32, w: i32, h: i32, alpha: &[u8], c: Color) {
        let m = IRect::new(x, y, w, h);
        let r = m.intersect(&self.clip);
        for py in r.y..r.bottom() {
            let mrow = (py - y) as usize * w as usize;
            let row = py as usize * self.width as usize;
            for px in r.x..r.right() {
                let cov = alpha[mrow + (px - x) as usize] as u32;
                if cov > 0 {
                    let i = (row + px as usize) * 4;
                    blend(&mut self.data[i..i + 4], c, cov);
                }
            }
        }
    }

    /// Fills a path (non-zero winding, anti-aliased).
    pub fn fill_path(&mut self, path: &Path, c: Color) {
        let Some((x0, y0, x1, y1)) = path.bounds() else {
            return;
        };
        let rect = IRect::around(x0, y0, x1, y1).intersect(&self.clip);
        if rect.is_empty() || c.a == 0 {
            return;
        }
        let mut raster = Raster::new(rect);
        for poly in path.flatten() {
            let p = &poly.points;
            for i in 0..p.len() {
                raster.line(p[i], p[(i + 1) % p.len()]);
            }
        }
        self.fill_mask(&raster.finish(), c);
    }

    /// Strokes a path with round caps and joins.
    pub fn stroke_path(&mut self, path: &Path, width: f32, c: Color) {
        for poly in path.flatten() {
            if let Some(m) = stroke::polyline(&poly.points, poly.closed, width, self.clip) {
                self.fill_mask(&m, c);
            }
        }
    }

    /// Strokes a polyline with round caps and joins (one point: a dot).
    pub fn stroke_polyline(&mut self, points: &[(f32, f32)], width: f32, c: Color) {
        if let Some(m) = stroke::polyline(points, false, width, self.clip) {
            self.fill_mask(&m, c);
        }
    }

    /// Fills a rectangle (anti-aliased edges when they fall between pixels).
    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Color) {
        let whole = |v: f32| v.fract() == 0.0 && v.abs() < 1e9;
        if whole(x) && whole(y) && whole(w) && whole(h) && w >= 0.0 && h >= 0.0 {
            let r = IRect::new(x as i32, y as i32, w as i32, h as i32).intersect(&self.clip);
            if c.a == 255 {
                return self.set_rect(r, c);
            }
            for py in r.y..r.bottom() {
                let row = py as usize * self.width as usize;
                for px in r.x..r.right() {
                    let i = (row + px as usize) * 4;
                    blend(&mut self.data[i..i + 4], c, 255);
                }
            }
            return;
        }
        let mut p = Path::new();
        p.rect(x, y, w, h);
        self.fill_path(&p, c);
    }

    /// Fills a rectangle with rounded corners.
    pub fn fill_rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: Color) {
        let mut p = Path::new();
        p.rounded_rect(x, y, w, h, r);
        self.fill_path(&p, c);
    }

    /// Fills a circle.
    pub fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, c: Color) {
        let mut p = Path::new();
        p.circle(cx, cy, r);
        self.fill_path(&p, c);
    }

    /// Draws RGBA pixels (straight alpha, `w × h`) at whole-pixel `x`, `y`,
    /// source-over, scaled by `opacity`.
    pub fn draw_rgba(&mut self, x: i32, y: i32, w: u32, h: u32, rgba: &[u8], opacity: f32) {
        if rgba.len() < w as usize * h as usize * 4 {
            return;
        }
        let o = (opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
        let r = IRect::new(x, y, w as i32, h as i32).intersect(&self.clip);
        for py in r.y..r.bottom() {
            let srow = (py - y) as usize * w as usize;
            let row = py as usize * self.width as usize;
            for px in r.x..r.right() {
                let s = (srow + (px - x) as usize) * 4;
                let c = Color::rgba(rgba[s], rgba[s + 1], rgba[s + 2], rgba[s + 3]);
                let i = (row + px as usize) * 4;
                blend(&mut self.data[i..i + 4], c, o);
            }
        }
    }

    /// Draws another canvas at whole-pixel `x`, `y`.
    pub fn draw_canvas(&mut self, other: &Canvas, x: i32, y: i32, opacity: f32) {
        self.draw_rgba(x, y, other.width, other.height, &other.data, opacity);
    }

    /// Draws RGBA pixels (`w × h`) scaled into the rectangle `dx, dy, dw,
    /// dh` (bilinear; whole pixels of the destination).
    pub fn draw_rgba_scaled(&mut self, w: u32, h: u32, rgba: &[u8], dst: IRect) {
        if w == 0 || h == 0 || dst.is_empty() || rgba.len() < w as usize * h as usize * 4 {
            return;
        }
        let r = dst.intersect(&self.clip);
        let sx = w as f32 / dst.w as f32;
        let sy = h as f32 / dst.h as f32;
        let at = |x: i32, y: i32, ch: usize| -> f32 {
            let x = x.clamp(0, w as i32 - 1) as usize;
            let y = y.clamp(0, h as i32 - 1) as usize;
            rgba[(y * w as usize + x) * 4 + ch] as f32
        };
        for py in r.y..r.bottom() {
            let fy = (py - dst.y) as f32 * sy + 0.5 * sy - 0.5;
            let y0 = fy.floor();
            let ty = fy - y0;
            let row = py as usize * self.width as usize;
            for px in r.x..r.right() {
                let fx = (px - dst.x) as f32 * sx + 0.5 * sx - 0.5;
                let x0 = fx.floor();
                let tx = fx - x0;
                let (xi, yi) = (x0 as i32, y0 as i32);
                let mut v = [0u8; 4];
                // Premultiplied interpolation, so transparent pixels do
                // not bleed their colour.
                let mut acc = [0.0f32; 4];
                for (dx, dy, wgt) in [
                    (0, 0, (1.0 - tx) * (1.0 - ty)),
                    (1, 0, tx * (1.0 - ty)),
                    (0, 1, (1.0 - tx) * ty),
                    (1, 1, tx * ty),
                ] {
                    let a = at(xi + dx, yi + dy, 3) * wgt;
                    acc[3] += a;
                    for (ch, slot) in acc.iter_mut().enumerate().take(3) {
                        *slot += at(xi + dx, yi + dy, ch) * a;
                    }
                }
                if acc[3] > 0.0 {
                    for ch in 0..3 {
                        v[ch] = (acc[ch] / acc[3]).round().clamp(0.0, 255.0) as u8;
                    }
                    v[3] = acc[3].round().clamp(0.0, 255.0) as u8;
                    let i = (row + px as usize) * 4;
                    blend(
                        &mut self.data[i..i + 4],
                        Color::rgba(v[0], v[1], v[2], v[3]),
                        255,
                    );
                }
            }
        }
    }

    /// Draws a QR code (or any grid of modules): `modules` per side,
    /// `dark(x, y)` for each, `scale` pixels per module, `quiet` modules of
    /// light margin, top left at `x`, `y`. Returns its size in pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_qr(
        &mut self,
        modules: usize,
        dark: impl Fn(usize, usize) -> bool,
        x: i32,
        y: i32,
        scale: u32,
        quiet: u32,
        fg: Color,
        bg: Color,
    ) -> u32 {
        let s = scale.max(1) as i32;
        let n = modules.min(1000) as i32;
        let total = (n + 2 * quiet as i32) * s;
        self.fill_rect(x as f32, y as f32, total as f32, total as f32, bg);
        let x0 = x + quiet as i32 * s;
        let y0 = y + quiet as i32 * s;
        for my in 0..n {
            for mx in 0..n {
                if dark(mx as usize, my as usize) {
                    self.fill_rect(
                        (x0 + mx * s) as f32,
                        (y0 + my * s) as f32,
                        s as f32,
                        s as f32,
                        fg,
                    );
                }
            }
        }
        total as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blending() {
        let mut px = [0, 0, 0, 255];
        blend(&mut px, Color::rgba(255, 255, 255, 128), 255);
        assert_eq!(px, [128, 128, 128, 255]);
        let mut px = [10, 20, 30, 255];
        blend(&mut px, Color::rgb(200, 100, 0), 255);
        assert_eq!(px, [200, 100, 0, 255]);
        // Over transparent: the source as is.
        let mut px = [0, 0, 0, 0];
        blend(&mut px, Color::rgba(200, 100, 50, 100), 255);
        assert_eq!(px, [200, 100, 50, 100]);
        // Half coverage of opaque white on black.
        let mut px = [0, 0, 0, 255];
        blend(&mut px, Color::WHITE, 128);
        assert_eq!(px, [128, 128, 128, 255]);
    }

    #[test]
    fn rects_and_clip() {
        let mut c = Canvas::new(10, 10);
        c.clear(Color::BLACK);
        c.set_clip(Some(IRect::new(0, 0, 5, 10)));
        c.fill_rect(2.0, 2.0, 6.0, 2.0, Color::WHITE);
        assert_eq!(c.pixel(4, 2), Color::WHITE);
        assert_eq!(c.pixel(5, 2), Color::BLACK);
        assert_eq!(c.pixel(1, 2), Color::BLACK);
        c.set_clip(None);
        // Half a pixel wide: half grey.
        c.fill_rect(7.5, 5.0, 1.0, 1.0, Color::WHITE);
        assert_eq!(c.pixel(7, 5), Color::rgb(128, 128, 128));
        assert_eq!(c.pixel(8, 5), Color::rgb(128, 128, 128));
        // Off canvas: nothing, no panic.
        c.fill_rect(-100.0, -100.0, 5.0, 5.0, Color::WHITE);
        c.fill_rect(1e30, 0.0, 1e30, 5.0, Color::WHITE);
        c.fill_rect(f32::NAN, 0.0, 5.0, 5.0, Color::WHITE);
    }

    #[test]
    fn circle_area() {
        let mut c = Canvas::new(200, 200);
        c.clear(Color::BLACK);
        let r = 61.3;
        c.fill_circle(100.2, 99.7, r, Color::WHITE);
        let sum: f64 = c.data().chunks(4).map(|p| p[0] as f64 / 255.0).sum();
        let want = std::f64::consts::PI * (r as f64) * (r as f64);
        assert!((sum - want).abs() / want < 0.005, "{sum} vs {want}");
    }

    #[test]
    fn rounded_rect_area() {
        let mut c = Canvas::new(100, 100);
        c.clear(Color::BLACK);
        c.fill_rounded_rect(10.0, 10.0, 80.0, 40.0, 10.0, Color::WHITE);
        let sum: f64 = c.data().chunks(4).map(|p| p[0] as f64 / 255.0).sum();
        let want = 80.0 * 40.0 - (4.0 - std::f64::consts::PI) * 100.0;
        assert!((sum - want).abs() / want < 0.005, "{sum} vs {want}");
    }

    #[test]
    fn scaled_images() {
        let src = [255u8, 0, 0, 255, 0, 0, 255, 255];
        let mut c = Canvas::new(8, 2);
        c.draw_rgba_scaled(2, 1, &src, IRect::new(0, 0, 8, 2));
        assert_eq!(c.pixel(0, 0), Color::rgb(255, 0, 0));
        assert_eq!(c.pixel(7, 1), Color::rgb(0, 0, 255));
        let mid = c.pixel(4, 0);
        assert!(mid.r > 64 && mid.b > 64, "{mid:?}");
        // Too short a buffer: nothing.
        c.draw_rgba_scaled(4, 4, &src, IRect::new(0, 0, 8, 2));
        c.draw_rgba(0, 0, 4, 4, &src, 1.0);
    }

    #[test]
    fn qr_modules_on_whole_pixels() {
        let mut c = Canvas::new(40, 40);
        let size = c.draw_qr(3, |x, y| x == y, 2, 2, 4, 1, Color::BLACK, Color::WHITE);
        assert_eq!(size, 20);
        assert_eq!(c.pixel(2, 2), Color::WHITE);
        assert_eq!(c.pixel(6, 6), Color::BLACK);
        assert_eq!(c.pixel(9, 9), Color::BLACK);
        assert_eq!(c.pixel(10, 6), Color::WHITE);
        assert_eq!(c.pixel(21, 21), Color::WHITE);
        assert_eq!(c.pixel(22, 22), Color::TRANSPARENT);
    }
}
