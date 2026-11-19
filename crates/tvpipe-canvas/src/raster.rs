//! The coverage accumulator: lines add their signed area to a buffer, a
//! running sum along each row turns it into coverage (the technique of
//! Raph Levien's font-rs). Exact anti-aliasing for polygons, non-zero
//! winding, one pass per row.

/// A rectangle of whole pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IRect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width (≥ 0).
    pub w: i32,
    /// Height (≥ 0).
    pub h: i32,
}

impl IRect {
    /// A rectangle.
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> IRect {
        IRect { x, y, w, h }
    }

    /// The right edge (exclusive).
    pub fn right(&self) -> i32 {
        self.x.saturating_add(self.w)
    }

    /// The bottom edge (exclusive).
    pub fn bottom(&self) -> i32 {
        self.y.saturating_add(self.h)
    }

    /// Whether it has no pixels.
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// The overlap of two rectangles (empty when they do not meet).
    pub fn intersect(&self, o: &IRect) -> IRect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        IRect::new(x, y, (r - x).max(0), (b - y).max(0))
    }

    /// The smallest rectangle of whole pixels around a box of floats
    /// (saturated to i32).
    pub fn around(x0: f32, y0: f32, x1: f32, y1: f32) -> IRect {
        let c = |v: f32| v.clamp(-1e9, 1e9) as i32;
        let (l, t) = (c(x0.floor()), c(y0.floor()));
        let (r, b) = (c(x1.ceil()), c(y1.ceil()));
        IRect::new(l, t, (r - l).max(0), (b - t).max(0))
    }
}

/// Coverage of a rectangle of pixels, 0.0..=1.0 each.
pub(crate) struct Mask {
    pub rect: IRect,
    pub cov: Vec<f32>,
}

/// Accumulates lines over `rect` (canvas pixels).
pub(crate) struct Raster {
    rect: IRect,
    /// Two spare cells per row: lines touching the right edge write there.
    stride: usize,
    acc: Vec<f32>,
}

impl Raster {
    pub fn new(rect: IRect) -> Raster {
        let w = rect.w.max(0) as usize;
        let h = rect.h.max(0) as usize;
        Raster {
            rect,
            stride: w + 2,
            acc: vec![0.0; (w + 2) * h],
        }
    }

    /// Adds a line (canvas coordinates; anything outside the rectangle
    /// is clipped: left of it counts as covering, right of it as not).
    pub fn line(&mut self, a: (f32, f32), b: (f32, f32)) {
        if !(a.0.is_finite() && a.1.is_finite() && b.0.is_finite() && b.1.is_finite()) {
            return;
        }
        let ox = self.rect.x as f32;
        let oy = self.rect.y as f32;
        let a = (a.0 - ox, a.1 - oy);
        let b = (b.0 - ox, b.1 - oy);
        let w = self.rect.w as f32;
        // Split where the line crosses x = 0 and x = w, then clamp x: a
        // piece left of the rectangle covers everything right of it, a
        // piece right of it nothing (it lands in the spare column).
        let mut cuts = [0.0f32, 1.0, 1.0, 1.0];
        let mut n = 1;
        for edge in [0.0, w] {
            if (a.0 < edge) != (b.0 < edge) && a.0 != b.0 {
                let t = (edge - a.0) / (b.0 - a.0);
                if t > 0.0 && t < 1.0 {
                    cuts[n] = t;
                    n += 1;
                }
            }
        }
        cuts[n] = 1.0;
        cuts[1..n].sort_by(f32::total_cmp);
        let at = |t: f32| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        for i in 0..n {
            let (p, q) = (at(cuts[i]), at(cuts[i + 1]));
            let p = (p.0.clamp(0.0, w), p.1);
            let q = (q.0.clamp(0.0, w), q.1);
            self.clipped_line(p, q);
        }
    }

