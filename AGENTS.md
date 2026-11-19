# Agent instructions

Read `PLAN.md` first. Read `MULTI_AGENT.md` if you were given a role or
workstream.

## Rules

- Rust workspace: `crates/tvpipe` (the library; `tvpipe playshare`, the
  server for web games, is its `playshare` module), `crates/tvpipe-cli`
  (the `tvpipe` binary, on the library's API) and `crates/tvpipe-fairplay`
  (the LGPL FairPlay port; nothing outside it may copy from it). No async
  runtime anywhere: use `std::thread` and blocking I/O.
- JavaScript: `npm/tvpipe` (`@tvpipe/sdk`, the Node SDK), `npm/party`
  (`@tvpipe/party`, the browser library of playshare games: erasable
  TypeScript, browser APIs only, no runtime dependencies; `npm run build`
  copies it into `crates/tvpipe/src/playshare/web/`, which is checked in
  and must match) and `games/drawbluff` (the reference game, a web app,
  not published). Games run in browsers: no Node and no Rust game logic;
  Node only for tests and build scripts.
- Dependencies: `tvpipe` and `tvpipe-fairplay` use only `ring` and `rustls`
  (ring backend, no default features); `argument` is allowed in `tvpipe-cli`
  only. Games may take their own dependencies. Do not add crates or npm
  packages anywhere else without asking the human first.
- Hand-roll formats and simple algorithms (see PLAN §2/§9), each with
  known-answer tests or fixtures.
- Never panic on data from the network or from ffmpeg. Return errors.
- Platform-neutral code only. No OS media/discovery APIs; `cfg(target_os)` is
  only acceptable for config-directory paths. The exceptions are
  `crates/tvpipe/src/signals.rs` (Ctrl-C handling, `cfg(unix)`/`cfg(windows)`) and
  `crates/tvpipe/src/util/timer.rs` (Windows 1 ms timer resolution, `cfg(windows)`); see
  "Platform-Specific Code" in `ARCHITECTURE.md`.

## Before committing

`cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`;
with JavaScript changes also `npm run build && npm run check && npm test`
in `npm/party` and `npm run check && npm test` in `games/drawbluff`.
