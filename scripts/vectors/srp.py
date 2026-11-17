#!/usr/bin/env python3
"""Generates tests/vectors/crypto/srp.txt: SRP-6a client transcripts with
the HAP conventions (SHA-512, RFC 5054 3072-bit group, g = 5).

The arithmetic below is written from the formulas in RFC 5054 §2.5-2.6
and RFC 2945 §3, independently of src/crypto/srp.rs, and plays both roles
(client and server) so every transcript is self-consistent:

    k  = H(N | PAD(g))
    x  = H(s | H(I | ":" | P))
    v  = g^x % N
    A  = g^a % N                  B = (k*v + g^b) % N
    u  = H(PAD(A) | PAD(B))
    S  = (B - k*v)^(a + u*x) % N  (client) = (A * v^u)^b % N (server)
    K  = H(S)
    M1 = H(H(N) xor H(g) | H(I) | s | A | B | K)
    M2 = H(A | M1 | K)

Byte conventions follow srptools, the library pyatv uses for HAP pairing:
integers hash as minimal big-endian bytes (no leading zeros) except where
PAD() is written; the salt hashes as the raw bytes received.

If srptools is importable, the script also checks each transcript against
it, e.g.:

    uv run --no-project --with srptools==1.0.1 python3 scripts/vectors/srp.py
    uv run --no-project --with srptools python3 scripts/vectors/srp.py

Usage: python3 scripts/vectors/srp.py   (from the repository root)
"""
import hashlib
import os
import random

OUT = os.path.join(os.path.dirname(__file__), "..", "..", "tests", "vectors", "crypto", "srp.txt")

N = int(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74"
    "020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F1437"
    "4FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED"
    "EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF05"
    "98DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB"
    "9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3B"
    "E39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF695581718"
    "3995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D04507A33"
    "A85521ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7"
    "ABF5AE8CDB0933D71E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864"
    "D87602733EC86A64521F2B18177B200CBBE117577A615D6C770988C0BAD946E2"
    "08E24FA074E5AB3143DB5BFCE0FD108E4B82D120A93AD2CAFFFFFFFFFFFFFFFF",
    16,
)
G = 5
NLEN = 384


def H(*parts):
    return hashlib.sha512(b"".join(parts)).digest()


