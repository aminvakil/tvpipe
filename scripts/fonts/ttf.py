"""TrueType fonts for the Rust renderer (tvpipe-canvas reads `glyf` TTFs,
not WOFF2; docs/multiscreen-design.md §5), made from the WOFF2 files the
web pages use: variable fonts pinned to static instances, subset to Latin,
Latin-1 and Latin Extended-A plus common punctuation, hinting dropped,
kerning (GPOS `kern`) kept.

    uvx --from 'fonttools[woff]' python scripts/fonts/ttf.py

Writes games/drawbluff/fonts/*.ttf and the test font of tvpipe-canvas
(crates/tvpipe-canvas/tests/fonts/plex-sans.ttf, ASCII and a few more).
"""

import os
import shutil

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
WEB = os.path.join(ROOT, "games", "drawbluff", "web", "phone", "fonts")
GAME = os.path.join(ROOT, "games", "drawbluff", "fonts")
TESTS = os.path.join(ROOT, "crates", "tvpipe-canvas", "tests", "fonts")

LATIN = (
    list(range(0x20, 0x7F))
    + list(range(0xA0, 0x180))
    + [0x2013, 0x2014, 0x2018, 0x2019, 0x201A, 0x201C, 0x201D, 0x201E,
       0x2022, 0x2026, 0x20AC, 0x2122, 0x2190, 0x2192]
)
TEST = list(range(0x20, 0x7F)) + [0xE9, 0xC4, 0xD6, 0xDC, 0xE4, 0xF6, 0xFC, 0x2026]


def make(src, dst, axes, unicodes):
    font = TTFont(os.path.join(WEB, src))
    font.flavor = None
    if "fvar" in font:
        font = instancer.instantiateVariableFont(font, axes)
    opts = subset.Options()
    opts.layout_features = ["kern"]
    opts.hinting = False
    opts.desubroutinize = True
    opts.name_IDs = ["*"]
    opts.notdef_outline = True
    sub = subset.Subsetter(opts)
    sub.populate(unicodes=unicodes)
    sub.subset(font)
    font.save(dst)
    print(f"{os.path.relpath(dst, ROOT)}: {os.path.getsize(dst)} bytes, {font['maxp'].numGlyphs} glyphs")


os.makedirs(GAME, exist_ok=True)
os.makedirs(TESTS, exist_ok=True)
make("plex-sans.woff2", os.path.join(GAME, "plex-sans-regular.ttf"), {"wght": 400}, LATIN)
make("plex-sans.woff2", os.path.join(GAME, "plex-sans-semibold.ttf"), {"wght": 600}, LATIN)
make("michroma.woff2", os.path.join(GAME, "michroma.ttf"), {}, LATIN)
make("sixtyfour.woff2", os.path.join(GAME, "sixtyfour.ttf"), {"SCAN": 0, "BLED": 0}, LATIN)
shutil.copy(os.path.join(WEB, "OFL.txt"), os.path.join(GAME, "OFL.txt"))
make("plex-sans.woff2", os.path.join(TESTS, "plex-sans.ttf"), {"wght": 400}, TEST)
shutil.copy(os.path.join(WEB, "OFL.txt"), os.path.join(TESTS, "OFL.txt"))


def vectors(path, out):
    """Known answers for tvpipe-canvas's font tests, from fontTools:
    glyph ids, advances, exact outline areas (font units², non-zero) and
    bounds, and GPOS kerning of a few pairs."""
    from fontTools.pens.areaPen import AreaPen
    from fontTools.pens.boundsPen import BoundsPen

    font = TTFont(path)
    cmap = font.getBestCmap()
    glyphs = font.getGlyphSet()
    order = font.getGlyphOrder()
    lines = [
        "# Known answers for tests/fonts/plex-sans.ttf, written by scripts/fonts/ttf.py (fontTools).",
        f"units_per_em = {font['head'].unitsPerEm}",
        f"ascender = {font['hhea'].ascent}",
        f"descender = {font['hhea'].descent}",
        "",
    ]
    for ch in "AHOeg.é&Ä…":
        name = cmap[ord(ch)]
        area = AreaPen(glyphs)
        glyphs[name].draw(area)
        bounds = BoundsPen(glyphs)
        glyphs[name].draw(bounds)
        b = bounds.bounds
        composite = font["glyf"][name].isComposite()
        lines += [
            f"char = U+{ord(ch):04X}",
            f"glyph = {order.index(name)}",
            f"advance = {font['hmtx'][name][0]}",
            f"area = {abs(area.value):.1f}",
            f"bounds = {b[0]} {b[1]} {b[2]} {b[3]}",
            f"composite = {str(composite).lower()}",
            "",
        ]
    # Pair kerning from GPOS (lookup type 2), the way a shaper would see
    # the first pair adjustment of the `kern` feature.
    from fontTools.ttLib.tables import otTables

    def kern(left, right):
        gpos = font["GPOS"].table
        idx = set()
        for fr in gpos.FeatureList.FeatureRecord:
            if fr.FeatureTag == "kern":
                idx.update(fr.Feature.LookupListIndex)
        for i in sorted(idx):
            lookup = gpos.LookupList.Lookup[i]
            for st in lookup.SubTable:
                if lookup.LookupType == 9:
                    st = st.ExtSubTable
                if st.LookupType != 2:
                    continue
                cov = st.Coverage.glyphs
                if left not in cov:
                    continue
                if st.Format == 1:
                    ps = st.PairSet[cov.index(left)]
                    for r in ps.PairValueRecord:
                        if r.SecondGlyph == right:
                            return getattr(r.Value1, "XAdvance", 0) or 0
                else:
                    c1 = st.ClassDef1.classDefs.get(left, 0)
                    c2 = st.ClassDef2.classDefs.get(right, 0)
                    v = st.Class1Record[c1].Class2Record[c2].Value1
                    v = getattr(v, "XAdvance", 0) or 0
                    if v:
                        return v
        return 0

    for pair in ["AV", "To", "Ty", "LT", "VA", "ab", "Pa", "F."]:
        l, r = cmap[ord(pair[0])], cmap[ord(pair[1])]
        lines += [f"pair = {pair}", f"kern = {kern(l, r)}", ""]
    with open(out, "w") as f:
        f.write("\n".join(lines))
    print(f"{os.path.relpath(out, ROOT)}")


vectors(os.path.join(TESTS, "plex-sans.ttf"), os.path.join(TESTS, "plex-sans.txt"))
