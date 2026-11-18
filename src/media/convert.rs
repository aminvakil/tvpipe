//! Raw picture formats for the built-in encoder: the planar 4:2:0 frame it
//! takes, conversions from packed RGB/BGR and NV12, and a bilinear scaler.
//!
//! RGB → Y'CbCr uses the matrix of ITU-R BT.709 (default) or BT.601 in
//! 14-bit fixed point, limited ("studio", 16-235/240) or full range.
//! Chroma is the average of each 2x2 block of RGB samples.
//!
//! Sources: ITU-R BT.709-6 Table 3 items 3.2-3.5, ITU-R BT.601-7 §2.5
//! (Kr/Kb and quantisation), ITU-T H.273 §8.3 (equations 26-31, range).

use crate::error::{Error, Result};

/// A planar 8-bit 4:2:0 picture (I420). Planes are tightly packed: luma
/// stride = width, chroma stride = ceil(width / 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Yuv420Frame {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl Yuv420Frame {
    /// A black (limited range) frame.
    pub fn new(width: u32, height: u32) -> Yuv420Frame {
        let (cw, ch) = chroma_size(width, height);
        Yuv420Frame {
            width,
            height,
            y: vec![16; width as usize * height as usize],
            u: vec![128; cw * ch],
            v: vec![128; cw * ch],
        }
    }

    /// Wraps planes after checking their sizes.
    pub fn from_planes(
        width: u32,
        height: u32,
        y: Vec<u8>,
        u: Vec<u8>,
        v: Vec<u8>,
    ) -> Result<Yuv420Frame> {
        let (cw, ch) = chroma_size(width, height);
        if y.len() != width as usize * height as usize || u.len() != cw * ch || v.len() != cw * ch {
            return Err(Error::malformed("yuv420: plane sizes do not match"));
        }
        Ok(Yuv420Frame {
            width,
            height,
            y,
            u,
            v,
        })
    }

    /// Parses a packed I420 buffer (Y, then U, then V).
    pub fn from_i420(width: u32, height: u32, data: &[u8]) -> Result<Yuv420Frame> {
        let ys = width as usize * height as usize;
        let (cw, ch) = chroma_size(width, height);
        let cs = cw * ch;
        if data.len() != ys + 2 * cs {
            return Err(Error::malformed("yuv420: buffer size does not match"));
        }
        Ok(Yuv420Frame {
            width,
            height,
            y: data[..ys].to_vec(),
            u: data[ys..ys + cs].to_vec(),
            v: data[ys + cs..].to_vec(),
        })
    }

    pub fn chroma_width(&self) -> usize {
        chroma_size(self.width, self.height).0
    }

    pub fn chroma_height(&self) -> usize {
        chroma_size(self.width, self.height).1
    }

    /// Packed I420 bytes.
    pub fn to_i420(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.y.len() + 2 * self.u.len());
        out.extend_from_slice(&self.y);
        out.extend_from_slice(&self.u);
        out.extend_from_slice(&self.v);
        out
    }
}

fn chroma_size(width: u32, height: u32) -> (usize, usize) {
    (width.div_ceil(2) as usize, height.div_ceil(2) as usize)
}

/// Input pixel layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// R, G, B, A bytes (alpha ignored).
    Rgba,
    /// B, G, R, A bytes (alpha ignored).
    Bgra,
    Rgb24,
    Bgr24,
    /// Luma plane, then interleaved Cb/Cr at half resolution.
    Nv12,
    /// Planar Y, U, V (4:2:0).
    I420,
}

impl PixelFormat {
    /// ffmpeg-style names (`rgba`, `bgra`, `rgb0`, `bgr0`, `rgb24`,
    /// `bgr24`, `nv12`, `yuv420p`).
    pub fn from_name(name: &str) -> Option<PixelFormat> {
        Some(match name {
            "rgba" | "rgb0" => PixelFormat::Rgba,
            "bgra" | "bgr0" => PixelFormat::Bgra,
            "rgb24" => PixelFormat::Rgb24,
            "bgr24" => PixelFormat::Bgr24,
            "nv12" => PixelFormat::Nv12,
            "yuv420p" | "i420" => PixelFormat::I420,
            _ => return None,
        })
    }

