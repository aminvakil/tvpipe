# W12 Share a browser tab (option C of decision 0012)

Status: in progress. Owns `src/net/{websocket,httpd,base64}.rs` (or a
`src/web/` module), `src/browser/**` (share mode), the embedded page
(`src/browser/share.html`/`.js`), CLI `share` subcommand wiring.

- [ ] base64 (RFC 4648) with KATs; WebSocket server AND client (RFC 6455:
      handshake with SHA-1 via crypto::ring, framing, masking, fragmentation,
      ping/pong, close; size limits; never panics) with RFC/known vectors and
      a loopback test. The client is for W13 (CDP), so make it usable.
- [ ] Tiny HTTP server on 127.0.0.1 (net::http framing): serves the page and
      upgrades /ws. Bind to loopback only; print the URL; optional `--port`.
- [ ] The page: buttons to share (getDisplayMedia, with audio when offered),
      VideoEncoder H.264 (avc format → AVCC + avcC description), realtime,
      bitrate/size/fps from tvpipe's plan (sent over the socket), keyframe on
      request, scaling to the plan size if needed (VideoFrame from canvas or
      MediaStreamTrackProcessor where available); audio via AudioWorklet →
      s16le PCM at the plan rate; a small status display (connected,
      receivers, fps, bitrate). No external resources (no CDNs).
- [ ] Wire protocol (document it in the decision note or a short spec):
      binary messages for video chunks (pts µs, key flag, data), audio
      (pts µs, PCM), the decoder config; JSON text messages for control
      (plan, keyframe request, stats, stop).
- [ ] `media::input` source for it, reusing the encoded-SLRW state (W9) so
      AirPlay gets PCM and Cast gets aacenc AAC; stop when the page closes or
      the share ends; Ctrl-C stops everything.
- [ ] CLI: `tvpipe share --to ...` (plus the usual size/fps/bitrate/latency
      options); opens nothing by itself, just prints the URL (optionally
      `--open` using the OS's opener command: `open`/`xdg-open`/`start`).
- [ ] Tests: loopback WebSocket + a fake page client in Rust feeding a
      recorded H.264/PCM stream into a fake receiver; manual test in Chrome
      and Safari on this Mac to the Office/cafe TV (ask the lead first).
