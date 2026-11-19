//! QR codes of the links a person opens on another device (the public
//! `--tunnel` URLs), drawn in the terminal so a phone can scan them.
//!
//! ISO/IEC 18004 Model 2 symbols, byte mode only (the text's UTF-8 bytes,
//! no ECI): error correction level M, or L when M does not fit, at the
//! smallest version that fits. Reed–Solomon over GF(256) (0x11D), blocks
//! interleaved, finder/timing/alignment patterns, BCH-coded format and
//! version information, and the mask with the lowest penalty (N1–N4).
//! The standard leaves parts of the penalty rules open; they are read as
//! Nayuki's QR Code generator reads them (each mask scored with its format
//! information drawn, the 1:1:3:1:1 rule at any module width with the
//! outside of the symbol light), so the known-answer tests compare whole
//! symbols with its output (`tests/vectors/qr/`).

use std::io::{IsTerminal, Write};

/// Error correction level (only the two a terminal QR code needs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ecl {
    /// Recovers ~7%.
    L,
    /// Recovers ~15%.
    M,
}

impl Ecl {
    fn index(self) -> usize {
        match self {
            Ecl::L => 0,
            Ecl::M => 1,
        }
    }

    /// The two bits in the format information.
    fn format_bits(self) -> u32 {
        match self {
            Ecl::L => 1,
            Ecl::M => 0,
        }
    }
}

pub const MAX_VERSION: u8 = 40;

/// The largest version drawn in a terminal: 69 modules plus the quiet
/// zone fit 80 columns.
pub const TERMINAL_MAX_VERSION: u8 = 13;

/// Light modules around the symbol in the terminal (the standard asks
/// for 4; phone cameras read 2, and it keeps the code small).
const QUIET: usize = 2;

/// Error correction codewords per block, by level (L, M) and version.
const ECC_PER_BLOCK: [[u8; 41]; 2] = [
    [
        0, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28, 28,
        30, 30, 26, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
    [
        0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28,
        28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
    ],
];

/// Error correction blocks, by level (L, M) and version.
const BLOCKS: [[u8; 41]; 2] = [
    [
        0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12, 12, 13,
        14, 15, 16, 17, 18, 19, 19, 20, 21, 22, 24, 25,
    ],
    [
        0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21,
        23, 25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49,
    ],
];

/// A QR code symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Qr {
    pub version: u8,
    pub ecl: Ecl,
    pub mask: u8,
    /// Modules per side.
    pub size: usize,
    /// Row by row, `true` is dark.
    modules: Vec<bool>,
}

impl Qr {
    /// Whether the module in column `x`, row `y` is dark (outside: light).
    pub fn dark(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.modules[y * self.size + x]
    }
}

impl Qr {
    /// One line per module row, `1` dark and `0` light (`tvpipe debug qr --rows`).
    pub fn rows(&self) -> Vec<String> {
        (0..self.size)
            .map(|y| {
                (0..self.size)
                    .map(|x| if self.dark(x, y) { '1' } else { '0' })
                    .collect()
            })
            .collect()
    }

    /// A standalone SVG document: one module per user unit, `quiet`
    /// light modules around the symbol, each row's dark runs as one path
    /// segment (`tvpipe debug qr --svg`; scale it with CSS or `width`).
    pub fn to_svg(&self, quiet: usize) -> String {
        let side = self.size + 2 * quiet;
        let mut d = String::new();
        for y in 0..self.size {
            let mut x = 0;
            while x < self.size {
                if !self.dark(x, y) {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < self.size && self.dark(x, y) {
                    x += 1;
                }
                let w = x - start;
                d.push_str(&format!("M{} {}h{w}v1h-{w}z", start + quiet, y + quiet));
            }
        }
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side} {side}\" \
             shape-rendering=\"crispEdges\"><rect width=\"{side}\" height=\"{side}\" \
             fill=\"#fff\"/><path d=\"{d}\" fill=\"#000\"/></svg>\n"
        )
    }
}