    /// Bytes of a tightly packed frame.
    pub fn frame_size(self, width: u32, height: u32) -> usize {
        let (w, h) = (width as usize, height as usize);
        let (cw, ch) = chroma_size(width, height);
        match self {
            PixelFormat::Rgba | PixelFormat::Bgra => w * h * 4,
            PixelFormat::Rgb24 | PixelFormat::Bgr24 => w * h * 3,
            PixelFormat::Nv12 | PixelFormat::I420 => w * h + 2 * cw * ch,
        }
    }
}

/// Y'CbCr matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Matrix {
    #[default]
    Bt709,
    Bt601,
}

/// How RGB input is mapped to Y'CbCr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ColorSpec {
    pub matrix: Matrix,
    pub full_range: bool,
}

/// Fixed-point (Q14) coefficients: Y = (yr·R + yg·G + yb·B) >> 14 + 16,
/// etc.
struct Coeffs {
    yr: i32,
    yg: i32,
    yb: i32,
    ur: i32,
    ug: i32,
    ub: i32,
    vr: i32,
    vg: i32,
    vb: i32,
    y_off: i32,
}

impl Coeffs {
    fn new(spec: ColorSpec) -> Coeffs {
        let (kr, kb) = match spec.matrix {
            Matrix::Bt709 => (0.2126, 0.0722),
            Matrix::Bt601 => (0.299, 0.114),
        };
        let kg = 1.0 - kr - kb;
        let (ys, cs, y_off) = if spec.full_range {
            (1.0, 1.0, 0)
        } else {
            (219.0 / 255.0, 224.0 / 255.0, 16)
        };
        let q = |v: f64| (v * 16384.0).round() as i32;
        let ud = 2.0 * (1.0 - kb);
        let vd = 2.0 * (1.0 - kr);
        Coeffs {
            yr: q(kr * ys),
            yg: q(kg * ys),
            yb: q(kb * ys),
            ur: q(-kr / ud * cs),
            ug: q(-kg / ud * cs),
            ub: q((1.0 - kb) / ud * cs),
            vr: q((1.0 - kr) / vd * cs),
            vg: q(-kg / vd * cs),
            vb: q(-kb / vd * cs),
            y_off,
        }
    }

    #[inline]
    fn y(&self, r: i32, g: i32, b: i32) -> u8 {
        (((self.yr * r + self.yg * g + self.yb * b + (1 << 13)) >> 14) + self.y_off).clamp(0, 255)
            as u8
    }

    /// Cb, Cr from the sum of four RGB samples.
    #[inline]
    fn uv4(&self, r: i32, g: i32, b: i32) -> (u8, u8) {
        let u = ((self.ur * r + self.ug * g + self.ub * b + (1 << 15)) >> 16) + 128;
        let v = ((self.vr * r + self.vg * g + self.vb * b + (1 << 15)) >> 16) + 128;
        (u.clamp(0, 255) as u8, v.clamp(0, 255) as u8)
    }
}

/// Converts a packed frame to I420. `stride` is the byte distance between
/// rows of packed RGB formats or of the luma plane (NV12, I420; chroma
/// rows then use `stride` for NV12 and `stride / 2` rounded up for
/// I420); `None` means tightly packed.
pub fn to_i420(
    fmt: PixelFormat,
    width: u32,
    height: u32,
    data: &[u8],
    stride: Option<usize>,
    spec: ColorSpec,
) -> Result<Yuv420Frame> {
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(Error::malformed("convert: invalid frame size"));
    }
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = chroma_size(width, height);
    let bpp = match fmt {
        PixelFormat::Rgba | PixelFormat::Bgra => 4,
        PixelFormat::Rgb24 | PixelFormat::Bgr24 => 3,
        _ => 1,
    };
    let stride = stride.unwrap_or(w * bpp);
    if stride < w * bpp {
        return Err(Error::malformed("convert: stride smaller than a row"));
    }
    let mut out = Yuv420Frame::new(width, height);
    match fmt {
        PixelFormat::Nv12 | PixelFormat::I420 => {
            let cstride = if fmt == PixelFormat::Nv12 {
                stride.max(cw * 2)
            } else {
                stride.div_ceil(2).max(cw)
            };
            let need = stride * h
                + if fmt == PixelFormat::Nv12 {
                    cstride * ch
                } else {
                    2 * cstride * ch
                };
            if data.len() < need {
                return Err(Error::malformed("convert: frame buffer too short"));
            }
            for y in 0..h {
                out.y[y * w..(y + 1) * w].copy_from_slice(&data[y * stride..y * stride + w]);
            }
            let c0 = stride * h;
            for y in 0..ch {
                if fmt == PixelFormat::Nv12 {
                    let row = &data[c0 + y * cstride..c0 + y * cstride + 2 * cw];
                    for x in 0..cw {
                        out.u[y * cw + x] = row[2 * x];
                        out.v[y * cw + x] = row[2 * x + 1];
                    }
                } else {
                    let u0 = c0 + y * cstride;
                    let v0 = c0 + cstride * ch + y * cstride;
                    out.u[y * cw..(y + 1) * cw].copy_from_slice(&data[u0..u0 + cw]);
                    out.v[y * cw..(y + 1) * cw].copy_from_slice(&data[v0..v0 + cw]);
                }
            }
        }
        _ => {
            if data.len() < stride * (h - 1) + w * bpp {
                return Err(Error::malformed("convert: frame buffer too short"));
            }
            let k = Coeffs::new(spec);
            match fmt {
                PixelFormat::Rgba => rgb_rows::<4, 0, 2>(&k, data, stride, &mut out),
                PixelFormat::Bgra => rgb_rows::<4, 2, 0>(&k, data, stride, &mut out),
                PixelFormat::Rgb24 => rgb_rows::<3, 0, 2>(&k, data, stride, &mut out),
                _ => rgb_rows::<3, 2, 0>(&k, data, stride, &mut out),
            }
        }
    }
    Ok(out)
}

