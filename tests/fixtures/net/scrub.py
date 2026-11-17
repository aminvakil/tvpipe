#!/usr/bin/env python3
"""Turn a `streamlib --debug discover` log into a scrubbed fixture file.

    streamlib --debug discover --timeout 6 2> capture.log
    python3 tests/fixtures/net/scrub.py capture.log > tests/fixtures/net/NAME.txt

Packets are deduplicated and identifiers are replaced in place by fakes of
the same length, so the wire layout (including compression pointers) stays
exactly as the receiver sent it:

- MAC-like ids (`deviceid`, `btaddr`, RAOP instance prefixes, `Sonos-...`
  host names), in colon, plain and lower-case forms;
- `pk`, `pi`, `psi`, `gid`, `pgid`, `gcgl` (group/pairing ids), Cast `id`,
  `cd`, `bs` values;
- IPv6 addresses (EUI-64 or privacy suffixes);
- (packets that differ only in the query id are kept once)
- extra literal replacements given as `--sub OLD=NEW` (same length).

Output is the shared vector format: blocks of `from = IP` and `msg = HEX`.
"""
import re
import sys

ID_KEYS = {b"deviceid", b"btaddr", b"pk", b"pi", b"psi", b"gid", b"pgid", b"gcgl",
           b"id", b"cd", b"bs", b"hkid", b"serialnumber"}
# Shorter values (flags like `gcgl=0`) are not identifiers.
MIN_ID_LEN = 8
HEXCHARS = b"0123456789abcdefABCDEF"


def walk(pkt):
    """Yields (kind, offset, length) for TXT strings and AAAA rdata."""
    def skip_name(p):
        while True:
            n = pkt[p]
            if n == 0:
                return p + 1
            if n & 0xC0 == 0xC0:
                return p + 2
            p += 1 + n
    counts = [int.from_bytes(pkt[i:i + 2], "big") for i in range(4, 12, 2)]
    p = 12
    for _ in range(counts[0]):
        p = skip_name(p) + 4
    for _ in range(sum(counts[1:])):
        p = skip_name(p)
        rtype = int.from_bytes(pkt[p:p + 2], "big")
        rdlen = int.from_bytes(pkt[p + 8:p + 10], "big")
        p += 10
        if rtype == 16:
            q = p
            while q < p + rdlen:
                n = pkt[q]
                yield ("txt", q + 1, n)
                q += 1 + n
        elif rtype == 28 and rdlen == 16:
            yield ("aaaa", p, 16)
        p += rdlen


class Faker:
    def __init__(self):
        self.map = {}

    def fake(self, value):
        """Same-length fake that keeps separators and hex-ness."""
        if value in self.map:
            return self.map[value]
        n = len(self.map) + 1
        out = bytearray()
        digits = b"%012X" % (0xAABBCC000000 + n)
        if value != value.upper():
            digits = digits.lower()
        di = 0
        for c in value:
            if c in HEXCHARS:
                out.append(digits[di % len(digits)])
                di += 1
            elif chr(c).isalnum():
                out.append(ord("X") if chr(c).isupper() else ord("x"))
            else:
                out.append(c)
        self.map[value] = bytes(out)
        return self.map[value]


def main():
    args = sys.argv[1:]
    subs = []
    while "--sub" in args:
        i = args.index("--sub")
        old, new = args[i + 1].encode().split(b"=", 1)
        assert len(old) == len(new), "substitutions must keep the length"
        subs.append((old, new))
        del args[i:i + 2]
    packets = []
    seen = set()
    for line in open(args[0], encoding="utf-8"):
        m = re.search(r"bytes from ([0-9.:\[\]a-f]+):\d+: ([0-9a-f]+)$", line.strip())
        if not m:
            continue
        ip, data = m.group(1), bytes.fromhex(m.group(2))
        if data[2:] in seen:
            continue
        seen.add(data[2:])
        packets.append((ip, bytearray(data)))

    faker = Faker()
    replacements = list(subs)
    v6 = {}
    for _, pkt in packets:
        for kind, off, n in walk(pkt):
            if kind == "aaaa":
                addr = bytes(pkt[off:off + n])
                v6.setdefault(addr, addr[:8] + bytes([0xAA, 0xBB, 0xCC, 0xFF, 0xFE, 0, 0, len(v6) + 1]))
                pkt[off:off + n] = v6[addr]
                continue
            s = bytes(pkt[off:off + n])
            if b"=" not in s:
                continue
            k, v = s.split(b"=", 1)
            if k.lower() in ID_KEYS and len(v) >= MIN_ID_LEN:
                fake = faker.fake(v)
                replacements.append((v, fake))
                if k.lower() in (b"deviceid", b"btaddr"):
                    plain, fplain = v.replace(b":", b""), fake.replace(b":", b"")
                    replacements += [(plain, fplain), (plain.lower(), fplain.lower())]
    # Longest first so that ids embedded in longer ids are handled whole.
    replacements.sort(key=lambda r: -len(r[0]))
    print("# Scrubbed mDNS responses captured with `streamlib --debug discover`;")
    print("# see scrub.py. One block per distinct packet.")
    for ip, pkt in packets:
        data = bytes(pkt)
        for old, new in replacements:
            data = data.replace(old, new)
        assert len(data) == len(pkt)
        print()
        print(f"from = {ip}")
        hx = data.hex()
        print("msg = " + "\n      ".join(hx[i:i + 64] for i in range(0, len(hx), 64)))


main()
