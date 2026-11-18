---
title: Playing to a browser
summary: Previews, OBS sources and video calls.
group: Use
spec:
  Command: `preview`
  Receiver: `--to browser`
  Port: 8765
---

A browser tab can be a receiver like a TV. tvpipe serves a player page on `127.0.0.1`, and the tab plays in step with any TVs in the same session.

## Previewing

:::aside Shorthand
`preview` is `play --to browser`. Without an input it plays the test pattern.
:::

```sh
$ tvpipe preview --open movie.mp4
$ tvpipe preview --open

# a tab next to a TV, on the same timeline
$ tvpipe play --to "Living Room" --to browser movie.mp4
```

Any number of tabs can watch, and a tab that joins later starts in sync. Without a tab the stream runs anyway; `--wait-for-viewer` holds the start until one is open.

## The player page

:::aside Keys
`F` or double-click: fullscreen.

`S`: statistics.
:::

:::aside Sound
Browsers only start audio after a click. Click the page once.
:::

![The player page showing the test pattern with the statistics overlay](shots/player-stats.png "The player with statistics open: the stream, the delay against its target, dropped frames, the decode queue, audio sync and the wallclock of the frame on screen.")

The page decodes H.264 with WebCodecs and plays PCM with Web Audio, in Chrome, Edge, Safari 16.4+ and Firefox 130+. The controls and the cursor hide when the mouse rests.

## A URL that stays the same

:::aside Specification
`--port PORT` Another port; 0 for any.

`--new-token` A new URL; old ones stop working.
:::

The player URL is the same on every run: port 8765 and a token kept in the [config directory](12-config.md). An open tab or an OBS source picks up the next run by itself. If 8765 is taken, tvpipe warns and uses another port for that run.

## OBS and video calls

### OBS
Add the URL as a browser source with `&ui=0` appended to hide the controls. Turn on “Control audio via OBS” to route the sound into the mix.

### Meet, Zoom, Teams
Present the tab in the call and share its audio with it.
