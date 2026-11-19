//! TrueType fonts (`glyf` outlines): the character map (formats 4 and
//! 12), metrics, simple and composite glyphs, and kerning from `kern`
//! (format 0) or GPOS pair adjustments. No hinting, no shaping; CFF
//! (`.otf`) fonts are refused. Every read is checked: broken font data is
//! an error, never a panic.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::path::Path;

/// A font that could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontError(String);

impl FontError {
    fn new(msg: impl Into<String>) -> FontError {
        FontError(msg.into())
    }
}

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "font: {}", self.0)
    }
}

impl std::error::Error for FontError {}

type Result<T> = std::result::Result<T, FontError>;

fn truncated() -> FontError {
    FontError::new("truncated data")
}

/// Checked big-endian reads.
#[derive(Clone, Copy)]
struct Data<'a>(&'a [u8]);

impl<'a> Data<'a> {
    fn u8(&self, at: usize) -> Result<u8> {
        self.0.get(at).copied().ok_or_else(truncated)
    }
    fn u16(&self, at: usize) -> Result<u16> {
        match self.0.get(at..at.saturating_add(2)) {
            Some(b) => Ok(u16::from_be_bytes([b[0], b[1]])),
            None => Err(truncated()),
        }
    }
    fn i16(&self, at: usize) -> Result<i16> {
        self.u16(at).map(|v| v as i16)
    }
    fn u32(&self, at: usize) -> Result<u32> {
        match self.0.get(at..at.saturating_add(4)) {
            Some(b) => Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]])),
            None => Err(truncated()),
        }
    }
    fn sub(&self, at: usize, len: usize) -> Result<Data<'a>> {
        self.0
            .get(at..at.saturating_add(len))
            .map(Data)
            .ok_or_else(truncated)
    }
}

#[derive(Debug, Clone, Copy)]
enum Cmap {
    /// Segment mapping (BMP), at this offset in the font.
    Format4(usize),
    /// Segmented coverage (all planes).
    Format12(usize),
}

#[derive(Debug, Clone, Copy, Default)]
struct Range {
    at: usize,
    len: usize,
}

struct Inner {
    id: u64,
    data: Cow<'static, [u8]>,
    units_per_em: u16,
    ascender: i16,
    descender: i16,
    line_gap: i16,
    num_glyphs: u16,
    num_hmetrics: u16,
    loca_long: bool,
    loca: Range,
    glyf: Range,
    hmtx: Range,
    cmap: Cmap,
    /// `kern` format 0 pairs: offset of the first pair, count.
    kern: Option<(usize, usize)>,
    /// GPOS pair-adjustment subtables of the `kern` feature (offsets).
    gpos_pairs: Vec<usize>,
}

/// A TrueType font; cheap to clone.
#[derive(Clone)]
pub struct Font(Arc<Inner>);

impl std::fmt::Debug for Font {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Font")
            .field("glyphs", &self.0.num_glyphs)
            .field("units_per_em", &self.0.units_per_em)
            .finish_non_exhaustive()
    }
}

/// A point of a glyph outline (font units, y up) and whether it is on
/// the curve.
type Point = (f32, f32, bool);

/// How deep composite glyphs may nest.
const MAX_DEPTH: u32 = 8;
/// Most points in one glyph (all components together).
const MAX_POINTS: usize = 1 << 16;

