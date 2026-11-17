#!/usr/bin/env python3
"""Generates tests/vectors/codec/protobuf.txt (CastMessage known answers).

Needs the `protobuf` Python package and pychromecast's generated
`cast_channel_pb2.py` (MIT; built by protoc from Chromium's
cast_channel.proto). protoc itself is not required:

    python3 -m venv /tmp/venv && /tmp/venv/bin/pip install protobuf
    git clone --depth 1 https://github.com/home-assistant-libs/pychromecast /tmp/pychromecast
    PYCHROMECAST_GENERATED=/tmp/pychromecast/pychromecast/generated \\
        /tmp/venv/bin/python scripts/vectors/protobuf_fixtures.py

Encodable vectors are serialized by the protobuf runtime. Decode-only and
error vectors are hand-built wire bytes; each is checked against the
runtime's parser (accepted with the same fields, or rejected) unless noted.
"""
import os
import sys

sys.path.insert(0, os.environ["PYCHROMECAST_GENERATED"])
import cast_channel_pb2 as pb  # noqa: E402
from google.protobuf.message import DecodeError  # noqa: E402

HB = "urn:x-cast:com.google.cast.tp.heartbeat"
CONN = "urn:x-cast:com.google.cast.tp.connection"
RECV = "urn:x-cast:com.google.cast.receiver"
AUTH = "urn:x-cast:com.google.cast.tp.deviceauth"
WEBRTC = "urn:x-cast:com.google.cast.webrtc"

blocks = []


def hexwrap(key, data):
    h = data.hex()
    s = f"{key} = {h[:64]}\n"
    for i in range(64, len(h), 64):
        s += "  " + h[i : i + 64] + "\n"
    return s


def describe(m):
    s = f"protocol_version = {m.protocol_version}\n"
    s += f"source_id = {m.source_id}\n"
    s += f"destination_id = {m.destination_id}\n"
    s += f"namespace = {m.namespace}\n"
    if m.payload_type == pb.CastMessage.STRING:
        s += "payload_type = STRING\n"
        assert m.payload_utf8 == m.payload_utf8.strip(), "vector format trims"
        s += f"payload_utf8 = {m.payload_utf8}\n"
    else:
        s += "payload_type = BINARY\n"
        s += hexwrap("payload_binary", m.payload_binary)
    return s


def encodable(name, **kw):
    m = pb.CastMessage(protocol_version=pb.CastMessage.CASTV2_1_0, **kw)
    data = m.SerializeToString()
    blocks.append(f"name = {name}\n" + hexwrap("encoded", data) + describe(m))


def key(field, wt):
    return varint(field << 3 | wt)


def varint(v):
    out = b""
    while v >= 0x80:
        out += bytes([v & 0x7F | 0x80])
        v >>= 7
    return out + bytes([v])


def ld(field, data):
    if isinstance(data, str):
        data = data.encode()
    return key(field, 2) + varint(len(data)) + data


def vi(field, v):
    return key(field, 0) + varint(v)


def decode_only(name, data):
    m = pb.CastMessage()
    m.ParseFromString(data)
    assert m.IsInitialized(), name
    blocks.append(
        f"name = {name}\ndecode_only = 1\n" + hexwrap("encoded", data) + describe(m)
    )


def error(name, data, msg, python_rejects=True):
    if python_rejects:
        m = pb.CastMessage()
        try:
            m.ParseFromString(data)
        except DecodeError:
            pass
        else:
            # The runtime parses partial messages; required fields are
            # checked separately.
            if m.IsInitialized():
                raise AssertionError(f"python accepted {name}")
    note = "" if python_rejects else "# python protobuf accepts this; we are stricter\n"
    blocks.append(f"{note}name = {name}\n" + hexwrap("encoded", data) + f"error = {msg}\n")


encodable("heartbeat PING", source_id="sender-0", destination_id="receiver-0",
          namespace=HB, payload_type=pb.CastMessage.STRING, payload_utf8='{"type":"PING"}')
encodable("connection CONNECT", source_id="sender-0", destination_id="receiver-0",
          namespace=CONN, payload_type=pb.CastMessage.STRING,
          payload_utf8='{"type":"CONNECT","userAgent":"streamlib"}')
encodable("receiver LAUNCH", source_id="sender-0", destination_id="receiver-0",
          namespace=RECV, payload_type=pb.CastMessage.STRING,
          payload_utf8='{"type":"LAUNCH","requestId":1,"appId":"0F5096E8"}')
