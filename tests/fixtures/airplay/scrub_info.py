#!/usr/bin/env python3
"""Scrub a captured /info bplist body in place-equivalent form: identifiers
are replaced by fakes of the same length (so offsets and the object table
stay exactly as the receiver sent them), matching the fakes used in
tests/fixtures/net/ (device id AA:BB:CC:00:00:0A, pk = bytes 00..1f).

    python3 tests/fixtures/airplay/scrub_info.py IN.bplist OUT.bplist
"""
import plistlib
import sys

data = open(sys.argv[1], "rb").read()
p = plistlib.loads(data)
subs = {
    p["deviceID"].encode(): b"AA:BB:CC:00:00:0A",
    p["macAddress"].encode(): b"AA:BB:CC:00:00:99",
    p["pi"].encode(): b"aabbcc00-000c-aabb-cc00-000caabbcc00",
    p["psi"].encode(): b"AABBCC00-000D-AABB-CC00-000DAABBCC00",
    p["pk"]: bytes(range(32)),
}
host, port = p["senderAddress"].rsplit(":", 1)
fake_sender = ("192.168.1.101:" + "6" * len(port))
fake_sender = fake_sender[: len(p["senderAddress"])].ljust(len(p["senderAddress"]), "0")
subs[p["senderAddress"].encode()] = fake_sender.encode()
for old, new in subs.items():
    assert len(old) == len(new), (old, new)
    assert data.count(old) >= 1, old
    data = data.replace(old, new)
q = plistlib.loads(data)
assert set(q) == set(p)
open(sys.argv[2], "wb").write(data)