impl Font {
    /// Reads a TrueType font (`.ttf`, `glyf` outlines).
    pub fn parse(data: impl Into<Cow<'static, [u8]>>) -> Result<Font> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let data = data.into();
        let d = Data(&data);
        let version = d.u32(0)?;
        match version {
            0x0001_0000 | 0x7472_7565 => {}
            0x4f54_544f => return Err(FontError::new("CFF outlines (.otf) are not supported")),
            0x7474_6366 => return Err(FontError::new("font collections are not supported")),
            0x774f_4646 | 0x774f_4632 => {
                return Err(FontError::new("WOFF fonts are not supported; use the .ttf"))
            }
            _ => return Err(FontError::new("not a TrueType font")),
        }
        let n = d.u16(4)? as usize;
        let mut tables: Vec<([u8; 4], Range)> = Vec::with_capacity(n);
        for i in 0..n {
            let rec = 12 + i * 16;
            let tag = d.sub(rec, 4)?.0;
            let at = d.u32(rec + 8)? as usize;
            let len = d.u32(rec + 12)? as usize;
            d.sub(at, len)?;
            tables.push(([tag[0], tag[1], tag[2], tag[3]], Range { at, len }));
        }
        let find = |tag: &[u8; 4]| tables.iter().find(|t| &t.0 == tag).map(|t| t.1);
        let need = |tag: &[u8; 4]| {
            find(tag)
                .ok_or_else(|| FontError::new(format!("no {} table", String::from_utf8_lossy(tag))))
        };
        let head = need(b"head")?;
        let hhea = need(b"hhea")?;
        let maxp = need(b"maxp")?;
        let hmtx = need(b"hmtx")?;
        let cmap = need(b"cmap")?;
        let loca = need(b"loca")?;
        let glyf = need(b"glyf")?;
        let units_per_em = d.u16(head.at + 18)?;
        if !(16..=16384).contains(&units_per_em) {
            return Err(FontError::new("bad unitsPerEm"));
        }
        let loca_long = d.i16(head.at + 50)? != 0;
        let num_glyphs = d.u16(maxp.at + 4)?;
        let num_hmetrics = d.u16(hhea.at + 34)?;
        if num_hmetrics == 0 || hmtx.len < num_hmetrics as usize * 4 {
            return Err(FontError::new("bad hmtx"));
        }
        let cmap = find_cmap(d, cmap)?;
        let kern = find(b"kern").and_then(|k| find_kern(d, k).ok().flatten());
        let gpos_pairs = find(b"GPOS")
            .map(|g| find_gpos_pairs(d, g).unwrap_or_default())
            .unwrap_or_default();
        Ok(Font(Arc::new(Inner {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            units_per_em,
            ascender: d.i16(hhea.at + 4)?,
            descender: d.i16(hhea.at + 6)?,
            line_gap: d.i16(hhea.at + 8)?,
            num_glyphs,
            num_hmetrics,
            loca_long,
            loca,
            glyf,
            hmtx,
            cmap,
            kern,
            gpos_pairs,
            data,
        })))
    }

