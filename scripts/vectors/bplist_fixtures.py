#!/usr/bin/env python3
"""Generates the bplist00 fixtures and tests/vectors/codec/bplist.txt.

Three kinds of vectors:

- origin = plistlib: written by Python's plistlib (FMT_BINARY,
  sort_keys=False). Our writer must reproduce these bytes exactly.
- origin = plutil: XML written here, converted with
  `plutil -convert binary1` (macOS). Our reader must decode them; plistlib
  decodes them too and provides the expected value.
- error: hand-assembled hostile files that must be rejected.

Expected values use a tagged JSON form so every plist type survives:
null/true/false, "string", [array], {"int": "decimal"}, {"real": "<f64 bits
hex>"}, {"date": "<f64 bits hex>"} (seconds since 2001-01-01),
{"data": "hex"}, {"dict": [["key", value], ...]} (file order).

Run from anywhere on macOS: python3 scripts/vectors/bplist_fixtures.py
"""
import datetime
import json
import os
import plistlib
import struct
import subprocess
import tempfile

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
FIXTURES = os.path.join(ROOT, "tests", "fixtures", "codec", "bplist")
VECTORS = os.path.join(ROOT, "tests", "vectors", "codec", "bplist.txt")
EPOCH = datetime.datetime(2001, 1, 1)
INLINE_LIMIT = 2000

blocks = []


def bits(f):
    return struct.pack(">d", f).hex()


def tagged(v):
    if v is None or isinstance(v, bool):
        return v
    if isinstance(v, int):
        return {"int": str(v)}
    if isinstance(v, float):
        return {"real": bits(v)}
    if isinstance(v, datetime.datetime):
        return {"date": bits((v - EPOCH).total_seconds())}
    if isinstance(v, (bytes, bytearray)):
        return {"data": bytes(v).hex()}
    if isinstance(v, str):
        return v
    if isinstance(v, (list, tuple)):
        return [tagged(x) for x in v]
    if isinstance(v, dict):
        return {"dict": [[k, tagged(x)] for k, x in v.items()]}
    raise TypeError(type(v))


def hexwrap(key, data):
    h = data.hex()
    s = f"{key} = {h[:64]}\n"
    for i in range(64, len(h), 64):
        s += "  " + h[i : i + 64] + "\n"
    return s


def add(name, origin, data, value):
    fname = name.replace(" ", "-") + ".bplist"
    with open(os.path.join(FIXTURES, fname), "wb") as f:
        f.write(data)
    expected = json.dumps(tagged(value), ensure_ascii=False, separators=(",", ":"))
    s = f"name = {name}\norigin = {origin}\nfile = {fname}\n"
    if len(expected) > INLINE_LIMIT:
        ename = name.replace(" ", "-") + ".expected.json"
        with open(os.path.join(FIXTURES, ename), "w", encoding="utf-8") as f:
            f.write(expected)
        s += f"expected_file = {ename}\n"
    else:
        s += f"expected = {expected}\n"
    blocks.append(s)


def from_plistlib(name, value):
    data = plistlib.dumps(value, fmt=plistlib.FMT_BINARY, sort_keys=False)
    assert plistlib.loads(data) == value
    add(name, "plistlib", data, value)


def from_plutil(name, xml_body):
    xml = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" '
        '"http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n'
        '<plist version="1.0">\n' + xml_body + "\n</plist>\n"
    )
    with tempfile.TemporaryDirectory() as d:
        src = os.path.join(d, "in.plist")
        dst = os.path.join(d, "out.bplist")
        with open(src, "w", encoding="utf-8") as f:
            f.write(xml)
        subprocess.run(["plutil", "-convert", "binary1", "-o", dst, src], check=True)
        with open(dst, "rb") as f:
            data = f.read()
    add(name, "plutil", data, plistlib.loads(data))


def assemble(objects, top=0, offset_size=1, ref_size=1, num=None, table=None, offsets=None):
    """A raw bplist00 from pre-encoded objects."""
    out = b"bplist00"
    offs = []
    for o in objects:
        offs.append(len(out))
        out += o
    table_offset = len(out) if table is None else table
    for o in offsets if offsets is not None else offs:
        out += o.to_bytes(max(offset_size, 1), "big")
    num = len(objects) if num is None else num
    out += struct.pack(">6xBBQQQ", offset_size, ref_size, num, top, table_offset)
    return out