/// `data` in byte mode at level M, else L, at the smallest version up to
/// `max_version` that fits (`None`: too long).
pub fn encode(data: &[u8], max_version: u8) -> Option<Qr> {
    let max = max_version.min(MAX_VERSION);
    [Ecl::M, Ecl::L].into_iter().find_map(|ecl| {
        (1..=max)
            .find(|&v| fits(data.len(), v, ecl))
            .map(|v| encode_at(data, v, ecl, None))
    })
}

/// The URL's QR code drawn with half blocks, two module rows per line,
/// dark on white with explicit colors (any terminal theme), each line
/// ending in an SGR reset and a newline. `None` when it does not fit
/// [`TERMINAL_MAX_VERSION`].
pub fn render_terminal(text: &str) -> Option<String> {
    let qr = encode(text.as_bytes(), TERMINAL_MAX_VERSION)?;
    Some(draw(&qr))
}

/// Whether QR codes go to stderr: a person at a terminal who did not
/// say `--no-qr` (`no_qr`) or `TVPIPE_NO_QR` (not empty or `0`), and not
/// `TERM=dumb`. The caller rules out `--machine` (its events carry the
/// URL).
pub fn wanted(no_qr: bool) -> bool {
    !no_qr
        && !std::env::var_os("TVPIPE_NO_QR").is_some_and(|v| !v.is_empty() && v != "0")
        && !std::env::var_os("TERM").is_some_and(|t| t == "dumb")
        && std::io::stderr().is_terminal()
}

/// Prints the QR code of a link another device should open to stderr
/// (after the log line that names it), if it fits.
pub fn print(url: &str) {
    match render_terminal(url) {
        // One write under the lock: log lines of other threads stay out.
        Some(art) => drop(std::io::stderr().lock().write_all(art.as_bytes())),
        None => debug!("qr: the link is too long for a QR code in the terminal"),
    }
}

