//! Where a capture goes in the stream's frames (`share`).
//!
//! Receivers stretch whatever they get to fill their screen (a Cast
//! receiver with `scaling: sender`, an Apple TV to its canvas), so the
//! stream has the shape of the receivers' screen and the capture sits
//! centered inside it at its own aspect ratio, with black bars
//! (letterbox or pillarbox). Pixels are square: the browser's SPS carries
//! no other sample aspect ratio (checked by the input).
//!
//! The page does the drawing (`capture.js`, the same arithmetic in `place`
//! there): it draws each frame into a canvas of the output size at the
//! plan's content rectangle, or recomputes the rectangle when the capture
//! changes size mid-stream.

/// A rectangle in the output frame (all values even, for 4:2:0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The shape assumed for a receiver that does not say (TVs and
/// projectors are 16:9).
pub const DEFAULT_ASPECT: (u32, u32) = (16, 9);

fn even(v: u64) -> u32 {
    (v.max(2) & !1).min(u32::MAX as u64 - 1) as u32
}

/// The output's shape for several receivers: the most common display
/// aspect (receivers that do not say count as 16:9); on a tie 16:9 if it
/// is among the most common, else the first receiver's. The others
/// stretch the picture.
pub fn output_aspect(aspects: &[Option<(u32, u32)>]) -> (u32, u32) {
    let all: Vec<(u32, u32)> = aspects
        .iter()
        .map(|a| a.filter(|&(w, h)| w > 0 && h > 0).unwrap_or(DEFAULT_ASPECT))
        .collect();
    let count = |a: (u32, u32)| all.iter().filter(|&&b| b == a).count();
    let best = all.iter().map(|&a| count(a)).max().unwrap_or(0);
    if best == 0 || count(DEFAULT_ASPECT) == best {
        return DEFAULT_ASPECT;
    }
    all.iter()
        .copied()
        .find(|&a| count(a) == best)
        .unwrap_or(DEFAULT_ASPECT)
}

/// The output frame for a `capture` (width, height): the smallest frame
/// of `aspect` that holds the capture at 1:1, shrunk to fit `max` (the
/// receivers' limits). Even sides.
pub fn output_size(capture: (u32, u32), aspect: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    let (a, b) = (aspect.0.max(1) as u64, aspect.1.max(1) as u64);
    let (cw, ch) = (capture.0.max(2) as u64, capture.1.max(2) as u64);
    let (mw, mh) = (max.0.max(2) as u64, max.1.max(2) as u64);
    // Height of the smallest a:b frame around the capture, and of the
    // largest one inside the limits.
    let around = ch.max((cw * b).div_ceil(a));
    let inside = mh.min(mw * b / a);
    let h = around.min(inside);
    let w = (h * a + b / 2) / b;
    (even(w.min(mw)), even(h))
}

/// Where a `src`-sized picture goes in a `dst` frame: as large as fits
/// at its own aspect ratio, centered.
pub fn place(src: (u32, u32), dst: (u32, u32)) -> Rect {
    let (sw, sh) = (src.0.max(1) as u64, src.1.max(1) as u64);
    let (dw, dh) = (dst.0.max(2) as u64, dst.1.max(2) as u64);
    let (w, h) = if sw * dh >= sh * dw {
        // Wider than the frame: full width, bars above and below.
        (dw, (sh * dw + sw / 2) / sw)
    } else {
        ((sw * dh + sh / 2) / sh, dh)
    };
    let (w, h) = (even(w.min(dw)), even(h.min(dh)));
    Rect {
        x: ((dw as u32 - w) / 2) & !1,
        y: ((dh as u32 - h) / 2) & !1,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::aspect_of;

    #[test]
    fn aspects() {
        assert_eq!(aspect_of(3840, 2160), Some((16, 9)));
        assert_eq!(aspect_of(1920, 1200), Some((8, 5)));
        assert_eq!(aspect_of(1280, 0), None);
        let (wide, old) = (Some((16, 9)), Some((4, 3)));
        assert_eq!(output_aspect(&[]), (16, 9));
        assert_eq!(output_aspect(&[None]), (16, 9));
        assert_eq!(output_aspect(&[old]), (4, 3));
        assert_eq!(output_aspect(&[old, old, wide]), (4, 3));
        // A tie: 16:9 (a receiver that does not say counts as 16:9).
        assert_eq!(output_aspect(&[old, None]), (16, 9));
        assert_eq!(output_aspect(&[Some((8, 5)), old]), (8, 5));
    }

    #[test]
    fn output_sizes() {
        let hd = (1920, 1080);
        // The projector case: a 16:10 capture on a 16:9 screen.
        assert_eq!(output_size((1920, 1200), (16, 9), hd), (1920, 1080));
        assert_eq!(
            place((1920, 1200), (1920, 1080)),
            Rect {
                x: 96,
                y: 0,
                width: 1728,
                height: 1080
            }
        );
        // Already the screen's shape and size, or larger: the limit.
        assert_eq!(output_size((1920, 1080), (16, 9), hd), hd);
        assert_eq!(output_size((3840, 2160), (16, 9), hd), hd);
        assert_eq!(output_size((3024, 1964), (16, 9), hd), hd);
        // Smaller than the limits: no upscaling, only the bars.
        assert_eq!(output_size((800, 600), (16, 9), hd), (1066, 600));
        assert_eq!(
            place((800, 600), (1066, 600)),
            Rect {
                x: 132,
                y: 0,
                width: 800,
                height: 600
            }
        );
        // A tall window: pillarbox.
        assert_eq!(output_size((600, 1000), (16, 9), hd), (1778, 1000));
        // A wide one on a 4:3 screen: letterbox.
        assert_eq!(output_size((1920, 800), (4, 3), (1440, 1080)), (1440, 1080));
        assert_eq!(
            place((1920, 800), (1440, 1080)),
            Rect {
                x: 0,
                y: 240,
                width: 1440,
                height: 600
            }
        );
        // Limits of another shape than the screen (a Cast ANSWER capping
        // at 1280x720 while the screen is 4:3): still 4:3, inside.
        assert_eq!(output_size((1920, 1080), (4, 3), (1280, 720)), (960, 720));
        // Degenerate input never panics and stays even.
        assert_eq!(output_size((0, 0), (0, 0), (0, 0)), (2, 2));
        let r = place((1, 99999), (2, 2));
        assert!(r.width >= 2 && r.height >= 2 && r.x % 2 == 0 && r.y % 2 == 0);
        // Same shape: full frame.
        assert_eq!(
            place((1280, 720), (1920, 1080)),
            Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
        // Every result is even and inside the frame.
        for (sw, sh) in [(1, 1), (333, 777), (1919, 1201), (5120, 2880), (17, 3)] {
            for (dw, dh) in [(1920, 1080), (1066, 600), (1440, 1080), (2, 2)] {
                let r = place((sw, sh), (dw, dh));
                assert!(
                    r.x + r.width <= dw && r.y + r.height <= dh,
                    "{sw}x{sh} in {dw}x{dh}"
                );
                assert!([r.x, r.y, r.width, r.height].iter().all(|v| v % 2 == 0));
            }
        }
    }
}
