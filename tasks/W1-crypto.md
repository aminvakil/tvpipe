# W1 Crypto

Status: in progress. Owns `src/crypto/**`, `tests/vectors/crypto/**`.

- [x] `aes.rs`: AES-128/256 block encrypt (+ decrypt for CBC), table-based
      (T-tables), key expansion. FIPS-197 Appendix C KATs.
- [x] `modes.rs`: CTR (big-endian 128-bit counter, arbitrary-length input,
      resumable keystream position so a continuous stream can be encrypted
      in pieces) and CBC (no padding; caller handles partial blocks).
      SP 800-38A F.2/F.5 KATs. A release-mode throughput test
      (`#[ignore]`, ≥200 MB/s for CTR on a modern CPU).
      Measured: 741 MB/s AES-128-CTR on Apple M5 Max
      (`cargo test --release ctr_throughput -- --ignored --nocapture`).
- [ ] `bigint.rs`: fixed-width unsigned big integers sized for 3072-bit
      (and 4096-bit headroom): from/to big-endian bytes, add, sub, mul, mod,
      modpow (Montgomery or plain). Vectors generated with Python `pow()`.
- [ ] `srp.rs`: SRP-6a client, SHA-512, RFC 5054 3072-bit group, HAP
      conventions (username `Pair-Setup`, k = H(N | pad(g)), u = H(pad(A) |
      pad(B)), x = H(s | H(I ":" P)), K = H(S), M1 = H(H(N) xor H(g) | H(I) |
      s | A | B | K), M2 = H(A | M1 | K)). Test with a transcript you
      generate with an independent Python implementation (write the
      script to `scripts/vectors/srp.py`). Expose: client new(user, pin,
      salt, B, private a from rng) → A, M1, verify(M2) → session key.
- [x] `ring.rs`: thin wrappers: Ed25519 keypair (from/to 32-byte seed),
      sign/verify; X25519 ephemeral agreement (note: ring's X25519 is
      ephemeral-only, which is fine for pair-verify); ChaCha20-Poly1305
      seal/open with 12-byte nonce and AAD (HAP uses 8-byte counters
      left-padded to 12, and string nonces like `PS-Msg05` padded with
      zeros on the *left*); HKDF-SHA512 (salt, ikm, info, len);
      SHA-1/256/512; HMAC-SHA1/256/512; CSPRNG fill.

Done when: all KATs pass; clippy clean; AES-CTR ≥200 MB/s in release.

Sources allowed: FIPS-197, SP 800-38A, RFC 5054, RFC 8439, RFC 5869,
HAP spec (public), pyatv (MIT). Never doubletake/UxPlay/playfair/airfry/
RottingApple.
