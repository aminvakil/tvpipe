# Agent instructions

Read `PLAN.md` first. Read `MULTI_AGENT.md` if you were given a role or
workstream.

## Rules

- Rust, single binary crate. No async runtime: use `std::thread` and blocking
  I/O.
- Dependencies: only `argument` (CLI), `rustls` (ring backend, no default
  features), and `ring`. Do not add crates without asking the
  human first.
- Hand-roll formats and simple algorithms (see PLAN §2/§9), each with
  known-answer tests or fixtures.
- Never panic on data from the network or from ffmpeg. Return errors.
- Platform-neutral code only. No OS media/discovery APIs; `cfg(target_os)` is
  only acceptable for config-directory paths. The exceptions are
  `src/util/interrupt.rs` (Ctrl-C handling, `cfg(unix)`/`cfg(windows)`) and
  `src/util/timer.rs` (Windows 1 ms timer resolution, `cfg(windows)`); see
  "Platform-Specific Code" in `ARCHITECTURE.md`.

## Before committing

`cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
