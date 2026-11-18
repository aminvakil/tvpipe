//! The encoding pipeline: block switching, MDCT, masking estimate, M/S,
//! quantization with a rate loop, sectioning and bitstream syntax.

use super::bits::{BitCounter, BitSink, BitWriter};
use super::huffman::{self, ESC_HCB};
use super::mdct::{sine_window, Mdct};
use super::tables;
use super::FRAME_LEN;

/// window_sequence values (Table 44).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seq {
    OnlyLong = 0,
    LongStart = 1,
    EightShort = 2,
    LongStop = 3,
}

const ID_SCE: u32 = 0;
const ID_CPE: u32 = 1;
const ID_END: u32 = 7;

/// Rounding offset of the quantizer, as in
/// `int(|x|^(3/4) * 2^(-3/16 (sf-100)) + 0.4054)`. Values below 0.5 bias towards smaller magnitudes, which
/// on Laplacian-like spectra lowers both bits and mean squared error.
const ROUND: f32 = 0.4054;
const MAX_Q: i32 = 8191;
/// Noise shaping exponent: allowed noise per band grows as energy^ALPHA.
const ALPHA: f32 = 0.6;
/// Rate-loop parameter range; the noise scale is 2^(t/4).
const T_MIN: i32 = -100;
const T_MAX: i32 = 120;
/// Rate controller gains (noise-scale steps per frame of reservoir error).
const KP: f64 = 4.0;
const KI: f64 = 0.3;

/// Long-term per-channel state.
struct ChannelState {
    /// Three frames of input: the block being coded is `buf[..2048]`, the
    /// last frame is the lookahead.
    buf: Vec<f32>,
}

/// One coded band: a scalefactor band of one window group.
#[derive(Clone, Copy)]
struct Band {
    start: usize,
    end: usize,
    group: usize,
    sfb: usize,
}

/// Per-frame working data of one channel.
#[derive(Default)]
struct Chan {
    /// Spectrum in coding order (groups interleaved for short blocks).
    x: Vec<f32>,
    ax: Vec<f32>,
    a34: Vec<f32>,
    q: Vec<i32>,
    energy: Vec<f32>,
    /// Masking estimate in normalized units (time-domain variance).
    pw: Vec<f32>,
    amax: Vec<f32>,
    sf: Vec<i32>,
    zero: Vec<bool>,
    maxq: Vec<i32>,
    cb: Vec<u8>,
    global_gain: i32,
}

pub struct Core {
    channels: usize,
    swb_long: &'static [u16],
    swb_short: &'static [u16],
    /// Bands below the bandwidth limit.
    nb_long: usize,
    nb_short: usize,
    win_long: Vec<f32>,
    win_start: Vec<f32>,
    win_stop: Vec<f32>,
    win_short: Vec<f32>,
    mdct_long: Mdct,
    mdct_short: Mdct,
    ath_long: Vec<f32>,
    ath_short: Vec<f32>,
    state: Vec<ChannelState>,
    prev_seq: Seq,
    next_short: bool,
    next_attack: Option<usize>,
    // Rate control.
    mean_bits: f64,
    mean_acc: f64,
    reservoir: i64,
    reservoir_max: i64,
    last_t: i32,
    q_base: f64,
    // Scratch.
    chans: Vec<Chan>,
    bands: Vec<Band>,
    ms: Vec<bool>,
    group_len: Vec<usize>,
    tmp: Vec<f32>,
    pow_q: [f32; 256],
    pow_g: [f32; 256],
    pow43: Vec<f32>,
}

/// Absolute threshold of hearing (Terhardt), dB SPL, f in Hz.
fn ath_db(f: f64) -> f64 {
    let k = (f / 1000.0).max(0.02);
    3.64 * k.powf(-0.8) - 6.5 * (-0.6 * (k - 3.3) * (k - 3.3)).exp() + 1e-3 * k.powi(4)
}