fn draw(qr: &Qr) -> String {
    // Black on white from the 256-color palette (16, 231), which themes
    // do not remap, unlike the 8 basic colors.
    const COLORS: &str = "\x1b[38;5;16m\x1b[48;5;231m";
    const RESET: &str = "\x1b[0m";
    let side = qr.size + 2 * QUIET;
    let dark = |x: usize, y: usize| x >= QUIET && y >= QUIET && qr.dark(x - QUIET, y - QUIET);
    let mut out = String::new();
    // An odd last row gets a light lower half: a little more quiet zone.
    for y in (0..side).step_by(2) {
        out.push_str(COLORS);
        for x in 0..side {
            out.push(match (dark(x, y), dark(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        out.push_str(RESET);
        out.push('\n');
    }
    out
}

// --- Capacity -------------------------------------------------------------

/// Modules for data and error correction (after every function pattern
/// and the format and version information), remainder bits included.
fn raw_data_modules(version: u8) -> usize {
    let v = version as usize;
    let mut n = (16 * v + 128) * v + 64;
    if v >= 2 {
        let align = v / 7 + 2;
        n -= (25 * align - 10) * align - 55;
        if v >= 7 {
            n -= 36;
        }
    }
    n
}

fn data_codewords(version: u8, ecl: Ecl) -> usize {
    let v = version as usize;
    raw_data_modules(version) / 8
        - ECC_PER_BLOCK[ecl.index()][v] as usize * BLOCKS[ecl.index()][v] as usize
}

/// Bits of the byte mode's character count.
fn count_bits(version: u8) -> usize {
    if version <= 9 {
        8
    } else {
        16
    }
}

fn fits(len: usize, version: u8, ecl: Ecl) -> bool {
    len < 1 << count_bits(version)
        && 4 + count_bits(version) + 8 * len <= 8 * data_codewords(version, ecl)
}

// --- Codewords ------------------------------------------------------------

struct Bits {
    bytes: Vec<u8>,
    len: usize,
}

impl Bits {
    fn push(&mut self, value: u32, n: usize) {
        for i in (0..n).rev() {
            if self.len % 8 == 0 {
                self.bytes.push(0);
            }
            if (value >> i) & 1 == 1 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 0x80 >> (self.len % 8);
            }
            self.len += 1;
        }
    }
}

/// The data codewords: mode, count, bytes, terminator, padding.
fn data_stream(data: &[u8], version: u8, ecl: Ecl) -> Vec<u8> {
    let capacity = data_codewords(version, ecl);
    let mut bits = Bits {
        bytes: Vec::with_capacity(capacity),
        len: 0,
    };
    bits.push(0b0100, 4);
    bits.push(data.len() as u32, count_bits(version));
    for &b in data {
        bits.push(b.into(), 8);
    }
    let terminator = (capacity * 8 - bits.len).min(4);
    bits.push(0, terminator);
    let mut bytes = bits.bytes;
    for pad in [0xEC, 0x11].into_iter().cycle() {
        if bytes.len() >= capacity {
            break;
        }
        bytes.push(pad);
    }
    bytes
}

fn gf_mul(a: u8, b: u8) -> u8 {
    let (mut a, mut b, mut r) = (a as u16, b, 0u16);
    while b != 0 {
        if b & 1 == 1 {
            r ^= a;
        }
        a <<= 1;
        if a & 0x100 != 0 {
            a ^= 0x11D;
        }
        b >>= 1;
    }
    r as u8
}

/// The generator polynomial Π (x − α^i), i < degree, without its leading
/// 1, highest power first.
fn rs_generator(degree: usize) -> Vec<u8> {
    let mut g = vec![0u8; degree];
    g[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            g[j] = gf_mul(g[j], root);
            if j + 1 < degree {
                g[j] ^= g[j + 1];
            }
        }
        root = gf_mul(root, 2);
    }
    g
}

/// The remainder of `data`·x^degree divided by the generator.
fn rs_remainder(data: &[u8], generator: &[u8]) -> Vec<u8> {
    let mut rem = vec![0u8; generator.len()];
    for &b in data {
        let factor = b ^ rem[0];
        rem.rotate_left(1);
        let last = rem.len() - 1;
        rem[last] = 0;
        for (r, &g) in rem.iter_mut().zip(generator) {
            *r ^= gf_mul(g, factor);
        }
    }
    rem
}

/// Data codewords split into blocks (the short ones first), each with
/// its error correction, then interleaved: data, then error correction.
fn interleave(data: &[u8], version: u8, ecl: Ecl) -> Vec<u8> {
    let v = version as usize;
    let blocks = BLOCKS[ecl.index()][v] as usize;
    let ecc = ECC_PER_BLOCK[ecl.index()][v] as usize;
    let short = data.len() / blocks;
    let long_blocks = data.len() % blocks;
    let generator = rs_generator(ecc);
    let mut parts = Vec::with_capacity(blocks);
    let mut at = 0;
    for i in 0..blocks {
        let len = short + usize::from(i >= blocks - long_blocks);
        let d = &data[at..at + len];
        at += len;
        parts.push((d, rs_remainder(d, &generator)));
    }
    let mut out = Vec::with_capacity(raw_data_modules(version) / 8);
    for i in 0..=short {
        out.extend(parts.iter().filter_map(|(d, _)| d.get(i)));
    }
    for i in 0..ecc {
        out.extend(parts.iter().map(|(_, e)| e[i]));
    }
    out
}

// --- Matrix ---------------------------------------------------------------

struct Grid {
    size: usize,
    dark: Vec<bool>,
    function: Vec<bool>,
}

impl Grid {
    fn set(&mut self, x: usize, y: usize, dark: bool) {
        let i = y * self.size + x;
        self.dark[i] = dark;
        self.function[i] = true;
    }
}

fn alignment_positions(version: u8) -> Vec<usize> {
    if version == 1 {
        return Vec::new();
    }
    let v = version as usize;
    let n = v / 7 + 2;
    let step = if v == 32 {
        26
    } else {
        (v * 4 + n * 2 + 1) / (n * 2 - 2) * 2
    };
    let size = 4 * v + 17;
    let mut pos: Vec<usize> = (0..n - 1).map(|i| size - 7 - i * step).collect();
    pos.push(6);
    pos.reverse();
    pos
}

/// Timing, finder (with separators) and alignment patterns, the version
/// information, the format area (light) and the dark module.
fn function_patterns(version: u8) -> Grid {
    let size = 4 * version as usize + 17;
    let mut g = Grid {
        size,
        dark: vec![false; size * size],
        function: vec![false; size * size],
    };
    for i in 0..size {
        g.set(6, i, i % 2 == 0);
        g.set(i, 6, i % 2 == 0);
    }
    for (cx, cy) in [(3, 3), (size - 4, 3), (3, size - 4)] {
        for dy in -4i32..=4 {
            for dx in -4i32..=4 {
                let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                if (0..size as i32).contains(&x) && (0..size as i32).contains(&y) {
                    let d = dx.abs().max(dy.abs());
                    g.set(x as usize, y as usize, d != 2 && d != 4);
                }
            }
        }
    }
    let pos = alignment_positions(version);
    let last = pos.len().wrapping_sub(1);
    for (i, &cy) in pos.iter().enumerate() {
        for (j, &cx) in pos.iter().enumerate() {
            if (i == 0 && j == 0) || (i == 0 && j == last) || (i == last && j == 0) {
                continue;
            }
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let d = dx.abs().max(dy.abs());
                    g.set((cx as i32 + dx) as usize, (cy as i32 + dy) as usize, d != 1);
                }
            }
        }
    }
    // Reserved here, drawn for each mask.
    draw_format(&mut g, 0);
    if version >= 7 {
        draw_version(&mut g, version_bits(version));
    }
    g
}

