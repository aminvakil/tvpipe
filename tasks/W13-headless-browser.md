# W13 `tvpipe share URL` (headless Chrome, auto-shared)

Status: done (pending merge). Owns `src/browser/{launch,cdp,headless}.rs`,
`src/browser/headless.js`, `src/session/browse*.rs`, `share URL`, docs.
Built as `tvpipe browser URL`; merged into `share` (owner decision,
"Proposal A"): `share` without URL is the capture page, `share URL` this
mode. URL-only options are refused without a URL; the `--to browser`
player options are shared by every streaming subcommand. Reuses W12 (`capture.js`, share protocol,
`net::websocket` client) and W14 (`--to browser` works as a target too).
How it works: ARCHITECTURE.md "Headless browser", docs/share-protocol.md
("Rejoin mode", "Capture script API").

```
tvpipe share URL --to TV [--to browser] [--size WxH] [--fps N]
              [--profile DIR] [--visible] [--reload MIN] [--browser PATH]
              [--open] [--port PORT] [--wait-for-viewer] [--new-token]
```

- [x] Find a Chromium-based browser: `--browser PATH` (also a name on
      PATH or a macOS `.app`), `$TVPIPE_BROWSER`, then well-known install
      paths per OS (Chrome, Edge, Chromium, Brave, Chrome Canary;
      `browser/platform.rs`) and PATH (`google-chrome`, `chromium`,
      `msedge`, …).
- [x] Launch: `--headless=new` (unless `--visible`) with `--screen-info`
      holding the viewport, the window fitted so the viewport is the
      stream size at scale 1 (measured, then corrected again once
      Chrome's "sharing this tab" bar appears), own profile (temp dir,
      deleted on exit; `--profile DIR` persistent; the browsers' real
      profiles refused), `--auto-accept-this-tab-capture`, autoplay, no
      first-run/default-browser/keychain prompts, no background
      throttling, `--remote-debugging-port=0` + `DevToolsActivePort`.
- [x] CDP client over `net::websocket::connect` (ids, results, errors,
      events, flattened sessions); capture.js + `headless.js` evaluated
      after every navigation as a user gesture; `Page.setBypassCSP`;
      Local Network Access granted (https pages are blocked from
      127.0.0.1 otherwise).
- [x] Session: share's encoded-input path; keyframe requests to the page;
      PCM → AirPlay/browser, AAC → Cast; size from the receivers' shape and
      limits, viewport = stream size (no bars).
- [x] Lifecycle: Ctrl-C / `q` / receivers gone / browser closed; on exit
      Browser.close, then kill, temp profile deleted; a crashed page is
      loaded again, a dead browser started again (≤ 3 per minute);
      navigations re-share (share server rejoin mode); `--reload MIN`;
      console errors, exceptions and log entries at debug; dialogs
      dismissed; 401/403/429 of the page reported; meet.google.com refused.
- [x] Tests: path discovery, URLs, profiles, command line, page states;
      CDP framing against a fake DevTools endpoint; the session with fake
      pages (two documents to fake Cast + AirPlay, sizing, failures);
      opt-in real Chrome (`TVPIPE_TEST_CHROME=...`):
      `chrome_browser_to_cast_and_player` (clock + tone page, reloads, fake
      Cast and the `--to browser` player in a second Chrome, ffprobe
      decode, no leftovers) and `chrome_browser_survives_crashes`.
      W12's Chrome helper now runs on `browser::launch`.

Measured (Chrome 157, Apple Silicon, `-v`): browser start to first
capture ~1.0 s, to the first frame at the receivers ~2.4 s (encoder
warm-up); a reload or navigation leaves a ~0.55 s gap, a page crash
~0.55 s, a browser crash ~3 s; 1920x1080@30 from github.com ~41 % CPU
for Chrome + tvpipe together (one core = 100 %), 1.4 GB RSS (Chrome).
Capture to tvpipe ~5 ms.

Open:
- The receivers connect before the browser starts (the page's size comes
  from them), so a slow page keeps a connected TV waiting for its first
  frame; starting the browser in parallel and resizing later would hide
  the ~1 s launch.
- `--visible` follows the screen's scaling (device scale 1.5 seen on a
  scaled display), so the capture is scaled to the stream.
- A browser that tvpipe cannot clean up (tvpipe killed with SIGKILL)
  keeps running with its temp profile until the next `tvpipe share URL`,
  which sweeps it: every temp profile holds `tvpipe-owner` (pid, the port
  of a loopback beacon answering a random nonce while its tvpipe runs);
  without an answer the browser on it is quit through the DevTools
  endpoint in its own `DevToolsActivePort` (the path holds that browser
  instance's id) and the directory removed.
- Not tried: Edge, Brave, Chromium, Linux, Windows; real TVs.