/// Audio bandwidth for a per-channel bitrate.
fn bandwidth(bps_per_channel: u32, fs: u32) -> f64 {
    let pts = [
        (16000.0, 5500.0),
        (24000.0, 8000.0),
        (32000.0, 11000.0),
        (48000.0, 14000.0),
        (64000.0, 16000.0),
        (96000.0, 18500.0),
        (128000.0, 20000.0),
    ];
    let b = bps_per_channel as f64;
    let mut bw = pts[0].1;
    for w in pts.windows(2) {
        let ((b0, f0), (b1, f1)) = (w[0], w[1]);
        if b >= b0 {
            bw = if b >= b1 {
                f1
            } else {
                f0 + (f1 - f0) * (b - b0) / (b1 - b0)
            };
        }
    }
    bw.min(fs as f64 / 2.0)
}

fn swb_tables(fs_index: usize) -> (&'static [u16], &'static [u16]) {
    match fs_index {
        0 | 1 => (&tables::SWB_LONG_96, &tables::SWB_SHORT_96),
        2 => (&tables::SWB_LONG_64, &tables::SWB_SHORT_64),
        3 | 4 => (&tables::SWB_LONG_48, &tables::SWB_SHORT_48),
        5 => (&tables::SWB_LONG_32, &tables::SWB_SHORT_48),
        6 | 7 => (&tables::SWB_LONG_24, &tables::SWB_SHORT_24),
        8..=10 => (&tables::SWB_LONG_16, &tables::SWB_SHORT_16),
        _ => (&tables::SWB_LONG_8, &tables::SWB_SHORT_8),
    }
}

/// Minimum ATH over each band, in normalized units (LSB^2 of time-domain
/// noise variance; 0 dB at the most sensitive frequency).
fn ath_bands(swb: &[u16], bins: usize, fs: u32) -> Vec<f32> {
    let min_db = (1000..6000)
        .step_by(10)
        .map(|f| ath_db(f as f64))
        .fold(f64::INFINITY, f64::min);
    swb.windows(2)
        .map(|w| {
            let db = (w[0]..w[1])
                .map(|k| ath_db((k as f64 + 0.5) * fs as f64 / (2 * bins) as f64))
                .fold(f64::INFINITY, f64::min);
            10f64.powf(((db - min_db).min(80.0)) / 10.0) as f32
        })
        .collect()
}

impl Core {
    pub fn new(fs_index: usize, fs: u32, channels: usize, bitrate: u32) -> Core {
        let (swb_long, swb_short) = swb_tables(fs_index);
        let bw = bandwidth(bitrate / channels as u32, fs);
        let count = |swb: &[u16], bins: usize| {
            swb.windows(2)
                .take_while(|w| (w[0] as f64) * (fs as f64) / ((2 * bins) as f64) < bw)
                .count()
        };
        let nb_long = count(swb_long, 1024);
        let nb_short = count(swb_short, 128);

        let long = sine_window(2048);
        let short = sine_window(256);
        let mut win_start = vec![0f32; 2048];
        win_start[..1024].copy_from_slice(&long[..1024]);
        win_start[1024..1472].fill(1.0);
        win_start[1472..1600].copy_from_slice(&short[128..]);
        let mut win_stop = vec![0f32; 2048];
        win_stop[448..576].copy_from_slice(&short[..128]);
        win_stop[576..1024].fill(1.0);
        win_stop[1024..].copy_from_slice(&long[1024..]);

        let mean_bits = bitrate as f64 * FRAME_LEN as f64 / fs as f64;
        let reservoir_max = (6144 * channels as i64 - mean_bits.ceil() as i64).max(0);

        let mut pow_q = [0f32; 256];
        let mut pow_g = [0f32; 256];
        for sf in 0..256 {
            pow_q[sf] = 2f64.powf(-3.0 / 16.0 * (sf as f64 - 100.0)) as f32;
            pow_g[sf] = 2f64.powf(0.25 * (sf as f64 - 100.0)) as f32;
        }
        let pow43 = (0..=MAX_Q)
            .map(|q| (q as f64).powf(4.0 / 3.0) as f32)
            .collect();

        Core {
            channels,
            swb_long,
            swb_short,
            nb_long,
            nb_short,
            win_long: long,
            win_start,
            win_stop,
            win_short: short,
            mdct_long: Mdct::new(1024),
            mdct_short: Mdct::new(128),
            ath_long: ath_bands(swb_long, 1024, fs),
            ath_short: ath_bands(swb_short, 128, fs),
            state: (0..channels)
                .map(|_| ChannelState {
                    buf: vec![0.0; 3 * FRAME_LEN],
                })
                .collect(),
            prev_seq: Seq::OnlyLong,
            next_short: false,
            next_attack: None,
            mean_bits,
            mean_acc: 0.0,
            reservoir: 0,
            reservoir_max,
            last_t: 0,
            q_base: 0.0,
            chans: (0..channels).map(|_| Chan::default()).collect(),
            bands: Vec::new(),
            ms: Vec::new(),
            group_len: Vec::new(),
            tmp: vec![0.0; 2048],
            pow_q,
            pow_g,
            pow43,
        }
    }

