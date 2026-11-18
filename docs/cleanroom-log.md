# Clean-room log

Which agent session had which role, which inputs it saw, and what it produced.

| Date | Agent | Role | Inputs seen | Output |
|---|---|---|---|---|
| 2026-11-17 | lead (e7336ea3) | lead / integrator | PLAN.md, MULTI_AGENT.md, `argument` crate source | M0 scaffold |
| 2026-11-17 | subagent `w1-crypto` | clean implementer W1 | repo, FIPS-197, SP 800-38A, RFC 5054/8439/8032/5869, HAP spec, srptools (MIT, as oracle), Python stdlib; no tainted sources | `src/crypto/**` (done) |
| 2026-11-17 | subagent `w2-codecs` | clean implementer W2 | repo, RFC 8259, HAP spec, protobuf encoding docs, pychromecast `cast_channel.proto` + generated `cast_channel_pb2` (MIT), CPython `plistlib.py` (PSF), plutil; no tainted sources | `src/codec/**` (done) |
| 2026-11-17 | subagent `w3-net` | clean implementer W3 | repo, RFC 1035/6762/6763/9110/9112/2326, pyatv + pychromecast (MIT), live LAN captures; no tainted sources | `src/net/**`, `airplay/discovery.rs` (done, LG/Samsung/Cast fixtures pending) |
| 2026-11-17 | subagent `w4-media` | clean implementer W4 | repo, RFC 8794, RFC 9559, ITU-T H.264, ISO/IEC 14496-15/-3, RFC 6716/7845, ffmpeg docs and black-box output; no tainted sources | `src/media/**`, `examples/slrw-gen.rs`, media fixtures/vectors/scripts (done) |

Note: subagents share this machine's filesystem, which contains tainted
checkouts under `~/.cache/checkouts/github.com/{omarroth,nored,uber6}`.
Isolation is by instruction only, not by sandbox.
| 2026-11-17 | subagent `w5-pairing` | clean implementer W5 | repo, HAP spec, pyatv (MIT), RFCs, Apple TV black-box | `src/airplay/{pairing,secure,credentials,info,pair}.rs` |
| 2026-11-17 | subagent `w8-cast` | clean implementer W8 | repo, openscreen (BSD-3), pychromecast/node-castv2/rust_cast (MIT), RFCs, openssl as oracle; no tainted sources | `src/cast/**` (done on openscreen receiver) |
| 2026-11-18 | subagent `w6-fairplay` | FairPlay LGPL porter (dirty, decision 0005) | doubletake (LGPL-3.0+), repo | `src/airplay/fairplay/**` (LGPL) |
| 2026-11-18 | subagent `s2-spec` | spec writer (dirty room) | doubletake @ be27776 (`internal/airplay/*`: mirror, client, fairplay, compatibility, audio*, alac_compressed, video_packetizer, video_codec, mirror_hevc, latency, boot_clock_*, event_channel, media_data_control, pairing, discovery, digest, debug, receiver_server, receiver_media, capture, credentials, receiver_identity, test names; `cmd/doubletake*/main.go`, `docs/compatibility.md`, README); UxPlay @ 48f5ce4 (`lib/raop_rtp_mirror.c`, `mirror_buffer.c`, `raop_buffer.c`, `raop_rtp.c`); aead/chacha20poly1305 (MIT); repo incl. fairplay public API; black-box runs against the Apple TV; scratch in `~/.cache/streamlib-dirty/` (not committed) | `docs/spec/airplay-mirroring.md`, `docs/spec/data/`, `tests/vectors/airplay-mirror/` |
| 2026-11-18 | subagent `spec-review` | spec reviewer (dirty) | doubletake, UxPlay, S2 spec + vectors | review: approve with changes (4 required, 10 gaps), all applied by S2 in 9f1dda5 |
| 2026-11-18 | subagent `w7-mirror` | clean implementer W7 | repo incl. `docs/spec/airplay-mirroring.md` + vectors, pyatv, RFCs; fairplay/ public API only | `src/airplay/{mirror,audio,timing,sink}.rs`, session.rs AirPlay path |
