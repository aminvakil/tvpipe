---
title: Playing media
summary: Files, URLs, cameras and the screen.
group: Use
spec:
  Command: `play`
  Needs: ffmpeg
---

`tvpipe play` plays a file or a URL on the TV, or mirrors a camera or the screen.

## Files and URLs

```sh
$ tvpipe play --to TV movie.mkv
$ tvpipe play --to TV --loop intro.mp4
$ tvpipe play --to TV https://example.com/stream.m3u8
```

`--loop` starts over at the end. `--no-audio` sends only the picture.

tvpipe reads files at playback speed. A live source already arrives at that speed, and `--no-realtime` turns the pacing off for it.

## Screens and cameras

:::aside Tip
For a single window or browser tab, [share](share.md) is simpler.
:::

Everything after `--` goes to ffmpeg as its input, so anything ffmpeg can capture works.

| Platform | Mirror the screen |
|---|---|
| macOS | `tvpipe play --to TV -- -f avfoundation -i 1:0` |
| Linux | `tvpipe play --to TV -- -f x11grab -i :0.0` |
| Windows | `tvpipe play --to TV -- -f gdigrab -i desktop` |

On macOS, `ffmpeg -f avfoundation -list_devices true -i ""` lists the screens and cameras.

## Size and quality

```sh
$ tvpipe play --to TV --size 1280x720 --fps 30 --bitrate 4000 movie.mp4
```

By default tvpipe picks what the input and the TV support. These options set limits, they never make the picture larger. All options are in [Options](options.md).
