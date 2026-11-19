# beam

A demo of playshare's surfaces (docs/playshare-design.md §17): a WebGL
picture (an animated shader field, 400 spinning triangles, a beep a
second, and the wall clock as a row of blocks) put on the TVs either as a
**surface** (encoded in the host page with WebCodecs: drawn per animation
frame, on a timer, or in a worker with an `OffscreenCanvas`; optionally a
second canvas to the same TVs) or as a page tvpipe's own browser
captures (`tv.html`, the tab path). Plain JavaScript, no build:

```sh
tvpipe playshare games/beam
```

`node games/beam/measure.ts [--secs N] [--only NAME,…] [--headed]`
compares the two paths through the `browser` receiver: glass-to-glass
latency read back from the player page, the frames that arrive, CPU of
tvpipe (with its browser) and of the window, tvpipe's numbers for
surfaces; `--headed` adds a minimized window (with and without Chrome's
anti-throttling switches), Firefox (headless) and Safari (opens a
window). Results: docs/playshare-design.md §17.2.