/// Packed RGB → I420, two rows at a time. `BPP` bytes per pixel, red at
/// byte `R`, blue at byte `B`, green at 1. The caller checked the sizes.
fn rgb_rows<const BPP: usize, const R: usize, const B: usize>(
    k: &Coeffs,
    data: &[u8],
    stride: usize,
    out: &mut Yuv420Frame,
) {
    let (w, h) = (out.width as usize, out.height as usize);
    let cw = out.chroma_width();
    let row = |y: usize| &data[y * stride..y * stride + w * BPP];
    let px = |r: &[u8], x: usize| -> (i32, i32, i32) {
        let p = &r[x * BPP..x * BPP + 3];
        (p[R] as i32, p[1] as i32, p[B] as i32)
    };
    for (cy, (yy, (uu, vv))) in out
        .y
        .chunks_mut(2 * w)
        .zip(out.u.chunks_exact_mut(cw).zip(out.v.chunks_exact_mut(cw)))
        .enumerate()
    {
        let y0 = 2 * cy;
        let y1 = (y0 + 1).min(h - 1);
        let (r0, r1) = (row(y0), row(y1));
        let (ya, yb) = yy.split_at_mut(w);
        for cx in 0..cw {
            let x0 = 2 * cx;
            let x1 = (x0 + 1).min(w - 1);
            let a = px(r0, x0);
            let b = px(r0, x1);
            let c = px(r1, x0);
            let d = px(r1, x1);
            ya[x0] = k.y(a.0, a.1, a.2);
            if x1 != x0 {
                ya[x1] = k.y(b.0, b.1, b.2);
            }
            if y1 != y0 {
                yb[x0] = k.y(c.0, c.1, c.2);
                if x1 != x0 {
                    yb[x1] = k.y(d.0, d.1, d.2);
                }
            }
            let (u, v) = k.uv4(
                a.0 + b.0 + c.0 + d.0,
                a.1 + b.1 + c.1 + d.1,
                a.2 + b.2 + c.2 + d.2,
            );
            uu[cx] = u;
            vv[cx] = v;
        }
    }
}

/// Bilinear resampling of one plane (sample centres aligned).
fn scale_plane(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    // 16.16 source positions of each destination column / row.
    let pos = |d: usize, s: usize, n: usize| -> (usize, usize, u32) {
        let x = ((2 * d as i64 + 1) * s as i64 * 65536 / (2 * n as i64) - 32768).max(0);
        let i = (x >> 16) as usize;
        if i + 1 >= s {
            (s - 1, s - 1, 0)
        } else {
            (i, i + 1, (x & 0xffff) as u32)
        }
    };
    let cols: Vec<_> = (0..dw).map(|x| pos(x, sw, dw)).collect();
    let mut out = vec![0u8; dw * dh];
    let mut tmp0 = vec![0u32; dw];
    let mut tmp1 = vec![0u32; dw];
    for y in 0..dh {
        let (r0, r1, fy) = pos(y, sh, dh);
        for (t, row) in [(&mut tmp0, r0), (&mut tmp1, r1)] {
            let s = &src[row * sw..row * sw + sw];
            for (x, &(c0, c1, fx)) in cols.iter().enumerate() {
                t[x] = s[c0] as u32 * (65536 - fx) + s[c1] as u32 * fx;
            }
        }
        let o = &mut out[y * dw..(y + 1) * dw];
        for x in 0..dw {
            let v = (tmp0[x] as u64 * (65536 - fy) as u64 + tmp1[x] as u64 * fy as u64 + (1 << 31))
                >> 32;
            o[x] = v as u8;
        }
    }
    out
}

