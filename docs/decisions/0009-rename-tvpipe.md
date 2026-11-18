# 0009: Rename streamlib → tvpipe

Status: accepted by the human (2026-11-18).

- Crate and binary: `tvpipe`. Environment variables: `TVPIPE_*` (was
  `STREAMLIB_*`).
- Config directory: `…/tvpipe`. On first use, an existing `…/streamlib`
  config directory (with the pairing credentials) is moved over once
  (`airplay::credentials::default_path`). `TVPIPE_CONFIG_DIR` overrides.
- Unchanged on purpose: the SLRW game-input magic and its name; the
  external scratch paths `~/.cache/streamlib-dirty` and
  `~/.cache/streamlib-oracle` (they exist on disk under those names);
  historical worktree names in `tasks/`; captured fixtures and test
  vectors that contain the old name as data.
- Unit tests now use a scratch config directory (`target/test-tmp/`)
  unless `TVPIPE_ATV` is set (hardware tests), so they never touch the
  user's credentials; the `streamlib` move only ever targets the real
  platform directory. Unit tests also cannot browse the real network
  (mDNS) unless `TVPIPE_LIVE_MDNS` or `TVPIPE_ATV` is set.