    fn d(&self) -> Data<'_> {
        Data(&self.0.data)
    }

    /// A number unique to this font (for caches).
    pub(crate) fn id(&self) -> u64 {
        self.0.id
    }

    /// Font units per em.
    pub fn units_per_em(&self) -> u16 {
        self.0.units_per_em
    }

    /// The ascender (font units, above the baseline).
    pub fn ascender(&self) -> i16 {
        self.0.ascender
    }

    /// The descender (font units, negative: below the baseline).
    pub fn descender(&self) -> i16 {
        self.0.descender
    }

    /// The extra gap between lines (font units).
    pub fn line_gap(&self) -> i16 {
        self.0.line_gap
    }

    /// Glyphs in the font.
    pub fn glyph_count(&self) -> u16 {
        self.0.num_glyphs
    }

    /// The glyph of a character (`None` when the font lacks it).
    pub fn glyph_index(&self, c: char) -> Option<u16> {
        let g = match self.0.cmap {
            Cmap::Format4(at) => cmap4(self.d(), at, c as u32).ok()?,
            Cmap::Format12(at) => cmap12(self.d(), at, c as u32).ok()?,
        };
        (g != 0 && g < self.0.num_glyphs).then_some(g)
    }

    /// A glyph's advance width (font units).
    pub fn advance(&self, glyph: u16) -> u16 {
        let i = glyph.min(self.0.num_hmetrics - 1) as usize;
        self.d().u16(self.0.hmtx.at + i * 4).unwrap_or(0)
    }

    /// The kerning between two glyphs (font units, usually negative).
    pub fn kerning(&self, left: u16, right: u16) -> i16 {
        for &st in &self.0.gpos_pairs {
            if let Ok(Some(v)) = gpos_pair(self.d(), st, left, right) {
                return v;
            }
        }
        if let Some((at, n)) = self.0.kern {
            return kern0(self.d(), at, n, left, right).unwrap_or(0);
        }
        0
    }

    /// A glyph's outline as a path in font units, y up (`None`: an empty
    /// glyph such as the space).
    pub fn outline(&self, glyph: u16) -> Result<Option<Path>> {
        let mut contours = Vec::new();
        let mut total = 0;
        self.contours(glyph, 0, &mut contours, &mut total)?;
        if contours.is_empty() {
            return Ok(None);
        }
        let mut path = Path::new();
        for c in &contours {
            contour_path(&mut path, c);
        }
        Ok(Some(path))
    }

    fn glyph_data(&self, glyph: u16) -> Result<Option<Data<'_>>> {
        if glyph >= self.0.num_glyphs {
            return Err(FontError::new("glyph out of range"));
        }
        let d = self.d();
        let loca = self.0.loca.at;
        let i = glyph as usize;
        let (start, end) = if self.0.loca_long {
            (
                d.u32(loca + i * 4)? as usize,
                d.u32(loca + i * 4 + 4)? as usize,
            )
        } else {
            (
                d.u16(loca + i * 2)? as usize * 2,
                d.u16(loca + i * 2 + 2)? as usize * 2,
            )
        };
        if end <= start {
            return Ok(None);
        }
        if end > self.0.glyf.len {
            return Err(truncated());
        }
        d.sub(self.0.glyf.at + start, end - start).map(Some)
    }

    fn contours(
        &self,
        glyph: u16,
        depth: u32,
        out: &mut Vec<Vec<Point>>,
        total: &mut usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(FontError::new("composite glyphs nest too deep"));
        }
        let Some(g) = self.glyph_data(glyph)? else {
            return Ok(());
        };
        let n = g.i16(0)?;
        if n >= 0 {
            return simple_glyph(g, n as usize, out, total);
        }
        // A composite glyph.
        let mut at = 10;
        loop {
            let flags = g.u16(at)?;
            let index = g.u16(at + 2)?;
            at += 4;
            let (a1, a2) = if flags & 0x0001 != 0 {
                let v = (g.i16(at)? as f32, g.i16(at + 2)? as f32);
                at += 4;
                v
            } else {
                let v = (g.u8(at)? as i8 as f32, g.u8(at + 1)? as i8 as f32);
                at += 2;
                v
            };
            // Point-matched placement (ARGS_ARE_XY_VALUES unset) is rare in
            // fonts without hinting; it is drawn unmoved.
            let (dx, dy) = if flags & 0x0002 != 0 {
                (a1, a2)
            } else {
                (0.0, 0.0)
            };
            let f2 = |v: i16| v as f32 / 16384.0;
            let (mut a, mut b, mut c, mut d) = (1.0, 0.0, 0.0, 1.0);
            if flags & 0x0008 != 0 {
                a = f2(g.i16(at)?);
                d = a;
                at += 2;
            } else if flags & 0x0040 != 0 {
                a = f2(g.i16(at)?);
                d = f2(g.i16(at + 2)?);
                at += 4;
            } else if flags & 0x0080 != 0 {
                a = f2(g.i16(at)?);
                b = f2(g.i16(at + 2)?);
                c = f2(g.i16(at + 4)?);
                d = f2(g.i16(at + 6)?);
                at += 8;
            }
            let first = out.len();
            self.contours(index, depth + 1, out, total)?;
            for contour in &mut out[first..] {
                for p in contour.iter_mut() {
                    let (x, y) = (p.0, p.1);
                    p.0 = a * x + c * y + dx;
                    p.1 = b * x + d * y + dy;
                }
            }
            if flags & 0x0020 == 0 {
                return Ok(());
            }
        }
    }
}