    pub fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        let nch = self.channels;
        for (ch, st) in self.state.iter_mut().enumerate() {
            st.buf.copy_within(FRAME_LEN.., 0);
            let tail = &mut st.buf[2 * FRAME_LEN..];
            for (i, s) in tail.iter_mut().enumerate() {
                *s = pcm[i * nch + ch] as f32;
            }
        }

        // Block switching: the block being coded was decided last time;
        // decide the next one from the lookahead.
        let cur_short = self.next_short;
        let cur_attack = self.next_attack;
        let mut attack = None;
        for st in &self.state {
            if let Some(w) = detect_attack(&st.buf) {
                attack = Some(attack.map_or(w, |a: usize| a.min(w)));
            }
        }
        self.next_short = attack.is_some();
        self.next_attack = attack;
        let seq = if cur_short {
            Seq::EightShort
        } else if self.next_short {
            if self.prev_seq == Seq::EightShort {
                Seq::EightShort
            } else {
                Seq::LongStart
            }
        } else if self.prev_seq == Seq::EightShort {
            Seq::LongStop
        } else {
            Seq::OnlyLong
        };
        self.prev_seq = seq;

        self.group_len.clear();
        if seq == Seq::EightShort {
            match cur_attack {
                Some(a) => {
                    if a > 0 {
                        self.group_len.push(a);
                    }
                    self.group_len.push(1);
                    if a < 7 {
                        self.group_len.push(7 - a);
                    }
                }
                None => self.group_len.extend_from_slice(&[2, 2, 2, 2]),
            }
        } else {
            self.group_len.push(1);
        }

