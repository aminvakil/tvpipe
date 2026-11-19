# Agent instructions

Read `PLAN.md` first. Read `MULTI_AGENT.md` if you were given a role or
workstream.

## Rules

- Rust workspace of three crates: `crates/tvpipe` (the library),
  `crates/tvpipe-cli` (the `tvpipe` binary, on the library's API) and
  `crates/tvpipe-fairplay` (the LGPL FairPlay port; nothing outside it may
  copy from it). No async runtime: use `std::thread` and blocking I/O.
- Dependencies: only `argument` (in `tvpipe-cli` only), `rustls` (ring
  backend, no default features), and `ring`. Do not add crates without
  asking the human first.
- Hand-roll formats and simple algorithms (see PLAN §2/§9), each with
  known-answer tests or fixtures.
- Never panic on data from the network or from ffmpeg. Return errors.
- Platform-neutral code only. No OS media/discovery APIs; `cfg(target_os)` is
  only acceptable for config-directory paths. The exceptions are
  `crates/tvpipe-cli/src/interrupt.rs` (Ctrl-C handling, `cfg(unix)`/`cfg(windows)`) and
  `crates/tvpipe/src/util/timer.rs` (Windows 1 ms timer resolution, `cfg(windows)`); see
  "Platform-Specific Code" in `ARCHITECTURE.md`.

## Before committing

`cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