/// The 15 format bits (BCH(15,5), masked with 0x5412) in both copies,
/// and the dark module.
fn draw_format(g: &mut Grid, bits: u32) {
    let size = g.size;
    let bit = |i: u32| (bits >> i) & 1 == 1;
    for i in 0..=5 {
        g.set(8, i as usize, bit(i));
    }
    g.set(8, 7, bit(6));
    g.set(8, 8, bit(7));
    g.set(7, 8, bit(8));
    for i in 9..15 {
        g.set(14 - i as usize, 8, bit(i));
    }
    for i in 0..8 {
        g.set(size - 1 - i as usize, 8, bit(i));
    }
    for i in 8..15 {
        g.set(8, size - 15 + i as usize, bit(i));
    }
    g.set(8, size - 8, true);
}

fn format_bits(ecl: Ecl, mask: u8) -> u32 {
    let data = ecl.format_bits() << 3 | mask as u32;
    let mut rem = data;
    for _ in 0..10 {
        rem = (rem << 1) ^ ((rem >> 9) * 0x537);
    }
    (data << 10 | rem) ^ 0x5412
}

/// The 18 version bits (BCH(18,6)) in both corners.
fn draw_version(g: &mut Grid, bits: u32) {
    let size = g.size;
    for i in 0..18 {
        let dark = (bits >> i) & 1 == 1;
        let (a, b) = (size - 11 + i % 3, i / 3);
        g.set(a, b, dark);
        g.set(b, a, dark);
    }
}

fn version_bits(version: u8) -> u32 {
    let v = version as u32;
    let mut rem = v;
    for _ in 0..12 {
        rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
    }
    v << 12 | rem
}

/// The codewords' bits in two-module columns, zigzagging up and down
/// from the right; the remainder bits stay light.
fn place(g: &mut Grid, codewords: &[u8]) {
    let size = g.size;
    let total = codewords.len() * 8;
    let mut i = 0;
    let mut right = size - 1;
    loop {
        if right == 6 {
            right = 5;
        }
        let upward = (right + 1) & 2 == 0;
        for vert in 0..size {
            let y = if upward { size - 1 - vert } else { vert };
            for x in [right, right - 1] {
                let at = y * size + x;
                if !g.function[at] && i < total {
                    g.dark[at] = (codewords[i / 8] >> (7 - i % 8)) & 1 == 1;
                    i += 1;
                }
            }
        }
        if right < 2 {
            break;
        }
        right -= 2;
    }
}