encodable("binary payload", source_id="sender-0", destination_id="receiver-0",
          namespace=AUTH, payload_type=pb.CastMessage.BINARY,
          payload_binary=bytes(range(0, 256, 7)))
encodable("empty strings", source_id="", destination_id="", namespace="",
          payload_type=pb.CastMessage.STRING, payload_utf8="")
encodable("empty binary", source_id="a", destination_id="*", namespace=AUTH,
          payload_type=pb.CastMessage.BINARY, payload_binary=b"")
offer = '{"type":"OFFER","seqNum":1,"offer":{"castMode":"mirroring","supportedStreams":[' + ",".join(
    '{"index":%d,"type":"video_source","codecName":"h264","rtpProfile":"cast","rtpPayloadType":96,"ssrc":%d}' % (i, 1000 + i)
    for i in range(3)) + "]}}"
encodable("long payload two-byte length", source_id="sender-0", destination_id="web-5",
          namespace=WEBRTC, payload_type=pb.CastMessage.STRING, payload_utf8=offer)
encodable("unicode payload", source_id="sender-0", destination_id="receiver-0",
          namespace=RECV, payload_type=pb.CastMessage.STRING,
          payload_utf8='{"name":"Wohnzimmer-TV — Küche 📺"}')

base = [vi(1, 0), ld(2, "s"), ld(3, "d"), ld(4, "ns"), vi(5, 0), ld(6, "{}")]
decode_only("fields reordered", b"".join(reversed(base)))
decode_only("unknown fields skipped",
            base[0] + vi(8, 1) + b"".join(base[1:4]) + ld(9, b"\x00\x01\x02")
            + key(15, 5) + b"\x01\x02\x03\x04" + b"".join(base[4:])
            + key(100, 1) + bytes(8) + vi(2047, 2**64 - 1))
decode_only("repeated scalar last wins", b"".join(base[:3]) + ld(4, "old") + ld(4, "new") + b"".join(base[4:]))
decode_only("non-minimal varints", b"\x08\x80\x00" + b"".join(base[1:4]) + b"\x28\x80\x80\x00" + base[5])
decode_only("binary type ignores utf8 field", b"".join(base[:4]) + vi(5, 1) + ld(6, "x") + ld(7, b"\xff"))

error("missing namespace", b"".join(base[:3] + base[4:]), "missing required field namespace")
error("missing payload_type", b"".join(base[:4] + base[5:]), "missing payload_type")
error("truncated length", b"".join(base)[:-1], "unexpected end")
error("truncated varint", b"".join(base) + b"\x08\x80", "unexpected end")
error("wrong wire type for source_id", vi(1, 0) + vi(2, 5) + b"".join(base[2:]), "wrong wire type")
decode_only("unknown groups skipped",
            b"".join(base[:2]) + key(9, 3) + vi(1, 5) + key(10, 3) + ld(2, "x") + key(10, 4)
            + key(9, 4) + b"".join(base[2:]) + key(11, 3) + key(11, 4))
error("stray end-group", b"".join(base) + key(9, 4), "unexpected end-group")
error("mismatched end-group", b"".join(base) + key(9, 3) + key(10, 4), "mismatched end-group")
error("unterminated group", b"".join(base) + key(9, 3) + vi(1, 1), "unexpected end")
error("field number zero", b"".join(base) + b"\x00\x00", "invalid field number")
error("invalid wire type 7", b"".join(base) + b"\x0f", "invalid wire type")
error("unknown payload_type", b"".join(base[:4]) + vi(5, 2) + base[5],
      "unknown payload_type", python_rejects=False)
error("invalid utf-8 in source_id", vi(1, 0) + ld(2, b"\xc3\x28") + b"".join(base[2:]),
      "not UTF-8", python_rejects=False)

root = os.path.join(os.path.dirname(__file__), "..", "..")
out = os.path.join(root, "tests", "vectors", "codec", "protobuf.txt")
header = (
    "# CastMessage known answers. Generated by scripts/vectors/protobuf_fixtures.py\n"
    "# with the Python protobuf runtime and pychromecast's cast_channel_pb2.\n"
    "# decode_only: hand-built bytes (parsed identically by the runtime).\n"
    "# error: substring of our error message; decoding must fail.\n\n"
)
with open(out, "w", encoding="utf-8", newline="\n") as f:
    f.write(header + "\n".join(blocks))