def error(name, data, msg):
    blocks.append(f"name = {name}\n" + hexwrap("encoded", data) + f"error = {msg}\n")


def main():
    os.makedirs(FIXTURES, exist_ok=True)
    for f in os.listdir(FIXTURES):
        os.remove(os.path.join(FIXTURES, f))

    # --- plistlib (byte-exact writer oracle) ---
    from_plistlib("pl true", True)
    from_plistlib("pl empty dict", {})
    from_plistlib("pl ints", [0, 1, 255, 256, 65535, 65536, 2**32 - 1, 2**32, 2**63 - 1,
                              -1, -(2**63), 2**63, 2**64 - 1])
    from_plistlib("pl reals", [0.0, 0.5, -1.25, 1e300, 5e-324, float("inf"), 3.0])
    from_plistlib("pl strings", ["", "a", "fourteen chars", "fifteen chars!!", "grüße",
                                 "日本語", "😀", "x" * 300])
    from_plistlib("pl data", [b"", b"\x00", bytes(range(14)), bytes(range(15)), bytes(300)])
    from_plistlib("pl date", [EPOCH, datetime.datetime(2024, 5, 17, 12, 30, 45),
                              datetime.datetime(1999, 12, 31, 23, 59, 59)])
    from_plistlib("pl shared scalars", {"a": "a", "b": ["a", "b", 1, 1, True, 1.0],
                                        "c": {"a": 1}, "d": {"a": 1}})
    from_plistlib("pl nested", {"outer": [[[]], {"inner": [{}, [1, [2, [3]]]]}],
                                "z": None if False else "end"})
    from_plistlib("pl info like", {
        "deviceID": "AA:BB:CC:DD:EE:FF",
        "features": 0x1C340405F4A00,
        "model": "AppleTV6,2",
        "name": "Living Room",
        "pi": "2e388006-13ba-4041-9a67-25dd4a43d536",
        "pk": bytes.fromhex("f3769a660475d27b4f6040381d784645e13e21c53e6d2da6a8c3d757086fc336"),
        "statusFlags": 4,
        "displays": [{
            "widthPixels": 3840, "heightPixels": 2160, "refreshRate": 60.0,
            "uuid": "e0ff8a27-6738-3d56-8a16-cc53aacee925", "features": 30,
        }],
        "audioFormats": [{"type": 100, "audioInputFormats": 67108860,
                          "audioOutputFormats": 67108860}],
    })
    from_plistlib("pl 300 objects", list(range(300)))
    from_plistlib("pl wide offsets", ["x" * 66000, 1, b"after", "end"])

    # --- plutil (CoreFoundation writer) ---
    from_plutil("pu scalars", """<dict>
  <key>string</key><string>hello</string>
  <key>int</key><integer>42</integer>
  <key>negative</key><integer>-7</integer>
  <key>big</key><integer>9223372036854775807</integer>
  <key>real</key><real>0.5</real>
  <key>true</key><true/>
  <key>false</key><false/>
  <key>date</key><date>2024-05-17T12:30:45Z</date>
  <key>data</key><data>AAECAwQFBgcICQoLDA0ODxAREhM=</data>
</dict>""")
    from_plutil("pu unicode", """<array>
  <string>grüße</string><string>日本語のテキスト</string><string>😀👍</string>
  <string></string><string>fifteen chars!!</string>
</array>""")
    from_plutil("pu nested", """<dict>
  <key>a</key><array><array/><dict/><array><integer>1</integer><array><integer>2</integer></array></array></array>
  <key>b</key><dict><key>c</key><dict><key>d</key><string>deep</string></dict></dict>
</dict>""")
    from_plutil("pu numbers", """<array>
  <integer>0</integer><integer>255</integer><integer>256</integer><integer>65536</integer>
  <integer>4294967296</integer><integer>-1</integer><integer>-9223372036854775808</integer>
  <integer>18446744073709551615</integer>
  <real>1e300</real><real>-2.5</real><real>3</real><real>0.1</real>
</array>""")
    items = "".join(f"<string>item{i}</string>" for i in range(300))
    from_plutil("pu 300 strings", f"<array>{items}</array>")
    from_plutil("pu big data", "<data>" + __import__("base64").b64encode(bytes(range(256)) * 4).decode() + "</data>")
    from_plutil("pu info like", """<dict>
  <key>deviceID</key><string>AA:BB:CC:DD:EE:FF</string>
  <key>features</key><integer>496155769145856</integer>
  <key>model</key><string>AppleTV6,2</string>
  <key>displays</key><array><dict>
    <key>widthPixels</key><integer>1920</integer>
    <key>heightPixels</key><integer>1080</integer>
    <key>refreshRate</key><real>60</real>
  </dict></array>
  <key>pk</key><data>83dpZgR10ntPYEA4HXhGReE+IcU+bS2mqMPXVwhvwzY=</data>
</dict>""")

    # --- hostile / malformed ---
    true = bytes([0x09])
    error("too short", b"bplist00", "not a bplist00 file")
    error("wrong magic", b"bplist01" + bytes(40), "not a bplist00 file")
    error("zero offset size", assemble([true], offset_size=0), "invalid offset or reference size")
    error("ref size nine", assemble([true], ref_size=9), "invalid offset or reference size")
    error("no objects", assemble([true], num=0), "object count out of range")
    error("object count beyond table", assemble([true], num=1000), "object count out of range")
    error("huge object count", assemble([true], num=2**63), "object count out of range")
    error("top out of range", assemble([true], top=1), "top object out of range")
    error("table offset in header", assemble([true], table=4), "offset table out of range")
    error("table offset past end", assemble([true], table=2**40), "offset table out of range")
    error("object offset in header", assemble([true], offsets=[3]), "offset out of range")
    error("object offset at table", assemble([true], offsets=[9]), "offset out of range")
    error("ref out of range", assemble([bytes([0xA1, 0x05]), true]), "reference out of range")
    error("self cycle", assemble([bytes([0xA1, 0x00])]), "reference cycle")
    error("two-node cycle", assemble([bytes([0xA1, 0x01]), bytes([0xD1, 0x02, 0x00]), b"\x51k"]),
          "reference cycle")
    error("truncated string", assemble([bytes([0x55]) + b"ab"]), "unexpected end")
    error("count overruns", assemble([bytes([0x4F, 0x13]) + (2**62).to_bytes(8, "big")]),
          "unexpected end")
    error("count not an int", assemble([bytes([0x4F, 0x50])]), "invalid count")
    error("dict key not string", assemble([bytes([0xD1, 0x01, 0x01]), bytes([0x10, 0x01])]),
          "dict key is not a string")
    error("uid unsupported", assemble([bytes([0x80, 0x01])]), "UID")
    error("set unsupported", assemble([bytes([0xC0])]), "sets")
    error("bad marker", assemble([bytes([0x70])]), "unsupported marker")
    error("bad int size", assemble([bytes([0x15]) + bytes(32)]), "invalid integer size")
    error("bad real size", assemble([bytes([0x21, 0, 0])]), "invalid real size")
    error("fill byte", assemble([bytes([0x0F])]), "unsupported marker")
    # 200 nested arrays (array i -> i+1): deeper than MAX_DEPTH.
    deep = [bytes([0xA1]) + (i + 1).to_bytes(2, "big") for i in range(200)] + [true]
    error("too deep", assemble(deep, offset_size=2, ref_size=2), "nesting too deep")

    header = (
        "# bplist00 known answers. Generated by scripts/vectors/bplist_fixtures.py\n"
        "# (plistlib + plutil). See that script for the tagged expected format.\n"
        "# file = fixture under tests/fixtures/codec/bplist/; error = substring of\n"
        "# the decode error.\n\n"
    )
    with open(VECTORS, "w", encoding="utf-8", newline="\n") as f:
        f.write(header + "\n".join(blocks))


if __name__ == "__main__":
    main()