/// Bilinear scaler for I420 frames.
pub fn scale(src: &Yuv420Frame, width: u32, height: u32) -> Result<Yuv420Frame> {
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(Error::malformed("convert: invalid scale size"));
    }
    if src.y.len() != src.width as usize * src.height as usize
        || src.u.len() != src.chroma_width() * src.chroma_height()
        || src.v.len() != src.u.len()
    {
        return Err(Error::malformed("convert: plane sizes do not match"));
    }
    if src.width == width && src.height == height {
        return Ok(src.clone());
    }
    let (cw, ch) = chroma_size(width, height);
    let (scw, sch) = (src.chroma_width(), src.chroma_height());
    Yuv420Frame::from_planes(
        width,
        height,
        scale_plane(
            &src.y,
            src.width as usize,
            src.height as usize,
            width as usize,
            height as usize,
        ),
        scale_plane(&src.u, scw, sch, cw, ch),
        scale_plane(&src.v, scw, sch, cw, ch),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(fmt: PixelFormat, rgb: [u8; 3], w: u32, h: u32) -> Vec<u8> {
        let px: Vec<u8> = match fmt {
            PixelFormat::Rgba => vec![rgb[0], rgb[1], rgb[2], 255],
            PixelFormat::Bgra => vec![rgb[2], rgb[1], rgb[0], 0],
            PixelFormat::Rgb24 => rgb.to_vec(),
            PixelFormat::Bgr24 => vec![rgb[2], rgb[1], rgb[0]],
            _ => unreachable!(),
        };
        px.repeat((w * h) as usize)
    }

    #[test]
    fn bt709_limited_known_values() {
        // BT.709 limited range: white (235,128,128), black (16,128,128),
        // red (63,102,240), green (173,42,26), blue (32,240,118).
        let cases = [
            ([255, 255, 255], (235, 128, 128)),
            ([0, 0, 0], (16, 128, 128)),
            ([255, 0, 0], (63, 102, 240)),
            ([0, 255, 0], (173, 42, 26)),
            ([0, 0, 255], (32, 240, 118)),
        ];
        for fmt in [
            PixelFormat::Rgba,
            PixelFormat::Bgra,
            PixelFormat::Rgb24,
            PixelFormat::Bgr24,
        ] {
            for (rgb, (y, u, v)) in cases {
                let f = to_i420(
                    fmt,
                    5,
                    3,
                    &solid(fmt, rgb, 5, 3),
                    None,
                    ColorSpec::default(),
                )
                .unwrap();
                assert_eq!((f.y[0], f.u[0], f.v[0]), (y, u, v), "{fmt:?} {rgb:?}");
                assert_eq!(f.u.len(), 3 * 2);
                assert!(f.y.iter().all(|&s| s == y));
                assert!(f.v.iter().all(|&s| s == v));
            }
        }
    }

    #[test]
    fn bt601_full_range() {
        let spec = ColorSpec {
            matrix: Matrix::Bt601,
            full_range: true,
        };
        let f = to_i420(
            PixelFormat::Rgb24,
            2,
            2,
            &solid(PixelFormat::Rgb24, [255, 0, 0], 2, 2),
            None,
            spec,
        )
        .unwrap();
        // Y = 0.299 · 255 = 76.2, Cr = 128 + 127.5 → 255 (clipped).
        assert_eq!((f.y[0], f.u[0], f.v[0]), (76, 85, 255));
    }

    #[test]
    fn nv12_and_strides() {
        // 4x2 NV12 with a luma stride of 6.
        let mut d = vec![0u8; 6 * 2 + 6];
        d[..4].copy_from_slice(&[1, 2, 3, 4]);
        d[6..10].copy_from_slice(&[5, 6, 7, 8]);
        d[12..16].copy_from_slice(&[10, 20, 30, 40]);
        let f = to_i420(PixelFormat::Nv12, 4, 2, &d, Some(6), ColorSpec::default()).unwrap();
        assert_eq!(f.y, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(f.u, vec![10, 30]);
        assert_eq!(f.v, vec![20, 40]);
        assert!(to_i420(
            PixelFormat::Nv12,
            4,
            2,
            &d[..10],
            Some(6),
            ColorSpec::default()
        )
        .is_err());
        assert!(to_i420(
            PixelFormat::Rgba,
            4,
            2,
            &[0; 31],
            None,
            ColorSpec::default()
        )
        .is_err());
        assert!(to_i420(PixelFormat::Rgba, 0, 2, &[], None, ColorSpec::default()).is_err());
    }

    #[test]
    fn scaler() {
        let mut f = Yuv420Frame::new(4, 4);
        f.y = (0..16).map(|i| (i % 4) as u8 * 60).collect();
        let s = scale(&f, 8, 8).unwrap();
        assert_eq!(s.y.len(), 64);
        // Monotonic along x, constant along y, ends clamp to the edges.
        assert_eq!(s.y[0], 0);
        assert_eq!(s.y[7], 180);
        assert!(s.y[..8].windows(2).all(|p| p[0] <= p[1]));
        assert_eq!(&s.y[..8], &s.y[56..64]);
        let d = scale(&s, 4, 4).unwrap();
        assert_eq!(d.y[1], 60);
        let odd = scale(&f, 7, 5).unwrap();
        assert_eq!((odd.u.len(), odd.y.len()), (4 * 3, 35));
        assert!(scale(&f, 0, 4).is_err());
    }
}

#[cfg(test)]
mod oracle {
    use super::*;
    use std::process::Command;

    /// Compares with ffmpeg's swscale (luma within ±1, chroma within ±2 on
    /// smooth content). Skipped without ffmpeg.
    #[test]
    fn matches_swscale() {
        let Some(ff) = crate::media::h264enc::test_ffmpeg() else {
            eprintln!("ffmpeg not found; skipping");
            return;
        };
        let (w, h) = (64u32, 32u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[(x * 4) as u8, (y * 8) as u8, (255 - x * 2) as u8, 255]);
            }
        }
        let dir = std::env::temp_dir();
        let inp = dir.join(format!("streamlib-conv-{}.rgba", std::process::id()));
        let out = dir.join(format!("streamlib-conv-{}.yuv", std::process::id()));
        std::fs::write(&inp, &rgba).unwrap();
        let o = Command::new(ff)
            .args([
                "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", "64x32", "-i",
            ])
            .arg(&inp)
            .args([
                "-vf",
                "scale=out_color_matrix=bt709:out_range=tv:flags=accurate_rnd+full_chroma_int",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&out)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let theirs = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_file(&inp);
        let _ = std::fs::remove_file(&out);
        let ours = to_i420(PixelFormat::Rgba, w, h, &rgba, None, ColorSpec::default())
            .unwrap()
            .to_i420();
        let ys = (w * h) as usize;
        for (i, (a, b)) in ours.iter().zip(&theirs).enumerate() {
            let tol = if i < ys { 1 } else { 2 };
            assert!(a.abs_diff(*b) <= tol, "sample {i}: ours {a} ffmpeg {b}");
        }
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// `cargo test --release -- --ignored --nocapture convert_speed`.
    #[test]
    #[ignore]
    fn convert_speed() {
        let (w, h) = (1920u32, 1080u32);
        let rgba: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
        let n = 50;
        let t = std::time::Instant::now();
        for _ in 0..n {
            std::hint::black_box(
                to_i420(PixelFormat::Rgba, w, h, &rgba, None, ColorSpec::default()).unwrap(),
            );
        }
        let rgb_ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        let f = to_i420(PixelFormat::Rgba, w, h, &rgba, None, ColorSpec::default()).unwrap();
        let t = std::time::Instant::now();
        for _ in 0..n {
            std::hint::black_box(scale(&f, 1280, 720).unwrap());
        }
        let scale_ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        eprintln!("1080p rgba->i420 {rgb_ms:.2} ms, 1080p->720p scale {scale_ms:.2} ms");
    }
}
