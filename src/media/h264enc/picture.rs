//! Sample planes with optional edge padding.

/// An 8-bit plane. Sample (x, y) for x in -pad..w+pad, y in -pad..h+pad
/// lives at `(y + pad) * stride + x + pad`.
#[derive(Debug, Clone, Default)]
pub struct Plane {
    pub data: Vec<u8>,
    pub stride: usize,
    pub w: usize,
    pub h: usize,
    pub pad: usize,
}

impl Plane {
    pub fn new(w: usize, h: usize, pad: usize) -> Plane {
        let stride = w + 2 * pad;
        Plane {
            data: vec![0; stride * (h + 2 * pad)],
            stride,
            w,
            h,
            pad,
        }
    }

    /// Index of sample (x, y); x, y may be negative down to -pad.
    #[inline]
    pub fn idx(&self, x: isize, y: isize) -> usize {
        ((y + self.pad as isize) as usize) * self.stride + (x + self.pad as isize) as usize
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> u8 {
        self.data[(y + self.pad) * self.stride + x + self.pad]
    }

    /// Row `y` (0..h), the `w` visible samples.
    #[inline]
    pub fn row(&self, y: usize) -> &[u8] {
        let i = (y + self.pad) * self.stride + self.pad;
        &self.data[i..i + self.w]
    }

    #[inline]
    pub fn row_mut(&mut self, y: usize) -> &mut [u8] {
        let i = (y + self.pad) * self.stride + self.pad;
        &mut self.data[i..i + self.w]
    }

    /// Clamped sample access (the reference sample clamp of §8.4.2.2).
    #[inline]
    pub fn clamped(&self, x: isize, y: isize) -> u8 {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.at(x, y)
    }

    /// Replicates the border samples into the padding.
    pub fn extend_edges(&mut self) {
        let (p, w, h, s) = (self.pad, self.w, self.h, self.stride);
        if p == 0 {
            return;
        }
        for y in 0..h {
            let r = (y + p) * s;
            let left = self.data[r + p];
            let right = self.data[r + p + w - 1];
            self.data[r..r + p].fill(left);
            self.data[r + p + w..r + s].fill(right);
        }
        let top = p * s;
        for y in 0..p {
            self.data.copy_within(top..top + s, y * s);
        }
        let bottom = (p + h - 1) * s;
        for y in p + h..h + 2 * p {
            self.data.copy_within(bottom..bottom + s, y * s);
        }
    }

    /// Copies `src`'s visible area into rows `y0..` of this plane.
    pub fn copy_rows_from(&mut self, src: &Plane, y0: usize) {
        let w = self.w.min(src.w);
        for y in 0..src.h {
            if y0 + y >= self.h {
                break;
            }
            self.row_mut(y0 + y)[..w].copy_from_slice(&src.row(y)[..w]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges() {
        let mut p = Plane::new(4, 2, 2);
        p.row_mut(0).copy_from_slice(&[1, 2, 3, 4]);
        p.row_mut(1).copy_from_slice(&[5, 6, 7, 8]);
        p.extend_edges();
        assert_eq!(p.data[p.idx(-2, -2)], 1);
        assert_eq!(p.data[p.idx(5, 3)], 8);
        assert_eq!(p.data[p.idx(-1, 1)], 5);
        assert_eq!(p.clamped(-7, 9), 5);
    }
}
