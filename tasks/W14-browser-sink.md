# W14 `--to browser` receiver and `tvpipe preview`

Status: done (see docs/player-protocol.md; open questions at the end). Owns `src/browser/player*` (Rust side + embedded
page), the `browser` target kind in session target resolution, CLI
`preview`, docs.

A receiver like a TV: `--to browser` starts a loopback HTTP+WebSocket
server (reuse `net::httpd`, `net::websocket`, the `ShareServer` security
pattern: random `?t=` token, loopback-only Host check), prints the URL
and plays tvpipe's stream in any browser tab.

- [x] `BrowserSink: types::Sink`: capabilities (any size up to e.g.
      3840x2160@60, H.264, audio PCM s16le 48 kHz stereo or the session's
      family; pick a sensible `AudioFamily`/format so no extra encode is
      needed: PCM is simplest), display aspect unknown (= follow source).
      Several tabs may connect; a tab joining mid-stream gets the decoder
      config and waits for the next keyframe (request one via
      `wants_keyframe`). No tab connected: drop frames (don't block), keep
      the session alive; optional `--wait-for-viewer`.
- [x] Wire format tvpipe → page: one record per binary WebSocket message
      (same framing as the share protocol, decision 0008/0012), JSON control
      (config: codec string + avcC, size, audio format, playout delay).
- [x] Player page (embedded, no external resources): WebCodecs
      `VideoDecoder` (avc description) → canvas (fit, black bars, fullscreen
      button, cursor hiding), audio via AudioWorklet/Web Audio from PCM,
      A/V sync on the shared media clock with the session's playout delay
      (`--latency`), stats overlay (toggle), autoplay handling (click to
      start audio), reconnect on drop. Must work in Chrome, Safari,
      Firefox. Usable as an OBS browser source and for presenting into
      video calls (docs/meet.md).
- [x] Session: `--to browser` combinable with AirPlay/Cast targets (shared
      timeline, decision 0010); `tvpipe preview SOURCE` = play/test-pattern
      with only a browser target (and `--open` to launch the OS opener).
- [x] Tests: loopback test with a Rust WebSocket client checking config,
      keyframe-first delivery for late joiners, record framing; opt-in
      headless-Chrome test that the page decodes N frames without errors
      and plays audio (like W12's opt-in tests).

## Open questions

- A stable page URL across runs (a fixed `--port` gets a new token each
  run, so an OBS source or an open tab cannot reconnect after tvpipe
  restarts): a `--token` option or a token kept in the config directory?
- Passthrough files with AAC audio are re-encoded when a browser listens
  (the browser takes the AirPlay family's PCM). Decoding AAC in the page
  (`AudioDecoder`) would need a second audio family or a per-sink format.
- OBS and real Meet/Zoom/Teams presenting are not tried yet (only
  headless Chrome, Safari and headless Firefox on macOS).
