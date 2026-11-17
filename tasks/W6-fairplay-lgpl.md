# W6 FairPlay (LGPL port of doubletake)

Status: done (m3/m4 accepted by the Apple TV; ekey checked only at SETUP, W7). Ported from doubletake be27776b. Owns `src/airplay/fairplay/**`,
`tests/vectors/fairplay/**`, `LICENSES/LGPL-3.0-or-later.txt` (and the GPL
text it references, if needed). See decision 0005.

- [x] Find doubletake's FairPlay SAP code (Go) and the stream-key derivation
      in `~/.cache/checkouts/github.com/omarroth/doubletake`. Check its
      exact license (LGPL-3.0-or-later per PLAN) and record the commit hash.
- [x] Port to Rust behind PLAN §7.3's API:
      `FpSession::new(rng)`, `m1() -> [u8;16]`, `m3(&mut self, m2) ->
      Result<[u8;164]>`, `check_m4(&self, m4) -> Result<()>`,
      `wrap_key(&self, key, rng) -> Result<[u8;72]>`, plus a stream-key
      derivation function (FairPlay key + pair-verify shared secret +
      `streamConnectionID` → AES key/IV), with a doc comment saying exactly
      what W7 must pass in. Use `crypto::{aes, modes, ring}` for standard
      primitives (SHA-*, HMAC, AES); port the non-standard parts (white-box
      tables, modified hash, mode ciphers). Tables go in separate files;
      `include_bytes!` from `src/airplay/fairplay/data/` is fine.
- [x] Every file: `// SPDX-License-Identifier: LGPL-3.0-or-later` and an
      attribution line to doubletake (path + commit).
- [x] Known-answer vectors in `tests/vectors/fairplay/`, from doubletake's
      own tests and/or by running doubletake's Go code (write a tiny Go
      harness OUTSIDE the repo, e.g. `~/.cache/streamlib-oracle/`) and a
      differential fuzz test (`#[ignore]`, runs the harness if present).
- [x] Hardware: run m1→m2→m3→m4 against the Apple TV (192.168.1.34:7000,
      already paired; credentials in the config dir, use
      `airplay::pairing`/`pair::connect` to get a verified channel) via
      `POST /fp-setup` with `X-Apple-ET: 32`. An `#[ignore]` test
      `hardware_fp_setup`.
