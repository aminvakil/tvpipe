# Building tvpipe with multiple agents

How a team of coding agents (plus one human) could build `PLAN.md` in
parallel without stepping on each other or on the clean-room rules.

## Why this project suits a multi-agent setup

- **Most of the code is pure, testable leaf modules**: AES, SRP, bplist,
  TLV8, JSON, protobuf, DNS, Matroska, H.264 SPS, RTP/RTCP. Each one has an
  external oracle (published vectors, `ffprobe`, `plutil`, `protoc`, Python).
  An agent can work alone until the tests are green.
- **The two protocol stacks are independent** once the core types exist.
  AirPlay and Cast meet only at the `Sink` trait.
- **The clean-room requirement needs separated roles anyway.** Agents with
  explicitly different access rights are a natural fit.
- **The hard feedback loops need hardware and a human.** Everything else can
  be automated with local receivers.

## Roles

| Role | Count | Access | Responsibilities |
|---|---|---|---|
| **Lead / integrator** | 1 | repo, permissive refs | owns `PLAN.md`, the core types (§3), task board, merges, cross-module integration, `session.rs`, `main.rs` |
| **Spec writer (dirty room)** | 1–2 | repo + tainted sources (`doubletake`, UxPlay, playfair, …) | writes `docs/spec/*.md`, `docs/spec/data/*.hex`, `tests/vectors/**`; builds and maintains the external oracle harness; **never writes code in `src/`** |
| **Spec reviewer** | 1 | spec + tainted sources | checks that specs describe function and facts, not copied expression; signs off before clean implementers see a spec |
| **Implementers (clean room)** | 3–6 | repo, permissive refs, specs, oracle *binary* only | one workstream each (below); sandboxed without `~/.cache/checkouts/**/doubletake`, UxPlay, airfry, or RottingApple |
| **Test / infra** | 1 | repo, may *run* (not read) receivers | CI matrix, local receiver scripts (`doubletake-test-receiver`, UxPlay, openscreen `cast_receiver`), fixture capture tooling, latency test-pattern tooling |
| **Code reviewer** | 1 (or rotating) | repo | reviews every merge for the dependency policy, provenance lines, unsafe code, panics on network input, and test coverage |
| **Human** | 1 | everything + hardware | enters TV PINs, runs hardware tests, captures traffic, decides on license or purchases, breaks ties |

Agents should not switch roles. A dirty-room agent stays dirty for the whole
project; start a fresh agent to change role.

## Workstreams and dependency graph

```
M0 Scaffold + core types (Lead)
 ├── W1 Crypto: aes, modes, bigint, srp, ring wrappers ──────────────┐
 ├── W2 Codecs: json, bplist, tlv8, protobuf ────────────────────────┤
 ├── W3 Net: mdns, iface, http/rtsp framing → discovery (M2) ────────┤
 ├── W4 Media: ffmpeg, mkv, h264, rawin (SLRW), clock, fanout (M3) ──┤
 │                                                                   │
 ├── S1 Spec: FairPlay (dirty) ──► R review ──► W6 FairPlay impl (M5)│
 ├── S2 Spec: AirPlay mirroring/audio/timing (dirty) ──► R ──┐       │
 │                                                           ▼       ▼
 │                       W5 AirPlay pairing + secure channel (M4, needs W1,W2,W3)
 │                       W7 AirPlay mirror + audio + timing (M6–M7, needs W4,W5,W6,S2)
 │
 └── W8 Cast control channel + Streaming (M8–M9, needs W1 AES, W2 json/protobuf, W3, W4)
                                                                     │
                                         Lead: session.rs, fanout, CLI wiring, replication (M10)
```

What can run at the same time after M0: W1, W2, W3, W4, S1, S2, plus the
infra work. That is about seven agents. W5 and W8 start once their
foundations land; W6 starts once spec S1 is approved.

### Workstream cards

Each card becomes a task file in `tasks/` (or an issue):

- **W1 Crypto**:
  - AES-128/256 block (table-based), CTR, and CBC, with FIPS-197 and
    SP 800-38A KATs and a throughput benchmark.
  - Fixed-width bigint with modpow; SRP-6a client with a SHA-512/3072
    transcript test.
  - Thin `ring` wrappers.
  - Done when: KATs pass on all three OSes, and AES-CTR runs at ≥200 MB/s in
    release.
- **W2 Codecs**: JSON value/parse/serialize; bplist00 read and write (dicts,
  arrays, ints, reals, bools, data, strings, UIDs not needed); TLV8 with
  fragmentation; `CastMessage` encode/decode. Done when: fixture round-trips
  against `plutil`/`plistlib`/`protoc`, plus fuzz smoke tests.
- **W3 Net**:
  - DNS codec with compression; legacy-unicast mDNS browse loop; TXT parsing
    for both protocols; device merging.
  - Local-address selection; HTTP/RTSP request/response framing that works on
    a plain stream or our encrypted wrapper.
  - Done when: `tvpipe discover` against the real network, plus parsing
    fixtures of captured responses.