fn simple_glyph(
    g: Data<'_>,
    contours: usize,
    out: &mut Vec<Vec<Point>>,
    total: &mut usize,
) -> Result<()> {
    if contours == 0 {
        return Ok(());
    }
    let mut ends = Vec::with_capacity(contours);
    let mut last: Option<usize> = None;
    for i in 0..contours {
        let e = g.u16(10 + i * 2)? as usize;
        if last.is_some_and(|l| e < l) {
            return Err(FontError::new("contour ends out of order"));
        }
        last = Some(e);
        ends.push(e);
    }
    let n = last.map_or(0, |l| l + 1);
    *total += n;
    if *total > MAX_POINTS {
        return Err(FontError::new("too many points"));
    }
    let ins = 10 + contours * 2;
    let mut at = ins + 2 + g.u16(ins)? as usize;
    let mut flags = Vec::with_capacity(n);
    while flags.len() < n {
        let f = g.u8(at)?;
        at += 1;
        flags.push(f);
        if f & 0x08 != 0 {
            let repeat = g.u8(at)?;
            at += 1;
            for _ in 0..repeat {
                if flags.len() < n {
                    flags.push(f);
                }
            }
        }
    }
    let coords = |short: u8, same: u8, at: &mut usize| -> Result<Vec<f32>> {
        let mut v = 0i32;
        let mut out = Vec::with_capacity(n);
        for &f in &flags {
            if f & short != 0 {
                let d = g.u8(*at)? as i32;
                *at += 1;
                v += if f & same != 0 { d } else { -d };
            } else if f & same == 0 {
                v += g.i16(*at)? as i32;
                *at += 2;
            }
            out.push(v as f32);
        }
        Ok(out)
    };
    let xs = coords(0x02, 0x10, &mut at)?;
    let ys = coords(0x04, 0x20, &mut at)?;
    let mut start = 0;
    for &e in &ends {
        let contour: Vec<Point> = (start..=e)
            .filter_map(|i| Some((*xs.get(i)?, *ys.get(i)?, flags.get(i)? & 1 != 0)))
            .collect();
        if !contour.is_empty() {
            out.push(contour);
        }
        start = e + 1;
    }
    Ok(())
}

