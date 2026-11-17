#!/usr/bin/env python3
"""Generates tests/fixtures/codec/json/python.jsonl.

Random JSON values serialized by Python's json module, two lines per value:
ensure_ascii=True (every non-ASCII char as \\u escapes, astral chars as
surrogate pairs) and ensure_ascii=False (raw UTF-8), with varying
separators/indent. Deterministic (fixed seed).
"""
import json
import os
import random

rng = random.Random(0x57324A53)
ALPHABET = (
    [chr(c) for c in range(0x20, 0x7F)]
    + ["\n", "\t", "\r", "\b", "\f", "\x00", "\x1f", "\x7f", '"', "\\", "/"]
    + ["é", "ß", "€", "日", "本", "\u2028", "\ufeff", "😀", "𝄞", "\U0010FFFF"]
)


def rand_str():
    return "".join(rng.choice(ALPHABET) for _ in range(rng.randrange(0, 12)))


def rand_num():
    k = rng.randrange(5)
    if k == 0:
        return rng.randrange(-10, 10)
    if k == 1:
        return rng.randrange(-(2**63), 2**63)
    if k == 2:
        return rng.uniform(-1e6, 1e6)
    if k == 3:
        return rng.choice([0.1, 1e-7, 1e21, 5e-324, 1.7976931348623157e308, -0.0, 2.0])
    return rng.random() * 10 ** rng.randrange(-30, 30)


def rand_value(depth=0):
    k = rng.randrange(8 if depth < 4 else 5)
    if k == 0:
        return rng.choice([None, True, False])
    if k in (1, 2):
        return rand_num()
    if k in (3, 4):
        return rand_str()
    if k in (5, 6):
        return {rand_str(): rand_value(depth + 1) for _ in range(rng.randrange(0, 5))}
    return [rand_value(depth + 1) for _ in range(rng.randrange(0, 5))]


def main():
    root = os.path.join(os.path.dirname(__file__), "..", "..")
    out = os.path.join(root, "tests", "fixtures", "codec", "json", "python.jsonl")
    lines = []
    for _ in range(150):
        v = rand_value()
        lines.append(json.dumps(v, ensure_ascii=True))
        lines.append(json.dumps(v, ensure_ascii=False, separators=(",", ":")))
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
