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

`--open` opens the player page. Without it, tvpipe prints the address. Any number of tabs can watch.

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

Add the player address as a browser source and append `&ui=0` to hide the controls. Turn on “Control audio via OBS” to get the sound into OBS.

## Video calls

Open the player page and present that tab in Meet, Zoom or Teams, with its audio.
