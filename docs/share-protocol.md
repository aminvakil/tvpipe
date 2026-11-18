# Share page protocol (`tvpipe share`)

The protocol between tvpipe and a capture script (`src/browser/capture.js`)
running in a browser: tvpipe's share page (`tvpipe share`), or the page
tvpipe's own browser shows and injects the script into (`tvpipe share URL`).
Version 1. Using them: [Sharing a tab or screen](https://mitsuhiko.github.io/tvpipe/manual/share/) and
[A web page on the TV](https://mitsuhiko.github.io/tvpipe/manual/web-page/).

## Transport

- tvpipe listens on `127.0.0.1` only, on any free port (the URL has a
  new token every run anyway; `--port` is the `--to browser` player's),
  and prints `http://127.0.0.1:PORT/?t=TOKEN`. `TOKEN` is 16 random bytes
  in hex, new for every run.
- `GET /?t=TOKEN`: the share page. `GET /capture.js`: the capture script
  (static, no token). `GET /fonts/NAME.woff2`: the page's fonts
  (`src/browser/fonts/`, static, no token). `GET /ws?t=TOKEN`: the
  WebSocket (RFC 6455, no extensions, no subprotocol). Every request
  needs a `Host` of `127.0.0.1:PORT`, `localhost:PORT` or `[::1]:PORT`
  (421 otherwise: DNS rebinding), the page and the socket need the token
  (403).
- `http://127.0.0.1` is a secure context in Chrome, Safari and Firefox, so
  `getDisplayMedia` and WebCodecs are available without TLS.
- Largest message: 16 MiB.

## Flow

```
page                                    tvpipe
 |-- hello ------------------------------>|
 |<-------------------------------- config |  (once the receivers are resolved)
 |<-------------------------------- status |  (any time)
 |   user picks a surface (getDisplayMedia)
 |-- capture ---------------------------->|  tvpipe connects the receivers
 |<---------------------------------- plan |
 |-- record h264-config, h264, audio ... ->|
 |<------------------------------ keyframe |  (any time, coalesced)
 |-- stats ------------------------------>|  (every 2 s)
 |-- ended ------------------------------>|  share stopped: tvpipe ends
 |<----------------------------------- end |  or tvpipe stopped first
```

Several pages may connect; the first `capture` wins, later ones get
`busy` and are closed (1008). A page that ends (or closes) before the
`plan` frees the slot for another capture. After the plan, the page
closing its socket or sending `ended` ends the session (after one playout
delay, like the end of a file).

**Rejoin mode** (`tvpipe share URL`): the source page is a document
in tvpipe's own browser, and a navigation, a reload or a crash replaces
it. There, the newest page that sends `capture` is the source (nobody
gets `busy`); a source page that goes away mid-stream does not end the
session, and the next one gets the same `plan` (its `content` placed for
its own capture size) and continues the stream. Because `pts_us` is
wall-clock time, the timeline has a gap and no jump. Its first records
are an `h264-config` and a keyframe (the warm-up rule below); an SPS that
differs from the first one in more than its VUI ends the session (Chrome
on macOS changes the VUI's transfer characteristics between its first
keyframes, which is accepted). The session ends when tvpipe gives up on
the browser or the browser was closed.

## Text messages (JSON objects with a `type`)

Page → tvpipe:

| type | fields |
|---|---|
| `hello` | `version` (1), `clock` (the page's media clock, µs), `agent` (user agent), `features` (what the browser has; informational) |
| `capture` | `width`, `height` (of the first frame; the track's settings if none comes within 1.5 s), `fps` (or null), `audio` (bool: an audio track was shared), `surface` (`browser`/`window`/`monitor`), `label` |
| `stats` | `fps`, `sourceFps`, `kbps`, `audioKbps`, `encodeMs`, `dropped`, `repeated`, `keyRequests`, `queue`, `buffered`, `codec`, `width`, `height` (logged at `--debug`) |
| `ended` | `reason` |
| `log` | `level` (`error`/`warn`/`info`), `message` (shown in tvpipe's log) |

tvpipe → page:

| type | fields |
|---|---|
| `config` | `version`, `fps` (what to ask getDisplayMedia for), `audio` (whether to ask for audio), `sampleRate`, `channels` (the PCM tvpipe wants), `receivers` (names) |
| `status` | `state` (`waiting`, `connecting`, `streaming`), `message`, `receivers` |
| `plan` | `width`, `height` (encode size), `content` (`{x, y, width, height}`: where the capture goes, black around it), `fps`, `bitrate` (bit/s), `keyInterval` (frames between IDRs), `audio` (`{sampleRate, channels}` or null: send no audio) |
| `keyframe` | — make the next frame an IDR (soon: a still screen re-encodes its last frame) |
| `busy` | — another page is the source |
| `end` | `reason`: tvpipe stopped (Ctrl-C, receivers gone, an error) |

Unknown types are ignored by both sides.

## Binary messages

Each is exactly one SLRW record ([`slrw.md`](slrw.md)): `u8 kind | u64le
pts_us | u32le len | payload`, `len` = message length − 13.

| kind | payload |
|---|---|
| 4 `h264-config` | `u8 format` (0: avcC from `VideoEncoder` metadata `decoderConfig.description`) + data. Sent before the first chunk and again only when it changes. |
| 3 `h264` | `u8 flags` (bit 0: key, bit 1: Annex B) + one access unit (`EncodedVideoChunk` data; AVCC with the avcC's length size, or Annex B when the encoder gave no avcC) |
| 2 `audio` | s16le interleaved PCM at the plan's rate and channels (~10 ms per record) |
| 0 `end` | empty (accepted; `ended` is preferred) |

Other kinds are an error and end the session. `pts_us` is the page's
media clock (`performance.now()` in µs) for audio and video alike: video
frames are stamped when the page receives them from the capture, audio
records with their arrival time minus their duration. tvpipe takes media
time 0 from the first record it keeps and times audio by its sample count
(resyncing on drift over 50 ms), as for games.

The H.264 must have no B-frames (decode order = presentation order). The
script asks for Constrained Baseline first (then Main, High), at the
smallest level that fits the plan, with `latencyMode: "realtime"`.

## Stream size and shape

Receivers stretch what they get to fill their screen (a projector showed
a 16:10 capture stretched to 16:9). So:

- **Shape:** the stream has the receivers' screen shape: Cast from the
  ANSWER's `display` (`aspectRatio`, else `dimensions`), AirPlay from the
  SETUP canvas; a TV that does not say counts as 16:9. With several
  shapes, the most common wins (a tie: 16:9) and tvpipe warns that the
  others stretch. Browser players (`--to browser`) have no screen shape
  (they fit any picture into their window): next to a TV they take its
  shape, alone the stream keeps the capture's shape (no bars; `tvpipe
  browser`: the page's, 16:9 or `--size`).
- **Size:** the smallest frame of that shape that holds the capture at
  1:1 (no upscaling), shrunk to the tightest receiver limit. Cast is
  offered up to 1920x1080@60 (or `--size`/`--fps`/`--bitrate`), and its
  ANSWER constraints narrow that; AirPlay's limit is its canvas.
  `--size` sets the frame exactly. The frame rate is `--fps` or 30.
- **Picture:** the capture is centered inside at its own aspect ratio
  (`browser::layout::place`, the same arithmetic as `place` in
  `capture.js`, checked against each other by the Chrome test); the page
  draws it into an OffscreenCanvas of the output size. If the capture
  changes size mid-stream (a resized window) the page recomputes the
  rectangle; the stream's size never changes.
- **Pixels:** square. Browsers' H.264 carries no sample aspect ratio (seen
  in Chrome); tvpipe warns if a stream says otherwise.

## Encoder warm-up

The first pictures of a hardware encoder can come out most of a second
after capture (VideoToolbox in Chrome: ~0.9 s). Starting the timeline on
them made Cast's in-flight window overflow at once. Until it has sent a
keyframe, the page discards chunks: non-key ones, and keyframes more than
150 ms late (it asks its encoder for another), for at most 3 s.

## Flow control

tvpipe reads records as fast as the session takes them; when it falls
behind, the socket backs up. The page drops source frames *before* the
encoder while `WebSocket.bufferedAmount` exceeds 4 MiB or the encoder has
more than 2 frames queued, so the stream stays decodable. Audio records
are dropped above the same threshold.

## Browsers

| | Chrome / Edge | Safari 26 (macOS 26) | Firefox |
|---|---|---|---|
| getDisplayMedia offers | tabs, windows, screens | windows, screens (no tabs) | windows, screens |
| Sound | tab audio (system audio on Windows) | none | none |
| Video engine | page (MediaStreamTrackProcessor) | worker (track moved to a worker, MediaStreamTrackProcessor there) | page (`<video>` + VideoFrame) |
| Tested | headless, CDP-driven, real tab capture | WebDriver, synthetic stream (picker needs a human) | not yet |

- The engine runs in a worker whenever the browser can transfer the video
  track to one (`postMessage(track, [track])` and MediaStreamTrackProcessor,
  VideoEncoder, OffscreenCanvas in the worker): a worker is not throttled
  while the share page is a background tab, which it is in Safari when the
  user shares the Safari window and switches to the tab to show. The same
  engine code runs on the page otherwise.
- Safari/Firefox: the page asks for no audio (a refused constraint could
  not be retried without a new click) and hints `displaySurface: "window"`,
  and the page explains: share the Safari window (or the screen), then
  switch to the tab; or drag the tab into its own window first.
- Clocks are `performance.timeOrigin + performance.now()` so page and
  worker timestamps agree.

## Capture script API

`capture.js` defines `globalThis.tvpipeCapture`:

```js
const c = tvpipeCapture.connect(wsUrl, {
  preferCurrentTab: true,          // getDisplayMedia hint (headless mode)
  selfBrowserSurface: 'include',   // default 'exclude' (the share page)
  localAudio: false,               // keep playing tab audio locally
  profiles: ['42E0', '4D40', '6400'],
  worker: true,                    // false: keep the video engine on the page
});
c.on('config' | 'status' | 'plan' | 'capture' | 'stats' | 'log' | 'error' | 'end' | 'close' | 'open', fn);
await c.captureDisplay();  // getDisplayMedia from the config, then share()
await c.share(stream);     // or any MediaStream (canvas.captureStream(), ...)
c.stop(reason);
```

`captureDisplay()` must run with transient user activation (a click; or
CDP `Runtime.evaluate` with `userGesture: true` plus Chrome's
auto-accept flags in the headless mode).

`tvpipe share URL` evaluates `capture.js` and then `src/browser/headless.js`
in the page after every navigation. That one connects with
`preferCurrentTab`, `selfBrowserSurface: 'include'`, no local audio, and
calls `captureDisplay()` once the `config` arrives; its state is in
`globalThis.__tvpipe` (`connecting`, `capturing`, `sharing`, `ended` with
an `error`), which tvpipe polls. The tab must be allowed to reach
`ws://127.0.0.1`: tvpipe bypasses the page's CSP (`Page.setBypassCSP`)
and grants every origin Chrome's Local Network Access permission
(`loopbackNetwork`; without it an https page fails with
`net::ERR_BLOCKED_BY_LOCAL_NETWORK_ACCESS_CHECKS`). On a `file://` page
(an opaque origin) the audio worklet is loaded from a `data:` URL, since
a `blob:` one fails there.
