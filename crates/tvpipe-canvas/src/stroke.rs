//! Strokes as round-capped, round-joined polylines: a pixel's coverage
//! is how far its centre lies inside the nearest segment's capsule (one
//! pixel of ramp), max-combined over the segments so joints do not blend
//! twice.

use crate::raster::{IRect, Mask};

/// The coverage of a polyline of `width` over `clip`.
pub(crate) fn polyline(
    points: &[(f32, f32)],
    closed: bool,
    width: f32,
    clip: IRect,
) -> Option<Mask> {
    let r = width / 2.0;
    if !(r.is_finite() && r > 0.0) || points.is_empty() {
        return None;
    }
    let pts: Vec<(f32, f32)> = points
        .iter()
        .copied()
        .filter(|p| p.0.is_finite() && p.1.is_finite())
        .collect();
    let first = *pts.first()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
    for &(x, y) in &pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let pad = r + 1.0;
    let rect = IRect::around(x0 - pad, y0 - pad, x1 + pad, y1 + pad).intersect(&clip);
    if rect.is_empty() {
        return None;
    }
    let w = rect.w as usize;
    let mut cov = vec![0.0f32; w * rect.h as usize];
    let mut segment = |a: (f32, f32), b: (f32, f32)| {
        let s = IRect::around(
            a.0.min(b.0) - pad,
            a.1.min(b.1) - pad,
            a.0.max(b.0) + pad,
            a.1.max(b.1) + pad,
        )
        .intersect(&rect);
        if s.is_empty() {
            return;
        }
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let ll = dx * dx + dy * dy;
        for py in s.y..s.bottom() {
            let cy = py as f32 + 0.5;
            let row = (py - rect.y) as usize * w;
            for px in s.x..s.right() {
                let cx = px as f32 + 0.5;
                let t = if ll > 0.0 {
                    (((cx - a.0) * dx + (cy - a.1) * dy) / ll).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (ex, ey) = (cx - (a.0 + t * dx), cy - (a.1 + t * dy));
                let c = (r + 0.5 - (ex * ex + ey * ey).sqrt()).clamp(0.0, 1.0);
                let cell = &mut cov[row + (px - rect.x) as usize];
                if c > *cell {
                    *cell = c;
                }
            }
        }
    };
    if pts.len() == 1 {
        segment(first, first);
    }
    for pair in pts.windows(2) {
        segment(pair[0], pair[1]);
    }
    if closed && pts.len() > 2 {
        segment(pts[pts.len() - 1], first);
    }
    Some(Mask { rect, cov })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dot_and_a_line() {
        let clip = IRect::new(0, 0, 20, 20);
        // A dot of radius 3 at a pixel centre: the centre is covered, a
        // pixel 5 away is not.
        let m = polyline(&[(10.5, 10.5)], false, 6.0, clip).unwrap();
        let at =
            |m: &Mask, x: i32, y: i32| m.cov[((y - m.rect.y) * m.rect.w + x - m.rect.x) as usize];
        assert_eq!(at(&m, 10, 10), 1.0);
        assert_eq!(at(&m, 15, 10), 0.0);
        // Edge pixel centre exactly at distance r: half covered.
        assert_eq!(at(&m, 13, 10), 0.5);
        // A horizontal line of width 2 through pixel centres' row.
        let m = polyline(&[(2.0, 10.5), (18.0, 10.5)], false, 2.0, clip).unwrap();
        assert_eq!(at(&m, 10, 10), 1.0);
        assert_eq!(at(&m, 10, 9), 0.5);
        assert_eq!(at(&m, 10, 8), 0.0);
    }

    #[test]
    fn joints_do_not_add_up() {
        let clip = IRect::new(0, 0, 20, 20);
        let m = polyline(&[(2.0, 2.0), (10.0, 10.0), (2.0, 18.0)], false, 4.0, clip).unwrap();
        assert!(m.cov.iter().all(|&c| (0.0..=1.0).contains(&c)));
    }

    #[test]
    fn nothing_for_bad_input() {
        let clip = IRect::new(0, 0, 20, 20);
        assert!(polyline(&[], false, 2.0, clip).is_none());
        assert!(polyline(&[(1.0, 1.0)], false, f32::NAN, clip).is_none());
        assert!(polyline(&[(f32::NAN, 1.0)], false, 2.0, clip).is_none());
        assert!(polyline(&[(100.0, 100.0)], false, 2.0, clip).is_none());
    }
}