- **W4 Media**:
  - ffmpeg locate/probe/argv builder from `EncodePlan`.
  - Live Matroska demuxer, H.264 SPS/avcC.
  - SLRW raw input with a sample generator binary (`examples/slrw-gen`);
    media clock; fanout with keyframe-skip.
  - Done when: `debug pipeline` output matches `ffprobe -show_packets` for
    test files, and SLRW works end to end.
- **W5 AirPlay pairing**:
  - Controller identity and credential store.
  - Transient and PIN pair-setup, pair-verify, encrypted control channel,
    `/info`.
  - Done when: it pairs with the Apple TV (human types the PIN) and
    re-verifies after a restart; test-receiver `--auth pin` loop green.
- **W6 FairPlay (clean)**: implement the `FpSession` API from §7.3 using only
  `docs/spec/fairplay.md`, the data files, vectors, and the oracle binary.
  Done when: all vectors pass, differential fuzzing finds no mismatch in 10⁶
  cases, and the Apple TV accepts the handshake.
- **W7 AirPlay mirror**: SETUP/RECORD flow, stream key derivation, 128-byte
  video packetizer, AES-CTR, heartbeat, PTP anchoring, NTP responder,
  realtime audio, teardown. Done when: M6/M7 exit criteria are met on the
  Apple TV, and the test receiver is green for `modern`, `lg`, `appletv3`.
- **W8 Cast**:
  - TLS channel (rustls custom verifier), heartbeat, launch, OFFER/ANSWER.
  - Cast RTP packetizer with AES-CTR, RTCP SR/feedback, NACK retransmission,
    PLI handling.
  - Done when: openscreen `cast_receiver` shows video and audio in sync, the
    stream survives `tc netem`-style 2% loss, and it plays on real Cast
    hardware.

## Interfaces first

The lead freezes these in M0, before any workstream starts:

- `EncodedFrame`, `VideoInfo`, `AudioInfo`, `Capabilities`, `EncodePlan`, the
  `Sink` trait (PLAN §3).
- `Error` type and conventions: one crate-wide error enum per module, no
  panics on network input, `Result` everywhere.
- Byte helpers (`util::bytes::{Reader, Writer}`) so every codec parses the
  same way.
- The test vector file format: line-based `key = hex` blocks separated by
  blank lines, read by a tiny shared test helper. No JSON dependency in tests.

Changing a frozen interface needs a short proposal that the lead approves;
record the outcome in `ARCHITECTURE.md` (no separate decision records). Implementers can add private helpers freely.

## Working agreements

- **Isolation**: each agent works in its own git worktree and branch
  (`w1-crypto`, `w8-cast`, …). It owns only its module paths, and touching
  another module needs that module's owner to review.
- **Small merges**: merge into `main` as soon as a coherent piece is green.
  Don't keep long-lived branches.
- **Definition of done**: `cargo fmt`, `cargo clippy -- -D warnings`, and
  `cargo test` green on all three OSes; new code has KATs or fixtures; no new
  dependency.
- **Provenance line** in every commit body touching protocol code, for
  example `Sources: openscreen cast/streaming/rtp_defines.h; PLAN §8.3` or
  `Sources: docs/spec/fairplay.md §4.2, vectors fairplay/m3.txt`. Tainted
  sources must never appear there.
- **Dependency policy**: the only crates are `argument`, `rustls`, and `ring`.
  Anything else needs the human's approval.
- **Logs over guesses**: on hardware, agents ask the human to run
  `tvpipe --debug …` and attach the log or capture, instead of guessing
  at receiver behaviour.
- **No silent spec gaps**: if a clean implementer needs a fact the spec lacks,
  it asks the lead, who relays it to the spec writer. The spec writer answers by
  amending the spec. The implementer never goes to look in the tainted source
  itself.

## Enforcing the clean room in practice

- Run clean agents in a container or sandbox whose filesystem has the repo and
  the oracle *binary*, with no `~/.cache/checkouts/github.com/{omarroth,FDH2,
  nored,uber6,FD-}` and no web fetch for those hosts.
- Dirty agents get a separate scratch area (for example
  `~/.cache/streamlib-dirty/`). It holds the oracle harness source and the
  notes from reading tainted code, and is never committed to this repo.
- The spec reviewer gates every spec change. Reviewed specs are committed by
  the reviewer, not the writer.

## Human checkpoints

1. After M0: approve the frozen types and the CLI shape.
2. After M2: confirm all TVs are discovered; note model years.
3. M4: type the pairing PIN; confirm credentials persist.
4. M5/M6: first mirrored picture on the Apple TV.
5. M7: lip-sync check by eye and ear; LG and Samsung attempts.
6. M8: Cast hardware decision (built-in vs. dongle).
7. M10: replication across devices; latency table in `docs/measurements.md`.

## Suggested kickoff

1. Lead agent: M0 (crate, CLI skeleton with `argument`, core types, CI,
   vector-file helper). About one session.
2. Then start at once: W1, W2, W3, W4 (clean), S1 + S2 (dirty), and infra.
3. As soon as W1+W2+W3 land: W5 and W8.
4. As soon as S1 is reviewed: W6. As soon as W5+W6+S2 land: W7.
5. The lead integrates continuously and owns `session.rs` and replication.
