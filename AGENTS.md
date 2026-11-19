# Agent instructions

Read `PLAN.md` first. Read `MULTI_AGENT.md` if you were given a role or
workstream.

## Rules

- Rust workspace: `crates/tvpipe` (the library), `crates/tvpipe-cli` (the
  `tvpipe` binary, on the library's API), `crates/tvpipe-fairplay` (the LGPL
  FairPlay port; nothing outside it may copy from it), `crates/tvpipe-party`
  (the host for multi-screen party games), `crates/tvpipe-canvas` (a small
  2D software renderer for game TV screens) and `games/drawbluff` (the
  reference game, not published). No async runtime anywhere: use
  `std::thread` and blocking I/O.
- Dependencies: `tvpipe` and `tvpipe-fairplay` use only `ring` and `rustls`
  (ring backend, no default features); `argument` is allowed in `tvpipe-cli`
  only; `tvpipe-canvas` has none. `tvpipe-party` may additionally use `rtc`
  (behind its optional `webrtc` feature) and `deser` (behind its optional
  `deser` feature), both owner-approved; its default features add nothing
  beyond `tvpipe`. Games may take their own dependencies. Do not add crates
  anywhere else without asking the human first.
- Hand-roll formats and simple algorithms (see PLAN §2/§9), each with
  known-answer tests or fixtures.
- Never panic on data from the network or from ffmpeg. Return errors.
- Platform-neutral code only. No OS media/discovery APIs; `cfg(target_os)` is
  only acceptable for config-directory paths. The exceptions are
  `crates/tvpipe/src/signals.rs` (Ctrl-C handling, `cfg(unix)`/`cfg(windows)`) and
  `crates/tvpipe/src/util/timer.rs` (Windows 1 ms timer resolution, `cfg(windows)`); see
  "Platform-Specific Code" in `ARCHITECTURE.md`.

## Before committing

`cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
