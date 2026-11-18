# W13 `tvpipe browser URL` (headless Chrome, auto-shared)

Status: in progress. Owns `src/browser/launch*` / `src/browser/cdp*` (or
similar), the `browser` subcommand, docs. Reuses W12 (`capture.js`, share
protocol, `net::websocket` client) and W14 (`--to browser` works as a
target too). Decision 0012 option A.

```
tvpipe browser URL --to TV [--to browser] [--size WxH] [--fps N]
              [--profile DIR] [--visible] [--reload MIN] [--browser PATH]
```

- [ ] Find a Chromium-based browser: `--browser PATH`, `$TVPIPE_BROWSER`,
      then well-known install paths per OS (Chrome, Chrome Canary, Edge,
      Chromium, Brave; `cfg(target_os)` allowed for these paths only, like
      config dirs) and PATH (`google-chrome`, `chromium`, `msedge`, …).
- [ ] Launch: `--headless=new` (unless `--visible`), window size = stream
      size at device scale 1, own profile (temp dir by default, deleted on
      exit; `--profile DIR` persistent for logins), flags needed for silent
      tab capture with audio (`--auto-accept-this-tab-capture`, autoplay
      policy, no first-run/default-browser prompts, no throttling of the
      captured tab), `--remote-debugging-port=0` + read DevToolsActivePort.
      Never touch the user's real browser profile.
- [ ] Minimal CDP client over `net::websocket::connect` (JSON-RPC with ids,
      events): Target/Page/Runtime as needed: open URL, inject `capture.js`
      on every navigation (`Page.addScriptToEvaluateOnNewDocument` or
      `Runtime.evaluate` after load), start the capture pointed at
      tvpipe's loopback ws URL with a token, bypass CSP if needed
      (`Page.setBypassCSP`) so the script can connect to 127.0.0.1.
- [ ] Session: same encoded-input path as `share`; keyframe requests to the
      page; audio PCM → AirPlay/browser, AAC → Cast. Stream size from the
      receivers (W12's sizing/aspect logic), browser viewport set to it
      (no bars).
- [ ] Lifecycle: run until Ctrl-C / `q` / all receivers gone; on exit close
      Chrome (Browser.close, then kill) and delete the temp profile;
      restart Chrome and re-share if it crashes or the page dies;
      `--reload MIN` reloads periodically; log page console errors at debug.
- [ ] Tests: unit tests for path discovery and CDP framing (fake CDP
      server over loopback WebSocket); opt-in real-Chrome test
      (`TVPIPE_TEST_CHROME=1`) that streams a local test page with a moving
      clock and a tone into the fake Cast receiver and the `--to browser`
      player and checks decode, fps, audio, clean shutdown (no leftover
      processes, profile dir removed).
