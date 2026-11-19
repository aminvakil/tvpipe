//! Forward MDCT and the analysis windows.
//!
//! Definition (ISO/IEC 13818-7 §15.3.1 gives the inverse; the forward
//! transform pairs with it):
//! `X[k] = 2 * sum_{n=0}^{N-1} z[n] cos(2π/N (n + n0)(k + 1/2))`,
//! `n0 = (N/2 + 1)/2`, `k < N/2`, where `z` is the windowed input.
//!
//! Fast path: the MDCT of `(a, b, c, d)` (quarters) equals the DCT-IV of
//! `(-c_r - d, a - b_r)` (Wikipedia, "Modified discrete cosine
//! transform"); the DCT-IV of length M is computed with an M/2-point
//! complex FFT and pre/post twiddles (derived in the notes below).

use std::f64::consts::PI;

/// Iterative radix-2 complex FFT (forward, `exp(-2πi nk/n)`).
struct Fft {
    n: usize,
    rev: Vec<u32>,
    tw: Vec<(f32, f32)>,
}

impl Fft {
    fn new(n: usize) -> Fft {
        debug_assert!(n.is_power_of_two());
        let bits = n.trailing_zeros();
        let rev = (0..n as u32)
            .map(|i| {
                if bits == 0 {
                    0
                } else {
                    i.reverse_bits() >> (32 - bits)
                }
            })
            .collect();
        let tw = (0..n / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        Fft { n, rev, tw }
    }

    fn run(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        for (i, &j) in self.rev.iter().enumerate().take(n) {
            let j = j as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = self.tw[k * step];
                    let a = start + k;
                    let b = a + half;
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
    }
}

/// MDCT producing `m` coefficients from `2m` windowed samples.
pub struct Mdct {
    m: usize,
    fft: Fft,
    pre: Vec<(f32, f32)>,
    post: Vec<(f32, f32)>,
    v: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
}

// DCT-IV via FFT: with c[n] = v[2n] + i v[M-1-2n] and
// C[k] = e^{-iπ(k+1/4)/M} · FFT_{M/2}(c[n] e^{-iπn/M})[k],
// Y[2k] = Re C[k] and Y[M-1-2k] = -Im C[k] (expand (2n+1/2)(2k+1/2)
// = 4nk + n + k + 1/4 and use the symmetries of the cosine for the odd
// indices; M even).
impl Mdct {
    pub fn new(m: usize) -> Mdct {
        let h = m / 2;
        let pre = (0..h)
            .map(|n| {
                let a = -PI * n as f64 / m as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        let post = (0..h)
            .map(|k| {
                let a = -PI * (k as f64 + 0.25) / m as f64;
                // Includes the transform's factor 2.
                ((2.0 * a.cos()) as f32, (2.0 * a.sin()) as f32)
            })
            .collect();
        Mdct {
            m,
            fft: Fft::new(h),
            pre,
            post,
            v: vec![0.0; m],
            re: vec![0.0; h],
            im: vec![0.0; h],
        }
    }

    /// `input.len() == 2m` (already windowed), `out.len() == m`.
    pub fn forward(&mut self, input: &[f32], out: &mut [f32]) {
        let m = self.m;
        let q = m / 2;
        debug_assert_eq!(input.len(), 2 * m);
        debug_assert_eq!(out.len(), m);
        let (a, rest) = input.split_at(q);
        let (b, rest) = rest.split_at(q);
        let (c, d) = rest.split_at(q);
        for n in 0..q {
            self.v[n] = -c[q - 1 - n] - d[n];
            self.v[q + n] = a[n] - b[q - 1 - n];
        }
        let h = m / 2;
        for n in 0..h {
            let xr = self.v[2 * n];
            let xi = self.v[m - 1 - 2 * n];
            let (pr, pi) = self.pre[n];
            self.re[n] = xr * pr - xi * pi;
            self.im[n] = xr * pi + xi * pr;
        }
        self.fft.run(&mut self.re, &mut self.im);
        for k in 0..h {
            let (pr, pi) = self.post[k];
            let cr = self.re[k] * pr - self.im[k] * pi;
            let ci = self.re[k] * pi + self.im[k] * pr;
            out[2 * k] = cr;
            out[m - 1 - 2 * k] = -ci;
        }
    }
}

/// Sine window of length `n` (§15.3.2): `sin(π/n (i + 1/2))`.
pub fn sine_window(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (PI / n as f64 * (i as f64 + 0.5)).sin() as f32)
        .collect()
}

/// Zeroth-order modified Bessel function of the first kind (series).
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let h = x / 2.0;
    for k in 1..60 {
        term *= h / k as f64;
        let t2 = term * term;
        sum += t2;
        if t2 < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Kaiser-Bessel-derived window of length `n` (§15.3.2), alpha 4 for 2048
/// and 6 for 256.
pub fn kbd_window(n: usize, alpha: f64) -> Vec<f32> {
    let half = n / 2;
    let kernel: Vec<f64> = (0..=half)
        .map(|i| {
            let r = (i as f64 - n as f64 / 4.0) / (n as f64 / 4.0);
            bessel_i0(PI * alpha * (1.0 - r * r).max(0.0).sqrt())
        })
        .collect();
    let total: f64 = kernel.iter().sum();
    let mut w = vec![0f32; n];
    let mut acc = 0.0;
    for i in 0..half {
        acc += kernel[i];
        let v = (acc / total).sqrt() as f32;
        w[i] = v;
        w[n - 1 - i] = v;
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn direct(z: &[f32]) -> Vec<f64> {
        let n = z.len();
        let n0 = (n as f64 / 2.0 + 1.0) / 2.0;
        (0..n / 2)
            .map(|k| {
                2.0 * z
                    .iter()
                    .enumerate()
                    .map(|(i, &x)| {
                        x as f64 * (2.0 * PI / n as f64 * (i as f64 + n0) * (k as f64 + 0.5)).cos()
                    })
                    .sum::<f64>()
            })
            .collect()
    }

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (*seed >> 8) as f32 / (1 << 24) as f32 - 0.5
    }

    #[test]
    fn matches_direct_definition() {
        for m in [128usize, 1024] {
            let mut seed = 7;
            let z: Vec<f32> = (0..2 * m).map(|_| lcg(&mut seed) * 1000.0).collect();
            let mut out = vec![0.0; m];
            Mdct::new(m).forward(&z, &mut out);
            let want = direct(&z);
            let peak = want.iter().fold(0f64, |a, &b| a.max(b.abs()));
            for k in 0..m {
                assert!(
                    (out[k] as f64 - want[k]).abs() < peak * 1e-5,
                    "m={m} k={k}: {} vs {}",
                    out[k],
                    want[k]
                );
            }
        }
    }

    /// Princen-Bradley condition: w[i]^2 + w[i + n/2]^2 == 1.
    #[test]
    fn windows_are_power_complementary() {
        for w in [
            sine_window(2048),
            sine_window(256),
            kbd_window(2048, 4.0),
            kbd_window(256, 6.0),
        ] {
            let h = w.len() / 2;
            for i in 0..h {
                let s = w[i] * w[i] + w[i + h] * w[i + h];
                assert!((s - 1.0).abs() < 1e-5, "{s}");
            }
        }
    }
}