    /// A line with 0 ≤ x ≤ w (rectangle coordinates).
    fn clipped_line(&mut self, p0: (f32, f32), p1: (f32, f32)) {
        if p0.1 == p1.1 {
            return;
        }
        let (dir, p0, p1) = if p0.1 < p1.1 {
            (1.0, p0, p1)
        } else {
            (-1.0, p1, p0)
        };
        let h = self.rect.h as f32;
        if p1.1 <= 0.0 || p0.1 >= h {
            return;
        }
        let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
        let max_x = self.rect.w as f32;
        let y_start = p0.1.max(0.0);
        let mut x = p0.0 + (y_start - p0.1) * dxdy;
        let y_end = p1.1.min(h);
        let first = y_start.floor() as usize;
        let last = y_end.ceil() as usize;
        for y in first..last {
            let row = y * self.stride;
            let top = (y as f32).max(y_start);
            let bottom = ((y + 1) as f32).min(y_end);
            let dy = bottom - top;
            if dy <= 0.0 {
                continue;
            }
            let xnext = (x + dxdy * dy).clamp(0.0, max_x);
            let d = dy * dir;
            let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let x0floor = x0.floor();
            let x0i = x0floor as usize;
            let x1ceil = x1.ceil();
            let x1i = x1ceil as usize;
            if x1i <= x0i + 1 {
                let xmf = 0.5 * (x + xnext) - x0floor;
                self.acc[row + x0i] += d - d * xmf;
                self.acc[row + x0i + 1] += d * xmf;
            } else {
                let s = (x1 - x0).recip();
                let x0f = x0 - x0floor;
                let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
                let x1f = x1 - x1ceil + 1.0;
                let am = 0.5 * s * x1f * x1f;
                self.acc[row + x0i] += d * a0;
                if x1i == x0i + 2 {
                    self.acc[row + x0i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - x0f);
                    self.acc[row + x0i + 1] += d * (a1 - a0);
                    for xi in x0i + 2..x1i - 1 {
                        self.acc[row + xi] += d * s;
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    self.acc[row + x1i - 1] += d * (1.0 - a2 - am);
                }
                self.acc[row + x1i] += d * am;
            }
            x = xnext;
        }
    }

    /// The coverage (non-zero winding).
    pub fn finish(self) -> Mask {
        let w = self.rect.w.max(0) as usize;
        let h = self.rect.h.max(0) as usize;
        let mut cov = Vec::with_capacity(w * h);
        for y in 0..h {
            let mut sum = 0.0f32;
            let row = &self.acc[y * self.stride..y * self.stride + w];
            for &a in row {
                sum += a;
                cov.push(sum.abs().min(1.0));
            }
        }
        Mask {
            rect: self.rect,
            cov,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poly(r: &mut Raster, pts: &[(f32, f32)]) {
        for i in 0..pts.len() {
            r.line(pts[i], pts[(i + 1) % pts.len()]);
        }
    }

    fn mask(rect: IRect, pts: &[(f32, f32)]) -> Mask {
        let mut r = Raster::new(rect);
        poly(&mut r, pts);
        r.finish()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn a_whole_pixel_square() {
        let m = mask(
            IRect::new(0, 0, 4, 4),
            &[(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0)],
        );
        let want = [
            0., 0., 0., 0., //
            0., 1., 1., 0., //
            0., 1., 1., 0., //
            0., 0., 0., 0.,
        ];
        for (i, (&a, &b)) in m.cov.iter().zip(&want).enumerate() {
            assert!(close(a, b), "pixel {i}: {a} != {b}");
        }
    }

    #[test]
    fn half_pixels() {
        // x from 0.5 to 1.5: two half-covered pixels.
        let m = mask(
            IRect::new(0, 0, 3, 1),
            &[(0.5, 0.0), (1.5, 0.0), (1.5, 1.0), (0.5, 1.0)],
        );
        assert!(close(m.cov[0], 0.5) && close(m.cov[1], 0.5) && close(m.cov[2], 0.0));
        // A diagonal through one pixel: half of it.
        let m = mask(
            IRect::new(0, 0, 1, 1),
            &[(0.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
        );
        assert!(close(m.cov[0], 0.5), "{}", m.cov[0]);
    }

    #[test]
    fn winding_either_way_and_overlaps_saturate() {
        let cw = [(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)];
        let ccw: Vec<_> = cw.iter().rev().copied().collect();
        let mut r = Raster::new(IRect::new(0, 0, 2, 2));
        poly(&mut r, &cw);
        poly(&mut r, &cw);
        let m = r.finish();
        assert!(m.cov.iter().all(|&c| close(c, 1.0)));
        let m = mask(IRect::new(0, 0, 2, 2), &ccw);
        assert!(m.cov.iter().all(|&c| close(c, 1.0)));
    }

    #[test]
    fn clipped_to_the_rectangle() {
        // A square much larger than the raster, offset: all covered.
        let m = mask(
            IRect::new(10, 10, 3, 3),
            &[(-50.0, -50.0), (50.0, -50.0), (50.0, 50.0), (-50.0, 50.0)],
        );
        assert!(m.cov.iter().all(|&c| close(c, 1.0)), "{:?}", m.cov);
        // A triangle crossing the left edge mid-row: the clipped area.
        let m = mask(
            IRect::new(0, 0, 2, 1),
            &[(-1.0, 0.0), (1.0, 1.0), (-1.0, 1.0)],
        );
        // Inside x ∈ [0,1]: the part of the triangle right of 0 is a
        // triangle of area 0.25 (from (0,0.5) to (1,1) to (0,1)).
        assert!(close(m.cov[0], 0.25), "{}", m.cov[0]);
        assert!(close(m.cov[1], 0.0));
        // Entirely right of or below the raster: nothing.
        let m = mask(
            IRect::new(0, 0, 2, 2),
            &[(5.0, 0.0), (6.0, 0.0), (6.0, 2.0)],
        );
        assert!(m.cov.iter().all(|&c| c == 0.0));
    }

    #[test]
    fn non_finite_points_are_ignored() {
        let m = mask(
            IRect::new(0, 0, 2, 2),
            &[(f32::NAN, 0.0), (2.0, f32::INFINITY), (0.0, 2.0)],
        );
        assert!(m.cov.iter().all(|&c| c.is_finite()));
    }
}