fn mask_bit(mask: u8, x: usize, y: usize) -> bool {
    match mask {
        0 => (x + y) % 2 == 0,
        1 => y % 2 == 0,
        2 => x % 3 == 0,
        3 => (x + y) % 3 == 0,
        4 => (x / 3 + y / 2) % 2 == 0,
        5 => x * y % 2 + x * y % 3 == 0,
        6 => (x * y % 2 + x * y % 3) % 2 == 0,
        _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
    }
}

fn apply_mask(g: &Grid, mask: u8) -> Vec<bool> {
    let mut out = g.dark.clone();
    for y in 0..g.size {
        for x in 0..g.size {
            let at = y * g.size + x;
            if !g.function[at] && mask_bit(mask, x, y) {
                out[at] = !out[at];
            }
        }
    }
    out
}

// --- Penalty --------------------------------------------------------------

/// The penalty of a masked symbol: N1 (runs of five or more), N2 (2×2
/// blocks), N3 (finder-like patterns), N4 (dark/light balance).
fn penalty(m: &[bool], size: usize) -> u32 {
    let at = |x: usize, y: usize| m[y * size + x];
    let mut score = 0;
    let mut line = vec![false; size];
    let mut runs = Vec::with_capacity(size + 2);
    for vertical in [false, true] {
        for a in 0..size {
            for (b, cell) in line.iter_mut().enumerate() {
                *cell = if vertical { at(a, b) } else { at(b, a) };
            }
            // Run lengths, starting and ending with a light run that
            // includes the light outside (`size` modules; empty if the
            // line starts or ends dark).
            runs.clear();
            runs.push(size);
            let mut color = false;
            for &dark in &line {
                if dark != color {
                    runs.push(0);
                    color = dark;
                }
                if let Some(r) = runs.last_mut() {
                    *r += 1;
                }
            }
            if color {
                runs.push(0);
            }
            let last = runs.len() - 1;
            runs[last] += size;
            for (i, &run) in runs.iter().enumerate() {
                // The light runs at both ends include the outside.
                let inside = run - size * (usize::from(i == 0) + usize::from(i == last));
                if inside >= 5 {
                    score += 3 + (inside - 5) as u32;
                }
            }
            score += 40 * finder_like(&runs);
        }
    }
    for y in 1..size {
        for x in 1..size {
            let c = at(x, y);
            if at(x - 1, y) == c && at(x, y - 1) == c && at(x - 1, y - 1) == c {
                score += 3;
            }
        }
    }
    // The smallest k with (45 − 5k)% ≤ dark ≤ (55 + 5k)%.
    let dark = m.iter().filter(|&&d| d).count() as u32;
    let total = (size * size) as u32;
    let k = (20 * dark).abs_diff(10 * total).div_ceil(total) - 1;
    score + 10 * k
}

/// Dark-light-dark-dark-dark-light-dark in the ratio 1:1:3:1:1 (any
/// width n), with 4n light on one side and n on the other: each side
/// with 4n counts once. `runs` alternate light and dark, light first.
fn finder_like(runs: &[usize]) -> u32 {
    let mut count = 0;
    for w in runs.windows(7).step_by(2) {
        let n = w[1];
        if n > 0 && w[2] == n && w[3] == 3 * n && w[4] == n && w[5] == n {
            count += u32::from(w[0] >= 4 * n && w[6] >= n);
            count += u32::from(w[6] >= 4 * n && w[0] >= n);
        }
    }
    count
}

