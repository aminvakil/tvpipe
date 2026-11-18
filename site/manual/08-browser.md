---
title: Playing to a browser
summary: Previews, OBS and video calls.
group: Use
spec:
  Command: `preview`
  Receiver: `--to browser`
---

A browser tab on your computer can be a receiver, next to a TV or on its own.

## Previewing

:::aside Shorthand
`preview` is `play --to browser`.
:::

```sh
$ tvpipe preview --open movie.mp4

# a tab next to a TV
$ tvpipe play --to "Living Room" --to browser movie.mp4
```

`--open` opens the player page. Without it, tvpipe prints the address. Any number of tabs can watch, and a tab opened later shows the same picture as the others. `--wait-for-viewer` starts the stream once a tab is open.

## The player page

:::aside Keys
`F` or double-click: fullscreen

`S`: statistics
:::

:::aside Sound
Browsers only play sound after a click. Click the page once.
:::

![The player page showing a picture](shots/test-pattern.png "The player page.")

The player works in Chrome, Edge, Safari 16.4+ and Firefox 130+.

## OBS

:::aside Address
Port 8765, or `--port`. If the port is taken, tvpipe uses another one for that run.
:::

Add the player address as a browser source and append `&ui=0` to hide the controls. Turn on “Control audio via OBS” to get the sound into OBS.

The address stays the same between runs, so the source keeps working the next time tvpipe plays. `--new-token` makes a new address, and the old one stops working.

## With other commands

The player options `--open`, `--port`, `--wait-for-viewer` and `--new-token` work with `play`, `preview`, `share` and `test-pattern`, whenever one of the receivers is `--to browser`. `share --open` without a URL opens the share page instead.

## Video calls

Open the player page and present that tab in Meet, Zoom or Teams, with its audio.