def b(v):
    """Minimal big-endian bytes (at least one byte)."""
    return v.to_bytes(max(1, (v.bit_length() + 7) // 8), "big")


def pad(v):
    return v.to_bytes(NLEN, "big")


def i(bs):
    return int.from_bytes(bs, "big")


def transcript(user, password, salt, a, b_priv):
    k = i(H(b(N), pad(G)))
    x = i(H(salt, H(user + b":" + password)))
    v = pow(G, x, N)
    A = pow(G, a, N)
    B = (k * v + pow(G, b_priv, N)) % N
    u = i(H(pad(A), pad(B)))
    S_client = pow((B - k * v) % N, a + u * x, N)
    S_server = pow(A * pow(v, u, N), b_priv, N)
    assert S_client == S_server
    K = H(b(S_client))
    hn, hg = H(b(N)), H(b(G))
    M1 = H(bytes(p ^ q for p, q in zip(hn, hg)), H(user), salt, b(A), b(B), K)
    M2 = H(b(A), M1, K)
    return dict(k=k, x=x, v=v, A=A, B=B, u=u, S=S_client, K=K, M1=M1, M2=M2)


def check_srptools(user, password, salt, a, t):
    try:
        from srptools import SRPClientSession, SRPContext, constants
    except ImportError:
        return False
    ctx = SRPContext(
        user.decode(),
        password.decode(),
        prime=constants.PRIME_3072,
        generator=constants.PRIME_3072_GEN,
        hash_func=hashlib.sha512,
    )
    s = SRPClientSession(ctx, private=format(a, "x"))
    s.process(format(t["B"], "x"), salt.hex())
    assert int(s.public, 16) == t["A"], "srptools A"
    assert bytes.fromhex(s.key) == t["K"], "srptools K"
    assert bytes.fromhex(s.key_proof) == t["M1"], "srptools M1"
    assert bytes.fromhex(s.key_proof_hash) == t["M2"], "srptools M2"
    return True


def wrap(h):
    return "\n    ".join(h[j:j + 64] for j in range(0, len(h), 64))


rng = random.Random(6054)
cases = []

# RFC 5054 Appendix B inputs (I, P, s, a, b), with SHA-512 and the 3072-bit
# group instead of SHA-1/1024, so the outputs differ from the RFC's.
cases.append((
    "RFC 5054 App. B inputs, SHA-512/3072",
    b"alice",
    b"password123",
    bytes.fromhex("BEB25379D1A8581EB5A727673A2441EE"),
    int("60975527035CF2AD1989806F0407210BC81EDC04E2762A56AFD529DDDA2D4393", 16),
    int("E487CB59D31AC550471E81F00F6928E01DDA08E974A004F49E61F5D105284D20", 16),
))

# HAP-style: username Pair-Setup, 16-byte salt, 8-digit and 4-digit PINs.
for label, pin in [("HAP PIN 1234", b"1234"), ("HAP transient PIN 3939", b"3939"),
                   ("HAP 8-digit setup code", b"123-45-678")]:
    cases.append((label, b"Pair-Setup", pin, rng.randbytes(16), rng.getrandbits(512),
                   rng.getrandbits(512)))

# Salt with a leading zero byte: hashed raw, not as an integer.
cases.append(("salt with leading zero", b"Pair-Setup", b"1234",
              b"\x00\x00" + rng.randbytes(14), rng.getrandbits(512), rng.getrandbits(512)))


def find(pred, label):
    """A server private b whose B (and the resulting S) satisfy `pred`."""
    salt, a = rng.randbytes(16), rng.getrandbits(512)
    while True:
        bp = rng.getrandbits(512)
        t = transcript(b"Pair-Setup", b"1234", salt, a, bp)
        if pred(t):
            return (label, b"Pair-Setup", b"1234", salt, a, bp)


# B < 2^3064: PAD(B) in u, minimal B in M1.
cases.append(find(lambda t: t["B"] < 1 << (8 * NLEN - 8), "B with leading zero byte"))
# S < 2^3064: K = H(minimal S).
cases.append(find(lambda t: t["S"] < 1 << (8 * NLEN - 8), "S with leading zero byte"))

blocks = []
checked = 0
for label, user, pw, salt, a, bp in cases:
    t = transcript(user, pw, salt, a, bp)
    checked += check_srptools(user, pw, salt, a, t)
    blocks.append([
        ("name", label),
        ("user", user.decode()),
        ("password", pw.decode()),
        ("salt", salt.hex()),
        ("a", b(a).hex()),
        ("b", b(bp).hex()),
        ("k", pad(t["k"]).hex()[-128:]),
        ("x", b(t["x"]).hex()),
        ("A", pad(t["A"]).hex()),
        ("B", pad(t["B"]).hex()),
        ("u", b(t["u"]).hex()),
        ("S", pad(t["S"]).hex()),
        ("K", t["K"].hex()),
        ("M1", t["M1"].hex()),
        ("M2", t["M2"].hex()),
    ])

with open(OUT, "w") as f:
    f.write("# Generated by scripts/vectors/srp.py (independent Python SRP-6a).\n")
    f.write("# SHA-512, RFC 5054 3072-bit group, g = 5; HAP/srptools byte conventions.\n")
    f.write("# A, B, S are written padded to 384 bytes; user and password are text.\n")
    f.write(f"# Cross-checked against srptools: {'yes' if checked else 'no'}.\n")
    for blk in blocks:
        f.write("\n")
        for key, val in blk:
            f.write(f"{key} = {wrap(val) if len(val) > 64 else val}\n")
print(f"{len(blocks)} transcripts, {checked} checked against srptools")
