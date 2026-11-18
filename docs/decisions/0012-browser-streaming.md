# 0012: Streaming a browser

Status: accepted by the human (2026-11-18): option C first, then A.

- **C: "share a tab"** (`tvpipe share`, name open): tvpipe serves a small
  page on 127.0.0.1; the user opens it in any browser with WebCodecs and
  shares a tab/window/screen. The page encodes H.264 with `VideoEncoder`
  (AVCC + avcC, `latencyMode: "realtime"`, no B-frames) and sends
  encoded chunks plus PCM audio (AudioWorklet) over a localhost WebSocket.
  tvpipe feeds them into the same path as encoded SLRW game input
  (decision 0008): passthrough video, PCM for AirPlay, `media::aacenc` for
  Cast, keyframe requests back to the page (`keyFrame: true`).
- **A: `tvpipe browser URL`**: launch an installed Chromium-based browser
  (Chrome/Edge/Chromium; `--browser PATH`), drive it over the DevTools
  protocol, inject the same capture script into the target page
  (`getDisplayMedia({preferCurrentTab: true})` auto-accepted by flag),
  fixed viewport = stream size; headless by default.
- B (CDP screencast + JPEG) only if A's tab capture turns out unusable.

No new crates: WebSocket (RFC 6455, client + server), the HTTP server bits,
base64 and the CDP JSON-RPC client are hand-rolled. The capture page is
static HTML/JS embedded in the binary (`include_str!`). Locating a browser
uses well-known install paths per OS; that is a config-path-like use of
`cfg(target_os)` and allowed in the same file only.

Sources allowed: RFC 6455, W3C WebCodecs / Media Capture / Screen Capture
specs, MDN, Chrome DevTools Protocol docs (public), RFC 4648.
