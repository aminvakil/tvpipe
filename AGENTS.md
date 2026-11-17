# Agent instructions

Read `PLAN.md` first. Read `MULTI_AGENT.md` if you were given a role or
workstream.

## Rules

- Rust, single binary crate. No async runtime: use `std::thread` and blocking
  I/O.
- Dependencies: only `argument` (CLI), `rustls` (ring backend, no default
  features), and `ring`. Do not add crates; propose them in `docs/deps.md`
  instead.
- Hand-roll formats and simple algorithms (see PLAN §2/§9), each with
  known-answer tests or fixtures.
- Never panic on data from the network or from ffmpeg. Return errors.
- Platform-neutral code only. No OS media/discovery APIs; `cfg(target_os)` is
  only acceptable for config-directory paths. The one exception is
  `src/util/interrupt.rs` (Ctrl-C handling, `cfg(unix)`/`cfg(windows)`; see
  `docs/decisions/0004-ctrl-c.md`).

## Clean room

- Do **not** read or copy code from doubletake, UxPlay, RPiPlay, playfair,
  airfry, or RottingApple unless your assigned role is "spec writer (dirty
  room)" or "FairPlay LGPL porter". Spec writers never write code in `src/`.
- **FairPlay is an LGPL port for now** (human decision 2026-11-18, PLAN §10
  fallback; `docs/decisions/0005-fairplay-lgpl.md`). Only the FairPlay porter
  reads doubletake, and it writes only `src/airplay/fairplay/**` (plus its
  vectors and license files). That directory is LGPL-3.0-or-later; nothing
  outside it may copy from it or from doubletake.
- Permissive references (openscreen, pyatv, pychromecast, node-castv2,
  rust_cast, RFCs) may be read and ported with attribution.
- Commit bodies for protocol code must include a `Sources:` line (see
  MULTI_AGENT.md).

## Before committing

`cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