/// A TrueType contour (quadratic, implied on-curve midpoints) as a path.
fn contour_path(path: &mut Path, pts: &[Point]) {
    let n = pts.len();
    if n == 0 {
        return;
    }
    // Start on the curve: the first on-curve point (then the points after
    // it), else the midpoint of the last and the first (then all of them).
    let (start, rest): ((f32, f32), Vec<Point>) = match pts.iter().position(|p| p.2) {
        Some(i) => (
            (pts[i].0, pts[i].1),
            (1..n).map(|k| pts[(i + k) % n]).collect(),
        ),
        None => {
            let (a, b) = (pts[n - 1], pts[0]);
            (((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0), pts.to_vec())
        }
    };
    path.move_to(start.0, start.1);
    let mut ctrl: Option<(f32, f32)> = None;
    for p in rest {
        if p.2 {
            match ctrl.take() {
                Some(c) => path.quad_to(c.0, c.1, p.0, p.1),
                None => path.line_to(p.0, p.1),
            };
        } else {
            if let Some(c) = ctrl {
                path.quad_to(c.0, c.1, (c.0 + p.0) / 2.0, (c.1 + p.1) / 2.0);
            }
            ctrl = Some((p.0, p.1));
        }
    }
    match ctrl {
        Some(c) => path.quad_to(c.0, c.1, start.0, start.1),
        None => path.line_to(start.0, start.1),
    };
    path.close();
}

fn find_cmap(d: Data<'_>, t: Range) -> Result<Cmap> {
    let n = d.u16(t.at + 2)? as usize;
    let mut best: Option<(u8, Cmap)> = None;
    for i in 0..n {
        let rec = t.at + 4 + i * 8;
        let platform = d.u16(rec)?;
        let encoding = d.u16(rec + 2)?;
        let at = t.at + d.u32(rec + 4)? as usize;
        let format = d.u16(at)?;
        let unicode = platform == 0 || (platform == 3 && (encoding == 1 || encoding == 10));
        if !unicode {
            continue;
        }
        let found = match format {
            12 => (2, Cmap::Format12(at)),
            4 => (1, Cmap::Format4(at)),
            _ => continue,
        };
        if best.is_none_or(|b| b.0 < found.0) {
            best = Some(found);
        }
    }
    best.map(|b| b.1)
        .ok_or_else(|| FontError::new("no Unicode character map (format 4 or 12)"))
}

fn cmap4(d: Data<'_>, at: usize, c: u32) -> Result<u16> {
    if c > 0xffff {
        return Ok(0);
    }
    let c = c as u16;
    let segs = d.u16(at + 6)? as usize / 2;
    let ends = at + 14;
    let starts = ends + segs * 2 + 2;
    let deltas = starts + segs * 2;
    let offsets = deltas + segs * 2;
    // The first segment whose end is ≥ c.
    let (mut lo, mut hi) = (0, segs);
    while lo < hi {
        let m = (lo + hi) / 2;
        if d.u16(ends + m * 2)? < c {
            lo = m + 1;
        } else {
            hi = m;
        }
    }
    if lo >= segs {
        return Ok(0);
    }
    let start = d.u16(starts + lo * 2)?;
    if c < start {
        return Ok(0);
    }
    let delta = d.u16(deltas + lo * 2)?;
    let ro = d.u16(offsets + lo * 2)? as usize;
    if ro == 0 {
        return Ok(c.wrapping_add(delta));
    }
    let g = d.u16(offsets + lo * 2 + ro + (c - start) as usize * 2)?;
    Ok(if g == 0 { 0 } else { g.wrapping_add(delta) })
}

fn cmap12(d: Data<'_>, at: usize, c: u32) -> Result<u16> {
    let n = d.u32(at + 12)? as usize;
    let groups = at + 16;
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let m = (lo + hi) / 2;
        let g = groups + m * 12;
        let (start, end) = (d.u32(g)?, d.u32(g + 4)?);
        if c < start {
            hi = m;
        } else if c > end {
            lo = m + 1;
        } else {
            let glyph = d.u32(g + 8)?.saturating_add(c - start);
            return Ok(u16::try_from(glyph).unwrap_or(0));
        }
    }
    Ok(0)
}

fn find_kern(d: Data<'_>, t: Range) -> Result<Option<(usize, usize)>> {
    if d.u16(t.at)? != 0 {
        return Ok(None);
    }
    let n = d.u16(t.at + 2)?;
    let mut at = t.at + 4;
    for _ in 0..n {
        let len = d.u16(at + 2)? as usize;
        let coverage = d.u16(at + 4)?;
        // Format 0, horizontal, not cross-stream or minimum values.
        if coverage >> 8 == 0 && coverage & 0x07 == 0x01 {
            let pairs = d.u16(at + 6)? as usize;
            d.sub(at + 14, pairs * 6)?;
            return Ok(Some((at + 14, pairs)));
        }
        at += len.max(6);
    }
    Ok(None)
}

fn kern0(d: Data<'_>, at: usize, n: usize, left: u16, right: u16) -> Result<i16> {
    let key = (left as u32) << 16 | right as u32;
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let m = (lo + hi) / 2;
        let k = d.u32(at + m * 6)?;
        match k.cmp(&key) {
            std::cmp::Ordering::Less => lo = m + 1,
            std::cmp::Ordering::Greater => hi = m,
            std::cmp::Ordering::Equal => return d.i16(at + m * 6 + 4),
        }
    }
    Ok(0)
}

