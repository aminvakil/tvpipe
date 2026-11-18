---
title: Sharing a tab or screen
summary: Capture with the browser, no ffmpeg needed.
group: Use
spec:
  Command: `share`
  Browsers: Chrome, Edge; Safari 16.4+; Firefox 130+
---

`tvpipe share` serves a page that captures a browser tab, a window or the whole screen and sends it to the TV. The browser does the capture and the encoding.

## Sharing

:::aside Local only
The page is served on `127.0.0.1` with a token in the URL. Other machines cannot open it.
:::

```sh
$ tvpipe share --to TV --open
```

1. `--open` opens the page in your default browser. Without it, tvpipe prints the URL.
2. Press **Share…** and pick a tab, a window or a screen.
3. For sound, tick “Share audio” in Chrome’s picker.

![The share page: its status, the Share and Stop buttons and the folded Details](shots/share-page.png "The share page, ready. The status on the left follows the session: ready, connecting to the TV, streaming, ended.")

## Picture and sound

:::aside Encoding
The page encodes H.264 with WebCodecs, on the browser’s hardware encoder where there is one.
:::

The stream takes the TV’s screen shape. The capture is centred with black bars, at its own size up to what the TV allows. A tab with a different shape is not stretched.

“Keep tab audio playing here” keeps the sound on the computer as well; browsers otherwise mute a tab while its audio is captured.

## Details

:::aside For experts
The details remember whether they were open.
:::

**Details** at the bottom of the page opens what the page knows about the stream: the size of the capture, the stream tvpipe asked for, the encoder and whether it runs in a worker, the frame rate, the bitrate, the encode delay, dropped frames, and the log. Errors also show in the page’s message, so the details only matter when looking for the cause.

## Stopping

Stopping the share in the browser, closing the page or pressing `Ctrl-C` ends the session, and the TV leaves mirroring mode.
