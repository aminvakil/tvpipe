---
title: Sharing a tab or screen
summary: A browser tab, a window or the whole screen.
group: Use
spec:
  Command: `share`
  Browsers: Chrome, Edge; Safari 16.4+; Firefox 130+
---

`tvpipe share` opens a page in your browser that shares a tab, a window or the whole screen with the TV.

## Sharing

:::aside Private
The page only opens on this computer.
:::

```sh
$ tvpipe share --to TV --open
```

1. `--open` opens the page. Without it, tvpipe prints the address.
2. Press **Share…** and pick a tab, a window or a screen.
3. For sound in Chrome, tick “Share audio” in the picker.

![The share page with its status and the Share and Stop buttons](shots/share-page.png "The share page, ready to share.")

## Picture and sound

The picture keeps its shape. If it does not fill the TV, tvpipe adds black bars.

The browser mutes a tab while it shares its sound. “Keep tab audio playing here” keeps it audible on the computer too.

Safari and Firefox share windows and screens, but not single tabs, and no sound. To show one tab, share the browser's window and switch it to that tab: the share page keeps streaming in the background. Or drag the tab into a window of its own and share that window.

For a web page with its sound in Firefox, let tvpipe show it in a Firefox of its own: `tvpipe share URL --browser firefox` ([A web page on the TV](07-web-page.md)).

## Stopping

Stop sharing in the browser, close the page or press `Ctrl-C`.