        self.analyze(seq);
        self.code_frame(seq)
    }

    #[cfg(test)]
    pub fn last_seq(&self) -> Seq {
        self.prev_seq
    }

    #[cfg(test)]
    pub fn last_t(&self) -> (i32, i64) {
        (self.last_t, self.reservoir)
    }

    fn short(&self, seq: Seq) -> bool {
        seq == Seq::EightShort
    }

    /// Transforms every channel and fills the per-band analysis.
    fn analyze(&mut self, seq: Seq) {
        let short = self.short(seq);
        let (swb, nb, ath) = if short {
            (self.swb_short, self.nb_short, &self.ath_short)
        } else {
            (self.swb_long, self.nb_long, &self.ath_long)
        };
        // Band layout.
        self.bands.clear();
        let mut base = 0;
        for (g, &glen) in self.group_len.iter().enumerate() {
            for sfb in 0..nb {
                self.bands.push(Band {
                    start: base + swb[sfb] as usize * glen,
                    end: base + swb[sfb + 1] as usize * glen,
                    group: g,
                    sfb,
                });
            }
            base += if short { 128 * glen } else { 1024 };
        }
        let norm = if short { 1.0 / 256.0 } else { 1.0 / 2048.0 };

        for ch in 0..self.channels {
            let buf = &self.state[ch].buf;
            let c = &mut self.chans[ch];
            c.x.resize(1024, 0.0);
            if short {
                let mut spec = [[0f32; 128]; 8];
                for (w, out) in spec.iter_mut().enumerate() {
                    let off = 448 + 128 * w;
                    let z = &mut self.tmp[..256];
                    for i in 0..256 {
                        z[i] = buf[off + i] * self.win_short[i];
                    }
                    self.mdct_short.forward(z, out);
                }
                // Interleave: group, sfb, window, bin.
                let mut k = 0;
                let mut w0 = 0;
                for &glen in &self.group_len {
                    for s in swb.windows(2) {
                        for sp in &spec[w0..w0 + glen] {
                            for &v in &sp[s[0] as usize..s[1] as usize] {
                                c.x[k] = v;
                                k += 1;
                            }
                        }
                    }
                    w0 += glen;
                }
            } else {
                let win = match seq {
                    Seq::LongStart => &self.win_start,
                    Seq::LongStop => &self.win_stop,
                    _ => &self.win_long,
                };
                let z = &mut self.tmp[..2048];
                for i in 0..2048 {
                    z[i] = buf[i] * win[i];
                }
                self.mdct_long.forward(z, &mut c.x);
            }
            c.q.resize(1024, 0);
            let nbands = self.bands.len();
            c.pw.resize(nbands, 0.0);
            for (b, band) in self.bands.iter().enumerate() {
                let e: f32 = c.x[band.start..band.end].iter().map(|v| v * v).sum();
                let w = (band.end - band.start) as f32;
                let e_norm = (e / w * norm).max(1e-9);
                c.pw[b] = e_norm.powf(ALPHA);
            }
        }
        // M/S decision for channel pairs, then the stats on what is coded.
        self.ms.clear();
        self.ms.resize(self.bands.len(), false);
        if self.channels == 2 {
            let g = 2f32.powf(self.last_t as f32 / 4.0);
            let (l, r) = self.chans.split_at_mut(1);
            let (l, r) = (&mut l[0], &mut r[0]);
            for (b, band) in self.bands.iter().enumerate() {
                let (mut el, mut er, mut em, mut es) = (0f32, 0f32, 0f32, 0f32);
                for k in band.start..band.end {
                    let (a, c) = (l.x[k], r.x[k]);
                    el += a * a;
                    er += c * c;
                    let m = 0.5 * (a + c);
                    let s = 0.5 * (a - c);
                    em += m * m;
                    es += s * s;
                }
                let w = (band.end - band.start) as f32;
                let a = ath[band.sfb];
                let tl = (g * l.pw[b]).max(a) * w / norm;
                let tr = (g * r.pw[b]).max(a) * w / norm;
                let tms = 0.5 * tl.min(tr);
                let cost = |e: f32, t: f32| (e / t).max(1.0).log2();
                if cost(em, tms) + cost(es, tms) < cost(el, tl) + cost(er, tr) {
                    self.ms[b] = true;
                    for k in band.start..band.end {
                        let (a, c) = (l.x[k], r.x[k]);
                        l.x[k] = 0.5 * (a + c);
                        r.x[k] = 0.5 * (a - c);
                    }
                }
            }
        }
        let nbands = self.bands.len();
        for c in &mut self.chans {
            c.ax.clear();
            c.ax.extend(c.x.iter().map(|v| v.abs()));
            c.a34.clear();
            c.a34
                .extend(c.ax.iter().map(|v| v.sqrt() * v.sqrt().sqrt()));
            c.energy.resize(nbands, 0.0);
            c.amax.resize(nbands, 0.0);
            for (b, band) in self.bands.iter().enumerate() {
                c.energy[b] = c.ax[band.start..band.end].iter().map(|v| v * v).sum();
                c.amax[b] = c.a34[band.start..band.end]
                    .iter()
                    .fold(0f32, |a, &v| a.max(v));
            }
            c.sf.resize(nbands, 0);
            c.zero.resize(nbands, true);
            c.maxq.resize(nbands, 0);
            c.cb.resize(nbands, 0);
        }
    }

    /// Distortion of band `[s, e)` quantized with `sf`.
    fn distortion(&self, c: &Chan, s: usize, e: usize, sf: i32) -> f32 {
        let p = self.pow_q[sf as usize];
        let gain = self.pow_g[sf as usize];
        let mut d = 0f32;
        for k in s..e {
            let q = ((c.a34[k] * p + ROUND) as i32).min(MAX_Q);
            let r = self.pow43[q as usize] * gain;
            let diff = c.ax[k] - r;
            d += diff * diff;
        }
        d
    }

    /// Smallest scalefactor that keeps every value of band `b` at or below
    /// [`MAX_Q`].
    fn sf_min(&self, c: &Chan, b: usize) -> i32 {
        let amax = c.amax[b];
        if amax <= 0.0 {
            return 0;
        }
        let mut sf = (100.0 + 16.0 / 3.0 * (amax / (MAX_Q as f32 + 0.5)).log2()).ceil() as i32;
        sf = sf.clamp(0, 255);
        while sf < 255 && amax * self.pow_q[sf as usize] + ROUND >= (MAX_Q + 1) as f32 {
            sf += 1;
        }
        sf
    }

    fn quantize_band(&self, c: &mut Chan, b: usize, sf: i32) {
        let band = self.bands[b];
        let p = self.pow_q[sf as usize];
        let mut maxq = 0;
        for k in band.start..band.end {
            let mut q = ((c.a34[k] * p + ROUND) as i32).min(MAX_Q);
            maxq = maxq.max(q);
            if c.x[k] < 0.0 {
                q = -q;
            }
            c.q[k] = q;
        }
        c.sf[b] = sf;
        c.maxq[b] = maxq;
        c.zero[b] = maxq == 0;
    }

    fn zero_band(c: &mut Chan, band: Band, b: usize) {
        c.q[band.start..band.end].fill(0);
        c.zero[b] = true;
        c.maxq[b] = 0;
    }

    /// Chooses scalefactors and quantizes every band for noise scale `t`.
    fn quantize(&mut self, t: i32, short: bool) {
        let g = 2f32.powf(t as f32 / 4.0);
        let norm = if short { 256.0 } else { 2048.0 };
        let ath = if short {
            &self.ath_short
        } else {
            &self.ath_long
        };
        let nbands = self.bands.len();
        let mut chans = std::mem::take(&mut self.chans);
        // Noise targets (total per band, MDCT domain).
        let mut targets = vec![[0f32; 2]; nbands];
        for (b, band) in self.bands.iter().enumerate() {
            let w = (band.end - band.start) as f32;
            for (ch, c) in chans.iter().enumerate() {
                targets[b][ch] = (g * c.pw[b]).max(ath[band.sfb]) * w * norm;
            }
            if self.ms[b] {
                let t = 0.5 * targets[b][0].min(targets[b][1]);
                targets[b] = [t, t];
            }
        }
        for (ch, c) in chans.iter_mut().enumerate() {
            let mut sfmins = vec![0i32; nbands];
            for b in 0..nbands {
                let band = self.bands[b];
                let target = targets[b][ch];
                if c.energy[b] <= target {
                    Self::zero_band(c, band, b);
                    continue;
                }
                let lo0 = self.sf_min(c, b);
                sfmins[b] = lo0;
                let (mut lo, mut hi) = (lo0, 256);
                if self.distortion(c, band.start, band.end, lo) <= target {
                    while hi - lo > 1 {
                        let mid = (lo + hi) / 2;
                        if self.distortion(c, band.start, band.end, mid) <= target {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                }
                self.quantize_band(c, b, lo);
            }
            // Keep scalefactor differences within -60..=60: first raise
            // the finest bands, then lower the coarsest.
            let live = |c: &Chan| (0..nbands).filter(|&b| !c.zero[b]).collect::<Vec<_>>();
            let bands = live(c);
            if let Some(floor) = bands.iter().map(|&b| sfmins[b]).max() {
                let floor = floor - 60;
                for &b in &bands {
                    if c.sf[b] < floor {
                        self.quantize_band(c, b, floor);
                        if c.zero[b] {
                            Self::zero_band(c, self.bands[b], b);
                        }
                    }
                }
            }
            let bands = live(c);
            if let Some(min) = bands.iter().map(|&b| c.sf[b]).min() {
                for &b in &bands {
                    if c.sf[b] > min + 60 {
                        self.quantize_band(c, b, min + 60);
                    }
                }
            }
            c.global_gain = bands.first().map_or(100, |&b| c.sf[b]);
        }
        self.chans = chans;
    }

    /// max_sfb shared by all channels (common window for pairs).
    fn max_sfb(&self) -> usize {
        let mut m = 0;
        for c in &self.chans {
            for (b, band) in self.bands.iter().enumerate() {
                if !c.zero[b] {
                    m = m.max(band.sfb + 1);
                }
            }
        }
        m
    }

    /// Picks codebooks per band (Viterbi over the 12 codebooks with the
    /// section header cost on changes).
    fn section(&mut self, max_sfb: usize, short: bool) {
        let len_bits = if short { 3 } else { 5 };
        let header = 4 + len_bits;
        let nsfb = if short { self.nb_short } else { self.nb_long };
        const INF: u32 = u32::MAX / 4;
        let mut chans = std::mem::take(&mut self.chans);
        for c in chans.iter_mut() {
            for g in 0..self.group_len.len() {
                let base = g * nsfb;
                let mut cost = [0u32; 12];
                let mut best = [INF; 12];
                let mut back = vec![[0u8; 12]; max_sfb];
                for (sfb, back_sfb) in back.iter_mut().enumerate() {
                    let b = base + sfb;
                    let band = self.bands[b];
                    let q = &c.q[band.start..band.end];
                    let first = huffman::first_codebook(c.maxq[b]);
                    cost[0] = if c.zero[b] { 0 } else { INF };
                    for (cb, slot) in cost.iter_mut().enumerate().skip(1) {
                        *slot = if cb >= first.max(1) && huffman::max_value(cb) >= c.maxq[b] {
                            huffman::run_bits(cb, q) + c.zero[b] as u32
                        } else {
                            INF
                        };
                    }
                    let (min_prev, arg_prev) = if sfb == 0 {
                        (0, 0)
                    } else {
                        best.iter()
                            .enumerate()
                            .map(|(i, &v)| (v, i))
                            .min()
                            .unwrap_or((INF, 0))
                    };
                    let mut next = [INF; 12];
                    for cb in 0..12 {
                        if cost[cb] >= INF {
                            continue;
                        }
                        let stay = if sfb > 0 { best[cb] } else { INF };
                        let switch = min_prev + header;
                        if stay <= switch {
                            next[cb] = stay + cost[cb];
                            back_sfb[cb] = cb as u8;
                        } else {
                            next[cb] = switch + cost[cb];
                            back_sfb[cb] = arg_prev as u8;
                        }
                    }
                    best = next;
                }
                if max_sfb == 0 {
                    continue;
                }
                let mut cb = best
                    .iter()
                    .enumerate()
                    .min_by_key(|&(_, &v)| v)
                    .map_or(0, |(i, _)| i);
                for sfb in (0..max_sfb).rev() {
                    c.cb[base + sfb] = cb as u8;
                    cb = back[sfb][cb] as usize;
                }
            }
        }
        self.chans = chans;
    }

    /// Writes (or counts) the whole raw_data_block.
    fn write(&self, out: &mut impl BitSink, seq: Seq, max_sfb: usize) {
        let short = self.short(seq);
        if self.channels == 1 {
            out.put(ID_SCE, 3);
            out.put(0, 4);
            self.write_ics(out, &self.chans[0], seq, max_sfb, false);
        } else {
            out.put(ID_CPE, 3);
            out.put(0, 4);
            out.put(1, 1); // common_window
            self.write_ics_info(out, seq, max_sfb);
            let nsfb = if short { self.nb_short } else { self.nb_long };
            let used = |g: usize, sfb: usize| self.ms[g * nsfb + sfb];
            let ngroups = self.group_len.len();
            let all = (0..ngroups).all(|g| (0..max_sfb).all(|s| used(g, s)));
            let any = (0..ngroups).any(|g| (0..max_sfb).any(|s| used(g, s)));
            if max_sfb > 0 && all {
                out.put(2, 2);
            } else if any {
                out.put(1, 2);
                for g in 0..ngroups {
                    for s in 0..max_sfb {
                        out.put(used(g, s) as u32, 1);
                    }
                }
            } else {
                out.put(0, 2);
            }
            self.write_ics(out, &self.chans[0], seq, max_sfb, true);
            self.write_ics(out, &self.chans[1], seq, max_sfb, true);
        }
        out.put(ID_END, 3);
    }

    fn write_ics_info(&self, out: &mut impl BitSink, seq: Seq, max_sfb: usize) {
        out.put(0, 1); // ics_reserved_bit
        out.put(seq as u32, 2);
        out.put(0, 1); // window_shape: sine
        if seq == Seq::EightShort {
            out.put(max_sfb as u32, 4);
            let mut grouping = 0u32;
            let mut w = 0;
            for &glen in &self.group_len {
                for i in 0..glen {
                    if w > 0 {
                        grouping = (grouping << 1) | (i > 0) as u32;
                    }
                    w += 1;
                }
            }
            out.put(grouping, 7);
        } else {
            out.put(max_sfb as u32, 6);
            out.put(0, 1); // predictor_data_present
        }
    }

    fn write_ics(&self, out: &mut impl BitSink, c: &Chan, seq: Seq, max_sfb: usize, common: bool) {
        let short = self.short(seq);
        let nsfb = if short { self.nb_short } else { self.nb_long };
        let ngroups = self.group_len.len();
        out.put(c.global_gain as u32, 8);
        if !common {
            self.write_ics_info(out, seq, max_sfb);
        }
        // section_data
        let (len_bits, esc) = if short { (3, 7) } else { (5, 31) };
        for g in 0..ngroups {
            let mut sfb = 0;
            while sfb < max_sfb {
                let cb = c.cb[g * nsfb + sfb];
                let mut end = sfb + 1;
                while end < max_sfb && c.cb[g * nsfb + end] == cb {
                    end += 1;
                }
                out.put(cb as u32, 4);
                let mut len = end - sfb;
                while len >= esc {
                    out.put(esc as u32, len_bits);
                    len -= esc;
                }
                out.put(len as u32, len_bits);
                sfb = end;
            }
        }
        // scale_factor_data
        let mut last = c.global_gain;
        for g in 0..ngroups {
            for sfb in 0..max_sfb {
                let b = g * nsfb + sfb;
                if c.cb[b] != 0 {
                    let sf = if c.zero[b] { last } else { c.sf[b] };
                    huffman::write_sf(out, sf - last);
                    last = sf;
                }
            }
        }
        out.put(0, 1); // pulse_data_present
        out.put(0, 1); // tns_data_present
        out.put(0, 1); // gain_control_data_present
                       // spectral_data
        for g in 0..ngroups {
            for sfb in 0..max_sfb {
                let b = g * nsfb + sfb;
                let cb = c.cb[b] as usize;
                if cb != 0 && cb <= ESC_HCB {
                    let band = self.bands[b];
                    huffman::write_run(out, cb, &c.q[band.start..band.end]);
                }
            }
        }
    }

    /// Quantizes at `t`, sections, and returns the exact frame size in bits
    /// (before byte alignment) and max_sfb.
    fn trial(&mut self, t: i32, short: bool) -> (usize, usize) {
        self.quantize(t, short);
        let max_sfb = self.max_sfb();
        self.section(max_sfb, short);
        (self.count(max_sfb, short), max_sfb)
    }

    fn count(&self, max_sfb: usize, short: bool) -> usize {
        let seq = if short {
            Seq::EightShort
        } else {
            Seq::OnlyLong
        };
        let mut n = BitCounter::default();
        self.write(&mut n, seq, max_sfb);
        n.0
    }

    /// Rate control: a constant-quality noise scale `t`, steered by the
    /// reservoir fullness (proportional term) and a slow integral term so
    /// the long-term rate meets the target. A frame that needs more bits
    /// than the reservoir holds gets the finest `t` that still fits.
    fn code_frame(&mut self, seq: Seq) -> Vec<u8> {
        let short = self.short(seq);
        let mean = {
            let before = self.mean_acc.floor();
            self.mean_acc += self.mean_bits;
            (self.mean_acc.floor() - before) as i64
        };
        let hard_max = 6144 * self.channels as i64;
        let avail = ((mean + self.reservoir).min(hard_max) - 7).max(0) as usize;
        let err = (self.reservoir_max / 2 - self.reservoir) as f64 / self.mean_bits.max(1.0);
        let q = self.q_base + KP * err;
        let t0 = (q.round() as i32).clamp(T_MIN, T_MAX);

        let mut t = t0;
        let (mut bits, mut max_sfb) = self.trial(t0, short);
        if bits > avail {
            // Grow the step until it fits, then bisect.
            let mut lo = t0;
            let mut step = 2;
            let mut hi = loop {
                let cand = (lo + step).min(T_MAX);
                let r = self.trial(cand, short);
                if r.0 <= avail || cand == T_MAX {
                    (bits, max_sfb) = r;
                    break cand;
                }
                lo = cand;
                step *= 2;
            };
            while hi - lo > 1 {
                let mid = (lo + hi) / 2;
                let r = self.trial(mid, short);
                if r.0 <= avail {
                    hi = mid;
                    (bits, max_sfb) = r;
                } else {
                    lo = mid;
                }
            }
            if hi != t0 && (hi - lo) == 1 && bits <= avail {
                // The last trial may have been `lo`; redo `hi`.
                (bits, max_sfb) = self.trial(hi, short);
            }
            t = hi;
        }
        if bits > avail {
            // Not even the coarsest scale fits: drop bands from the top.
            while bits > avail && max_sfb > 0 {
                for c in &mut self.chans {
                    for (b, band) in self.bands.iter().enumerate() {
                        if band.sfb + 1 >= max_sfb {
                            Self::zero_band(c, *band, b);
                        }
                    }
                }
                max_sfb = self.max_sfb();
                self.section(max_sfb, short);
                bits = self.count(max_sfb, short);
            }
        }
        self.last_t = t;
        let mut w = BitWriter::new();
        self.write(&mut w, seq, max_sfb);
        let used = w.bits().div_ceil(8) as i64 * 8;
        let level = self.reservoir + mean - used;
        if level > self.reservoir_max {
            // Bits left over with a full reservoir are lost: the quality
            // can go up for free.
            self.q_base -= 4.0 * (level - self.reservoir_max) as f64 / self.mean_bits.max(1.0);
        }
        self.reservoir = level.clamp(0, self.reservoir_max);
        self.q_base = (self.q_base + KI * err).clamp(T_MIN as f64, T_MAX as f64);
        w.finish()
    }
}

/// Finds an attack in the block after the one being coded (`buf[1024..]`):
/// returns the short window (0..8) whose centre segment holds it.
fn detect_attack(buf: &[f32]) -> Option<usize> {
    // High-passed energy of 128-sample segments; segment s covers
    // buf[128 s..128 s + 128]. Short window j of the next block is centred
    // on segment 12 + j.
    let mut e = [0f32; 24];
    let mut prev = buf[0];
    for (s, seg) in buf.chunks_exact(128).enumerate().take(24) {
        let mut acc = 0f32;
        for &v in seg {
            let d = v - prev;
            prev = v;
            acc += d * d;
        }
        e[s] = acc;
    }
    let floor = 128.0 * 30.0 * 30.0;
    (0..8).find(|&j| {
        let s = 12 + j;
        let past = (e[s - 4] + e[s - 3] + e[s - 2] + e[s - 1]) / 4.0;
        e[s] > floor && e[s] > 10.0 * past
    })
}