/// The pair-adjustment subtables (lookup type 2, or 9 wrapping 2) of
/// the `kern` feature, any script.
fn find_gpos_pairs(d: Data<'_>, t: Range) -> Result<Vec<usize>> {
    let g = t.at;
    let features = g + d.u16(g + 6)? as usize;
    let lookups = g + d.u16(g + 8)? as usize;
    let mut indices: Vec<u16> = Vec::new();
    let nf = d.u16(features)? as usize;
    for i in 0..nf {
        let rec = features + 2 + i * 6;
        if d.sub(rec, 4)?.0 != b"kern" {
            continue;
        }
        let f = features + d.u16(rec + 4)? as usize;
        let n = d.u16(f + 2)? as usize;
        for j in 0..n {
            let l = d.u16(f + 4 + j * 2)?;
            if !indices.contains(&l) {
                indices.push(l);
            }
        }
    }
    indices.sort_unstable();
    let nl = d.u16(lookups)?;
    let mut out = Vec::new();
    for l in indices {
        if l >= nl {
            continue;
        }
        let lookup = lookups + d.u16(lookups + 2 + l as usize * 2)? as usize;
        let kind = d.u16(lookup)?;
        let n = d.u16(lookup + 4)? as usize;
        for s in 0..n {
            let mut st = lookup + d.u16(lookup + 6 + s * 2)? as usize;
            let mut kind = kind;
            if kind == 9 {
                kind = d.u16(st + 2)?;
                st += d.u32(st + 4)? as usize;
            }
            if kind == 2 && out.len() < 64 {
                out.push(st);
            }
        }
    }
    Ok(out)
}