/// The symbol for `data` at this version and level; `mask` `None`: the
/// one with the lowest penalty (the first on a tie).
fn encode_at(data: &[u8], version: u8, ecl: Ecl, mask: Option<u8>) -> Qr {
    let codewords = interleave(&data_stream(data, version, ecl), version, ecl);
    let mut g = function_patterns(version);
    place(&mut g, &codewords);
    let masked = |m: u8| {
        let mut out = Grid {
            size: g.size,
            dark: apply_mask(&g, m),
            function: g.function.clone(),
        };
        draw_format(&mut out, format_bits(ecl, m));
        out
    };
    let (mask, out) = match mask {
        Some(m) => (m, masked(m)),
        None => (0..8)
            .map(|m| (m, masked(m)))
            .min_by_key(|(_, out)| penalty(&out.dark, out.size))
            .unwrap_or_else(|| (0, masked(0))),
    };
    Qr {
        version,
        ecl,
        mask,
        size: out.size,
        modules: out.dark,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::vectors;

    fn ecl(s: &str) -> Ecl {
        match s {
            "L" => Ecl::L,
            "M" => Ecl::M,
            _ => panic!("level {s}"),
        }
    }

    fn matrix_string(qr: &Qr) -> String {
        let mut s = String::new();
        for y in 0..qr.size {
            for x in 0..qr.size {
                s.push(if qr.dark(x, y) { '1' } else { '0' });
            }
        }
        s
    }

    /// Whole symbols, the chosen version and mask, and every mask's
    /// penalty against qrcodegen's.
    #[test]
    fn known_symbols() {
        let blocks = vectors::load("qr/symbols.txt");
        let mut n = 0;
        for b in blocks.iter().filter(|b| b.get("data").is_some()) {
            let name = b.name();
            let data = b.hex("data");
            let level = ecl(b.str("error"));
            let version: u8 = b.parse("version");
            let ours: Vec<String> = (0..8)
                .map(|m| {
                    let qr = encode_at(&data, version, level, Some(m));
                    penalty(&qr.modules, qr.size).to_string()
                })
                .collect();
            assert_eq!(ours.join(" "), b.str("penalties"), "{name}: penalties");
            let qr = encode_at(&data, version, level, None);
            assert_eq!(qr.mask, b.parse::<u8>("mask"), "{name}: mask");
            assert_eq!(matrix_string(&qr), b.str("matrix"), "{name}: matrix");
            // The automatic choice: the smallest version at M (else L).
            let auto = encode(&data, TERMINAL_MAX_VERSION).expect(&name);
            assert_eq!(auto, qr, "{name}");
            n += 1;
        }
        assert_eq!(n, 8);
    }

    /// Block structure and data capacity of every version, L and M,
    /// against segno's tables.
    #[test]
    fn block_structure() {
        let blocks = vectors::load("qr/symbols.txt");
        let b = blocks.iter().find(|b| b.name() == "blocks").unwrap();
        for (key, level) in [("l", Ecl::L), ("m", Ecl::M)] {
            let theirs: Vec<&str> = b.str(key).split(' ').collect();
            assert_eq!(theirs.len(), 40);
            for v in 1..=40u8 {
                let i = level.index();
                let ours = format!(
                    "{}/{}/{}",
                    BLOCKS[i][v as usize],
                    ECC_PER_BLOCK[i][v as usize],
                    data_codewords(v, level)
                );
                assert_eq!(ours, theirs[v as usize - 1], "{key} version {v}");
            }
        }
    }

    #[test]
    fn every_version_builds() {
        // The codewords fill the data area exactly, up to the remainder bits.
        for v in 1..=MAX_VERSION {
            let g = function_patterns(v);
            let free = g.function.iter().filter(|&&f| !f).count();
            assert_eq!(free, raw_data_modules(v), "version {v}");
            for level in [Ecl::L, Ecl::M] {
                let cap = (8 * data_codewords(v, level) - 4 - count_bits(v)) / 8;
                let data = vec![b'x'; cap.min((1 << count_bits(v)) - 1)];
                assert!(fits(data.len(), v, level));
                assert!(!fits(cap + 1, v, level));
                let qr = encode_at(&data, v, level, None);
                assert_eq!(qr.size, 4 * v as usize + 17);
            }
        }
    }

    #[test]
    fn gf_and_generator() {
        // α^255 = 1 and α^8 = 0x1D (the field polynomial 0x11D).
        let mut a = 1u8;
        for i in 1..=255 {
            a = gf_mul(a, 2);
            if i == 8 {
                assert_eq!(a, 0x1D);
            }
        }
        assert_eq!(a, 1);
        // ISO/IEC 18004 Annex A, degree 7: α^87 α^229 α^146 α^149 α^238 α^102 α^21.
        assert_eq!(rs_generator(7), [127, 122, 154, 164, 11, 68, 117]);
        // ISO/IEC 18004 7.9.1: M, mask 5 -> 100000011001110.
        assert_eq!(format_bits(Ecl::M, 5), 0b100_0000_1100_1110);
        // 7.10: version 7 -> 000111110010010100.
        assert_eq!(version_bits(7), 0b00_0111_1100_1001_0100);
    }

    #[test]
    fn rows_and_svg() {
        let qr = encode(b"tvpipe", MAX_VERSION).unwrap();
        let rows = qr.rows();
        assert_eq!(rows.len(), 21);
        // The finder pattern's top row and the timing pattern.
        assert!(rows[0].starts_with("1111111"));
        assert_eq!(&rows[6][8..13], "10101");
        let svg = qr.to_svg(4);
        assert!(svg.starts_with(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 29 29\" shape-rendering=\"crispEdges\">"
        ));
        // The first dark run: the top finder row, 7 modules, inside the quiet zone.
        assert!(svg.contains("<path d=\"M4 4h7v1h-7z"), "{svg}");
        // Every dark module is covered by exactly one run.
        let runs: usize = svg
            .split('h')
            .skip(1)
            .step_by(2)
            .filter_map(|r| r.split('v').next()?.parse::<usize>().ok())
            .sum();
        let dark = rows.iter().map(|r| r.matches('1').count()).sum::<usize>();
        assert_eq!(runs, dark);
    }

    #[test]
    fn too_long() {
        let data = vec![b'x'; 426];
        assert!(encode(&data, TERMINAL_MAX_VERSION).is_none());
        assert!(render_terminal(&"x".repeat(426)).is_none());
        let qr = encode(&data, MAX_VERSION).unwrap();
        assert_eq!((qr.version, qr.ecl), (16, Ecl::M));
        assert!(encode(&vec![0; 2954], MAX_VERSION).is_none());
        assert_eq!(encode(&vec![0; 2953], MAX_VERSION).unwrap().ecl, Ecl::L);
    }

    #[test]
    fn terminal_drawing() {
        let art = render_terminal("tvpipe").unwrap();
        let qr = encode(b"tvpipe", TERMINAL_MAX_VERSION).unwrap();
        let side = qr.size + 2 * QUIET;
        let lines: Vec<&str> = art.lines().collect();
        assert_eq!(lines.len(), side.div_ceil(2));
        for (row, line) in lines.iter().enumerate() {
            let body = line
                .strip_prefix("\x1b[38;5;16m\x1b[48;5;231m")
                .and_then(|l| l.strip_suffix("\x1b[0m"))
                .unwrap();
            let cells: Vec<char> = body.chars().collect();
            assert_eq!(cells.len(), side);
            for (x, c) in cells.into_iter().enumerate() {
                let module = |y: usize| x >= QUIET && y >= QUIET && qr.dark(x - QUIET, y - QUIET);
                let (top, bottom) = (module(2 * row), module(2 * row + 1));
                let want = match (top, bottom) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                };
                assert_eq!(c, want, "line {row} column {x}");
            }
        }
        // The quiet zone: blank first line, blank sides.
        assert!(lines[0].contains(&" ".repeat(side)));
    }
}
