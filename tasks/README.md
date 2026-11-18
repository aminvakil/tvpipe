# Task board

One file per workstream. The owner updates the `Status` line and the
checklist as it lands pieces on `main`.

| Card | Owner | Branch / worktree | Status |
|---|---|---|---|
| [W1 crypto](W1-crypto.md) | subagent `w1-crypto` | `w1-crypto` / `../streamlib-w1` | done |
| [W2 codecs](W2-codecs.md) | subagent `w2-codecs` | `w2-codecs` / `../streamlib-w2` | done |
| [W3 net](W3-net.md) | subagent `w3-net` | `w3-net` / `../streamlib-w3` | done (LG/Samsung/Cast captures pending) |
| [W4 media](W4-media.md) | subagent `w4-media` | `w4-media` / `../streamlib-w4` | done |
| [W5 AirPlay pairing](W5-airplay-pairing.md) | subagent `w5-pairing` | `w5-pairing` / `../streamlib-w5` | done (paired + re-verified on Apple TV) |
| [W6 FairPlay (LGPL port)](W6-fairplay-lgpl.md) | subagent `w6-fairplay` | `w6-fairplay` / `../streamlib-w6` | done |
| [W7 AirPlay mirror](W7-airplay-mirror.md) | subagent `w7-mirror` | `w7-mirror` / `../streamlib-w7` | done for Apple TV; legacy/HKP3 paths loop-tested only (waits for LG/Samsung) |
| [W8 Cast](W8-cast.md) | subagent `w8-cast` | `w8-cast` / `../streamlib-w8` | done on openscreen receiver; real hardware pending |
| Cast play wiring | subagent `play-cast` | merged | done (openscreen receiver) |
| [W9 passthrough + encoded SLRW](W9-passthrough.md) | subagent `w9-passthrough` | `w9-passthrough` / `../streamlib-w9` | done; Apple TV B-frame experiment pending (human) |
| [W10 AAC-LC encoder](W10-aac-encoder.md) | subagent `w10-aac` | `w10-aac` / `../streamlib-w10` | done (not yet wired into Cast) |
| [W11 H.264 encoder](W11-h264-encoder.md) | subagent `w11-h264enc` | `w11-h264enc` / `../streamlib-w11` | in progress |
| M10 replication (shared timeline, one budget) | subagent `multi-receiver` | `multi-receiver` / `../streamlib-multi-receiver` | in review (decision 0010); Office TV + local openscreen receiver tested; AirPlay+Cast mixed on hardware pending |
| [W12 share a browser tab](W12-browser-share.md) | subagent `w12-share` | `w12-share` / `../streamlib-w12` | in progress |
| W13 `tvpipe browser URL` (headless Chrome + CDP) | — | — | after W12 |
| S1 FairPlay spec | — | — | dropped for now (LGPL port, decision 0005) |
| [S2 AirPlay mirroring spec (dirty)](S2-airplay-mirroring-spec.md) | subagent `s2-spec` | `s2-spec` / `../streamlib-s2` | merged (reviewed); Q1-Q6 answered |

## Notes for later workstreams

- W7: ffmpeg's ALAC encoder always emits 4096-sample frames. Plan AirPlay
  audio as `PcmS16le` with `frame_samples = 352` (ffmpeg cuts exact packets)
  and wrap PCM as uncompressed ALAC frames in `airplay/audio.rs`.
- Lead (session.rs): ffprobe the input before planning so inputs without
  audio don't get `0:a:0?` maps (avoids ffmpeg warnings). Done (play-cast).
- Lead: switch `cast/annexb.rs` and the private clock in `cast/sink.rs` to
  `media::h264` / `media::clock` (both on main now). Done (play-cast); the
  Cast sink keeps its own anchoring rule on `media::clock` (early frames
  keep future reference times; ffmpeg delivers in bursts).
- `play --to` forms: `NAME` (discovery), `IP` (Cast on 8009), `IP:PORT`
  (Cast, except 7000 = AirPlay), `cast:…` / `airplay:…` to force one.
  Devices with both protocols go over Cast until AirPlay mirroring lands.
- Ctrl-C: `util::interrupt` (decision 0004). SIGINT/SIGTERM/`q` stop the
  session without draining; a second Ctrl-C exits at once (130).
- `-re` inputs: ffmpeg's 0.5 s initial read burst overruns the Cast
  in-flight window; `media::ffmpeg` now passes `-readrate_initial_burst
  0.001` (0 means "default" in ffmpeg 8).
- W4 follow-up: with SLRW input the game writes ~0.4 s before ffmpeg
  connects; drop stale audio at start so the Cast sink doesn't discard the
  first ~36 audio frames.
