//! Paths: lines and Bézier curves, flattened to polygons for the
//! accumulator (fills) or the capsule stroker (strokes).

use std::f32::consts::PI;

/// How far a flattened curve may stray from the real one, in pixels.
const TOLERANCE: f32 = 0.02;
/// Most line pieces per curve.
const MAX_PIECES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Cmd {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

/// A path of contours (canvas pixels, y down).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    cmds: Vec<Cmd>,
}

/// A flattened contour.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Polyline {
    pub points: Vec<(f32, f32)>,
    pub closed: bool,
}

impl Path {
    /// An empty path.
    pub fn new() -> Path {
        Path::default()
    }

    /// Whether nothing was added.
    pub fn is_empty(&self) -> bool {
        self.cmds.is_empty()
    }

    /// Starts a contour.
    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Path {
        self.cmds.push(Cmd::Move(x, y));
        self
    }

    /// A straight line.
    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Path {
        self.cmds.push(Cmd::Line(x, y));
        self
    }

    /// A quadratic Bézier curve.
    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) -> &mut Path {
        self.cmds.push(Cmd::Quad(cx, cy, x, y));
        self
    }

    /// A cubic Bézier curve.
    pub fn cubic_to(
        &mut self,
        c1x: f32,
        c1y: f32,
        c2x: f32,
        c2y: f32,
        x: f32,
        y: f32,
    ) -> &mut Path {
        self.cmds.push(Cmd::Cubic(c1x, c1y, c2x, c2y, x, y));
        self
    }

    /// Closes the contour (a line back to its start).
    pub fn close(&mut self) -> &mut Path {
        self.cmds.push(Cmd::Close);
        self
    }

    /// A rectangle.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) -> &mut Path {
        self.move_to(x, y)
            .line_to(x + w, y)
            .line_to(x + w, y + h)
            .line_to(x, y + h)
            .close()
    }

    /// A rectangle with circular corners of radius `r` (at most half the
    /// shorter side).
    pub fn rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) -> &mut Path {
        let r = r.min(w.abs() / 2.0).min(h.abs() / 2.0).max(0.0);
        if r == 0.0 {
            return self.rect(x, y, w, h);
        }
        // The cubic approximation of a quarter circle.
        let k = r * 0.552_284_8;
        let (r0, b0) = (x + w, y + h);
        self.move_to(x + r, y)
            .line_to(r0 - r, y)
            .cubic_to(r0 - r + k, y, r0, y + r - k, r0, y + r)
            .line_to(r0, b0 - r)
            .cubic_to(r0, b0 - r + k, r0 - r + k, b0, r0 - r, b0)
            .line_to(x + r, b0)
            .cubic_to(x + r - k, b0, x, b0 - r + k, x, b0 - r)
            .line_to(x, y + r)
            .cubic_to(x, y + r - k, x + r - k, y, x + r, y)
            .close()
    }

    /// An ellipse.
    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32) -> &mut Path {
        let (kx, ky) = (rx * 0.552_284_8, ry * 0.552_284_8);
        self.move_to(cx + rx, cy)
            .cubic_to(cx + rx, cy + ky, cx + kx, cy + ry, cx, cy + ry)
            .cubic_to(cx - kx, cy + ry, cx - rx, cy + ky, cx - rx, cy)
            .cubic_to(cx - rx, cy - ky, cx - kx, cy - ry, cx, cy - ry)
            .cubic_to(cx + kx, cy - ry, cx + rx, cy - ky, cx + rx, cy)
            .close()
    }

    /// A circle.
    pub fn circle(&mut self, cx: f32, cy: f32, r: f32) -> &mut Path {
        self.ellipse(cx, cy, r, r)
    }

    /// An arc of a circle from angle `a0` to `a1` (radians, clockwise on
    /// screen as y points down), continuing the contour with a line to
    /// its start (or starting one).
    pub fn arc(&mut self, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> &mut Path {
        let sweep = (a1 - a0).clamp(-2.0 * PI, 2.0 * PI);
        let n = ((sweep.abs() / (PI / 2.0)).ceil() as usize).max(1);
        let step = sweep / n as f32;
        let (sx, sy) = (cx + r * a0.cos(), cy + r * a0.sin());
        if matches!(self.cmds.last(), None | Some(Cmd::Close)) {
            self.move_to(sx, sy);
        } else {
            self.line_to(sx, sy);
        }
        // Each piece (≤ 90°) as a cubic.
        let k = 4.0 / 3.0 * (step / 4.0).tan();
        let mut a = a0;
        for _ in 0..n {
            let b = a + step;
            let (ca, sa, cb, sb) = (a.cos(), a.sin(), b.cos(), b.sin());
            self.cubic_to(
                cx + r * (ca - k * sa),
                cy + r * (sa + k * ca),
                cx + r * (cb + k * sb),
                cy + r * (sb - k * cb),
                cx + r * cb,
                cy + r * sb,
            );
            a = b;
        }
        self
    }

    /// The path moved and scaled: each point becomes `(x·sx + dx, y·sy + dy)`.
    pub fn transformed(&self, sx: f32, sy: f32, dx: f32, dy: f32) -> Path {
        let t = |x: f32, y: f32| (x * sx + dx, y * sy + dy);
        let cmds = self
            .cmds
            .iter()
            .map(|c| match *c {
                Cmd::Move(x, y) => {
                    let (x, y) = t(x, y);
                    Cmd::Move(x, y)
                }
                Cmd::Line(x, y) => {
                    let (x, y) = t(x, y);
                    Cmd::Line(x, y)
                }
                Cmd::Quad(a, b, x, y) => {
                    let ((a, b), (x, y)) = (t(a, b), t(x, y));
                    Cmd::Quad(a, b, x, y)
                }
                Cmd::Cubic(a, b, c, d, x, y) => {
                    let ((a, b), (c, d), (x, y)) = (t(a, b), t(c, d), t(x, y));
                    Cmd::Cubic(a, b, c, d, x, y)
                }
                Cmd::Close => Cmd::Close,
            })
            .collect();
        Path { cmds }
    }

    /// The bounding box of the points and control points
    /// (`(x0, y0, x1, y1)`; `None` when empty).
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut b: Option<(f32, f32, f32, f32)> = None;
        let mut add = |x: f32, y: f32| {
            if !(x.is_finite() && y.is_finite()) {
                return;
            }
            b = Some(match b {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        };
        for c in &self.cmds {
            match *c {
                Cmd::Move(x, y) | Cmd::Line(x, y) => add(x, y),
                Cmd::Quad(a, b, x, y) => {
                    add(a, b);
                    add(x, y);
                }
                Cmd::Cubic(a, b, c, d, x, y) => {
                    add(a, b);
                    add(c, d);
                    add(x, y);
                }
                Cmd::Close => {}
            }
        }
        b
    }

    /// The contours as polylines.
    pub(crate) fn flatten(&self) -> Vec<Polyline> {
        let mut out: Vec<Polyline> = Vec::new();
        let mut cur = Polyline::default();
        let mut start = (0.0, 0.0);
        let mut at = (0.0, 0.0);
        let finish = |cur: &mut Polyline, out: &mut Vec<Polyline>| {
            if cur.points.len() > 1 {
                out.push(std::mem::take(cur));
            } else {
                cur.points.clear();
                cur.closed = false;
            }
        };
        for c in &self.cmds {
            match *c {
                Cmd::Move(x, y) => {
                    finish(&mut cur, &mut out);
                    start = (x, y);
                    at = start;
                    cur.points.push(at);
                }
                Cmd::Line(x, y) => {
                    if cur.points.is_empty() {
                        cur.points.push(at);
                    }
                    at = (x, y);
                    cur.points.push(at);
                }
                Cmd::Quad(cx, cy, x, y) => {
                    if cur.points.is_empty() {
                        cur.points.push(at);
                    }
                    let (p0, p1, p2) = (at, (cx, cy), (x, y));
                    let dd = len(p0.0 - 2.0 * p1.0 + p2.0, p0.1 - 2.0 * p1.1 + p2.1);
                    let n = pieces((dd / (8.0 * TOLERANCE)).sqrt());
                    for i in 1..=n {
                        let t = i as f32 / n as f32;
                        let u = 1.0 - t;
                        cur.points.push((
                            u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0,
                            u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1,
                        ));
                    }
                    at = p2;
                }
                Cmd::Cubic(ax, ay, bx, by, x, y) => {
                    if cur.points.is_empty() {
                        cur.points.push(at);
                    }
                    let (p0, p1, p2, p3) = (at, (ax, ay), (bx, by), (x, y));
                    let dd = len(p0.0 - 2.0 * p1.0 + p2.0, p0.1 - 2.0 * p1.1 + p2.1)
                        .max(len(p1.0 - 2.0 * p2.0 + p3.0, p1.1 - 2.0 * p2.1 + p3.1));
                    let n = pieces((0.75 * dd / TOLERANCE).sqrt());
                    for i in 1..=n {
                        let t = i as f32 / n as f32;
                        let u = 1.0 - t;
                        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                        cur.points.push((
                            a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
                            a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
                        ));
                    }
                    at = p3;
                }
                Cmd::Close => {
                    cur.closed = true;
                    finish(&mut cur, &mut out);
                    at = start;
                }
            }
        }
        finish(&mut cur, &mut out);
        out
    }
}

fn len(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

fn pieces(n: f32) -> usize {
    if n.is_finite() {
        (n.ceil() as usize).clamp(1, MAX_PIECES)
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattening() {
        let mut p = Path::new();
        p.move_to(0.0, 0.0)
            .line_to(10.0, 0.0)
            .line_to(10.0, 10.0)
            .close();
        p.move_to(20.0, 0.0).quad_to(30.0, 20.0, 40.0, 0.0);
        let f = p.flatten();
        assert_eq!(f.len(), 2);
        assert!(f[0].closed && !f[1].closed);
        assert_eq!(f[0].points, vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
        // The quadratic's apex (t = 0.5) is at y = 10.
        let apex = f[1].points.iter().map(|p| p.1).fold(0.0f32, f32::max);
        assert!((apex - 10.0).abs() < TOLERANCE, "{apex}");
        assert_eq!(f[1].points.last(), Some(&(40.0, 0.0)));
    }

    #[test]
    fn circle_points_lie_on_the_circle() {
        let mut p = Path::new();
        p.circle(50.0, 50.0, 40.0);
        let f = p.flatten();
        for &(x, y) in &f[0].points {
            let r = len(x - 50.0, y - 50.0);
            assert!((r - 40.0).abs() < 0.05, "{r}");
        }
    }

    #[test]
    fn arcs() {
        let mut p = Path::new();
        p.arc(0.0, 0.0, 10.0, 0.0, PI);
        let f = p.flatten();
        let first = f[0].points[0];
        let last = *f[0].points.last().unwrap();
        assert!((first.0 - 10.0).abs() < 1e-4 && first.1.abs() < 1e-4);
        assert!((last.0 + 10.0).abs() < 1e-3 && last.1.abs() < 1e-3);
        // y down: from angle 0 to π passes through +y.
        assert!(f[0].points.iter().any(|p| p.1 > 9.9));
    }

    #[test]
    fn degenerate_input() {
        let mut p = Path::new();
        p.line_to(1.0, 1.0)
            .quad_to(f32::NAN, 0.0, 1.0, f32::INFINITY)
            .close()
            .close()
            .move_to(0.0, 0.0);
        let _ = p.flatten();
        assert_eq!(p.bounds(), Some((0.0, 0.0, 1.0, 1.0)));
    }
}