fn coverage(d: Data<'_>, at: usize, glyph: u16) -> Result<Option<usize>> {
    match d.u16(at)? {
        1 => {
            let n = d.u16(at + 2)? as usize;
            let (mut lo, mut hi) = (0, n);
            while lo < hi {
                let m = (lo + hi) / 2;
                let g = d.u16(at + 4 + m * 2)?;
                match g.cmp(&glyph) {
                    std::cmp::Ordering::Less => lo = m + 1,
                    std::cmp::Ordering::Greater => hi = m,
                    std::cmp::Ordering::Equal => return Ok(Some(m)),
                }
            }
            Ok(None)
        }
        2 => {
            let n = d.u16(at + 2)? as usize;
            let (mut lo, mut hi) = (0, n);
            while lo < hi {
                let m = (lo + hi) / 2;
                let r = at + 4 + m * 6;
                let (s, e) = (d.u16(r)?, d.u16(r + 2)?);
                if glyph < s {
                    hi = m;
                } else if glyph > e {
                    lo = m + 1;
                } else {
                    return Ok(Some(d.u16(r + 4)? as usize + (glyph - s) as usize));
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn class_of(d: Data<'_>, at: usize, glyph: u16) -> Result<u16> {
    match d.u16(at)? {
        1 => {
            let start = d.u16(at + 2)?;
            let n = d.u16(at + 4)?;
            if glyph < start || glyph - start >= n {
                return Ok(0);
            }
            d.u16(at + 6 + (glyph - start) as usize * 2)
        }
        2 => {
            let n = d.u16(at + 2)? as usize;
            let (mut lo, mut hi) = (0, n);
            while lo < hi {
                let m = (lo + hi) / 2;
                let r = at + 4 + m * 6;
                let (s, e) = (d.u16(r)?, d.u16(r + 2)?);
                if glyph < s {
                    hi = m;
                } else if glyph > e {
                    lo = m + 1;
                } else {
                    return d.u16(r + 4);
                }
            }
            Ok(0)
        }
        _ => Ok(0),
    }
}

/// Bytes of a value record, and where its XAdvance is (if it has one).
fn value_record(format: u16) -> (usize, Option<usize>) {
    let size = (format & 0xff).count_ones() as usize * 2;
    let x_advance = (format & 0x4 != 0).then(|| (format & 0x3).count_ones() as usize * 2);
    (size, x_advance)
}

fn gpos_pair(d: Data<'_>, st: usize, left: u16, right: u16) -> Result<Option<i16>> {
    let format = d.u16(st)?;
    let cov = st + d.u16(st + 2)? as usize;
    let Some(index) = coverage(d, cov, left)? else {
        return Ok(None);
    };
    let (vf1, vf2) = (d.u16(st + 4)?, d.u16(st + 6)?);
    let (s1, xa) = value_record(vf1);
    let (s2, _) = value_record(vf2);
    let Some(xa) = xa else {
        return Ok(None);
    };
    match format {
        1 => {
            let sets = d.u16(st + 8)? as usize;
            if index >= sets {
                return Ok(None);
            }
            let set = st + d.u16(st + 10 + index * 2)? as usize;
            let n = d.u16(set)? as usize;
            let rec = 2 + s1 + s2;
            let (mut lo, mut hi) = (0, n);
            while lo < hi {
                let m = (lo + hi) / 2;
                let r = set + 2 + m * rec;
                let g = d.u16(r)?;
                match g.cmp(&right) {
                    std::cmp::Ordering::Less => lo = m + 1,
                    std::cmp::Ordering::Greater => hi = m,
                    std::cmp::Ordering::Equal => return d.i16(r + 2 + xa).map(Some),
                }
            }
            Ok(None)
        }
        2 => {
            let cd1 = st + d.u16(st + 8)? as usize;
            let cd2 = st + d.u16(st + 10)? as usize;
            let n1 = d.u16(st + 12)? as usize;
            let n2 = d.u16(st + 14)? as usize;
            let c1 = class_of(d, cd1, left)? as usize;
            let c2 = class_of(d, cd2, right)? as usize;
            if c1 >= n1 || c2 >= n2 {
                return Ok(None);
            }
            let r = st + 16 + (c1 * n2 + c2) * (s1 + s2);
            let v = d.i16(r + xa)?;
            Ok((v != 0).then_some(v))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_what_it_cannot_read() {
        for (data, msg) in [
            (&b""[..], "truncated"),
            (b"OTTO\0\0\0\0", "CFF"),
            (b"wOF2\0\0\0\0", "WOFF"),
            (b"\0\x01\0\0\0\x01", "truncated"),
            (b"\0\x01\0\0\0\0\0\0\0\0\0\0", "no head"),
            (b"abcd", "not a TrueType"),
        ] {
            let e = Font::parse(data.to_vec()).unwrap_err();
            assert!(e.to_string().contains(msg), "{e} (want {msg})");
        }
    }

    #[test]
    fn value_records() {
        assert_eq!(value_record(0x4), (2, Some(0)));
        assert_eq!(value_record(0x5), (4, Some(2)));
        assert_eq!(value_record(0x0f), (8, Some(4)));
        assert_eq!(value_record(0x1), (2, None));
    }

    #[test]
    fn contours_with_implied_points() {
        // All off-curve: a rounded shape through the midpoints.
        let mut p = Path::new();
        contour_path(
            &mut p,
            &[
                (0.0, 0.0, false),
                (10.0, 0.0, false),
                (10.0, 10.0, false),
                (0.0, 10.0, false),
            ],
        );
        let b = p.bounds().unwrap();
        assert_eq!(b, (0.0, 0.0, 10.0, 10.0));
        // On-curve square.
        let mut p = Path::new();
        contour_path(
            &mut p,
            &[
                (0.0, 0.0, true),
                (10.0, 0.0, true),
                (10.0, 10.0, true),
                (0.0, 10.0, true),
            ],
        );
        let f = p.flatten();
        assert_eq!(f[0].points.len(), 5);
    }
}
