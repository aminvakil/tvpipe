#!/usr/bin/env python3
"""Generates src/media/aacenc/tables.rs from ISO/IEC 13818-7:2004 text.

Input: the plain text of ISO/IEC 13818-7:2004(E) (e.g. extracted from the
PDF with pypdf), given as argv[1]. The standard itself is not checked in.

Transcribed (normative data, not code):
- Annex A, Table A.1 (scalefactor Huffman codebook) and Tables A.2-A.12
  (spectrum Huffman codebooks 1-11): index, length, codeword (hex).
- Clause 8.9, Tables 45-57: swb_offset_long_window / swb_offset_short_window.

Each codebook is checked for complete index coverage, prefix-freeness and
Kraft equality before anything is written; the Rust tests repeat the checks.
"""
import re
import sys

text = open(sys.argv[1]).read()
lines = text.splitlines()

NOISE = re.compile(r"Copyright|Reproduced|Document provided|Questions or|303-397|^--|=====PAGE|ISO/IEC 13818-7|All rights reserved")


def huff_table(num, size):
    # Collect lines from "Table A.<num> –" up to "Table A.<num+1> –".
    start = next(i for i, l in enumerate(lines) if l.startswith(f"Table A.{num} –"))
    end = next(i for i, l in enumerate(lines) if l.startswith(f"Table A.{num + 1} –"))
    entries = {}
    for l in lines[start:end]:
        if NOISE.search(l):
            continue
        toks = l.split()
        if not toks or not all(re.fullmatch(r"[0-9a-f]+", t) for t in toks):
            continue
        if len(toks) % 3 != 0:
            raise SystemExit(f"A.{num}: odd line {l!r}")
        for j in range(0, len(toks), 3):
            idx, ln, cw = int(toks[j]), int(toks[j + 1]), int(toks[j + 2], 16)
            if idx in entries:
                raise SystemExit(f"A.{num}: duplicate index {idx}")
            entries[idx] = (ln, cw)
    if sorted(entries) != list(range(size)):
        missing = set(range(size)) - set(entries)
        raise SystemExit(f"A.{num}: indices incomplete, missing {sorted(missing)[:10]}")
    codes = [entries[i] for i in range(size)]
    check(codes, f"A.{num}")
    return codes


def check(codes, name):
    kraft = sum(2.0 ** -ln for ln, _ in codes)
    if abs(kraft - 1.0) > 1e-12:
        raise SystemExit(f"{name}: Kraft sum {kraft}")
    strs = sorted(format(cw, f"0{ln}b") for ln, cw in codes)
    for ln, cw in codes:
        if cw >> ln:
            raise SystemExit(f"{name}: codeword wider than length")
    for a, b in zip(strs, strs[1:]):
        if b.startswith(a):
            raise SystemExit(f"{name}: {a} is a prefix of {b}")


def swb_table(num, count, total):
    start = next(i for i, l in enumerate(lines) if l.startswith(f"Table {num} –"))
    end = next(i for i, l in enumerate(lines[start + 1:], start + 1) if l.startswith("Table ") or l.startswith("8.10"))
    pairs = {}
    final = None
    for l in lines[start:end]:
        if NOISE.search(l):
            continue
        toks = l.split()
        if not toks or not all(t.isdigit() for t in toks):
            continue
        # Lines are "swb off [swb off]" or a lone end value after a pair.
        if len(toks) == 4:
            pairs[int(toks[0])] = int(toks[1])
            pairs[int(toks[2])] = int(toks[3])
        elif len(toks) == 3:
            pairs[int(toks[0])] = int(toks[1])
            final = int(toks[2])
        elif len(toks) == 2:
            pairs[int(toks[0])] = int(toks[1])
        elif len(toks) == 1:
            final = int(toks[0])
    offs = [pairs[i] for i in range(count)] + [final]
    if offs[0] != 0 or offs[-1] != total or any(b <= a for a, b in zip(offs, offs[1:])):
        raise SystemExit(f"Table {num}: bad offsets {offs}")
    if any(o % 4 for o in offs):
        raise SystemExit(f"Table {num}: offsets not multiples of 4")
    return offs


sizes = {1: 81, 2: 81, 3: 81, 4: 81, 5: 81, 6: 81, 7: 64, 8: 64, 9: 169, 10: 169, 11: 289}
sf = huff_table(1, 121)
spec = {cb: huff_table(cb + 1, n) for cb, n in sizes.items()}

swb = {
    "LONG_48": swb_table(45, 49, 1024),
    "SHORT_48": swb_table(46, 14, 128),
    "LONG_32": swb_table(47, 51, 1024),
    "LONG_8": swb_table(48, 40, 1024),
    "SHORT_8": swb_table(49, 15, 128),
    "LONG_16": swb_table(50, 43, 1024),
    "SHORT_16": swb_table(51, 15, 128),
    "LONG_24": swb_table(52, 47, 1024),
    "SHORT_24": swb_table(53, 15, 128),
    "LONG_64": swb_table(54, 47, 1024),
    "SHORT_64": swb_table(55, 12, 128),
    "LONG_96": swb_table(56, 41, 1024),
    "SHORT_96": swb_table(57, 12, 128),
}

out = []
w = out.append
w("//! Standard tables for the AAC-LC encoder. Generated; do not edit.")
w("//!")
w("//! Transcribed by `tests/vectors/aacenc/gen_tables.py` from ISO/IEC")
w("//! 13818-7:2004(E): Annex A Tables A.1-A.12 (Huffman codebooks) and")
w("//! clause 8.9 Tables 45-57 (scalefactor band offsets).")
w("")
w("/// A Huffman codeword: (length in bits, codeword right-aligned).")
w("pub type Code = (u8, u32);")
w("")


def emit_codes(name, codes, doc):
    w(f"/// {doc}")
    w(f"pub static {name}: [Code; {len(codes)}] = [")
    for i in range(0, len(codes), 6):
        w("    " + " ".join(f"({ln}, 0x{cw:x})," for ln, cw in codes[i:i + 6]))
    w("];")
    w("")


emit_codes("SCALEFACTOR", sf, "Table A.1: scalefactor codebook, index = dpcm + 60.")
for cb, codes in spec.items():
    emit_codes(f"SPECTRUM_{cb}", codes, f"Table A.{cb + 1}: spectrum codebook {cb}.")
w("/// Spectrum codebooks 1-11 by number (index 0 unused).")
w("pub static SPECTRUM: [&[Code]; 12] = [")
w("    &[],")
for cb in spec:
    w(f"    &SPECTRUM_{cb},")
w("];")
w("")
for name, offs in swb.items():
    w(f"/// swb_offset table {name.lower()} (with the end offset).")
    w(f"pub static SWB_{name}: [u16; {len(offs)}] = [")
    for i in range(0, len(offs), 12):
        w("    " + " ".join(f"{o}," for o in offs[i:i + 12]))
    w("];")
    w("")
sys.stdout.write("\n".join(out))
