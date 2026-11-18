---
title: Playing media
summary: Files, URLs and capture devices with play.
group: Use
spec:
  Command: `play`
  Needs: ffmpeg, except passthrough
---

`tvpipe play` mirrors a file, a URL or any input ffmpeg can open, in real time.

## Files and URLs

:::aside Pacing
Files are read in real time. Live sources should use `--no-realtime` so tvpipe does not pace them a second time.
:::

```sh
$ tvpipe play --to TV movie.mkv
$ tvpipe play --to TV --loop intro.mp4
$ tvpipe play --to TV https://example.com/stream.m3u8
```

Anything ffmpeg can read works: local files, HTTP, HLS. `--loop` starts over at the end; `--no-audio` sends video only.

## Screens and cameras

:::aside Tip
For a single browser tab or window, [share](06-share.md) is simpler than screen capture and needs no ffmpeg.
:::

Everything after `--` is passed to ffmpeg as input arguments, so any capture device ffmpeg supports can be mirrored.

| Platform | Screen capture |
|---|---|
| macOS | `tvpipe play --to TV -- -f avfoundation -i 1:0` |
| Linux | `tvpipe play --to TV -- -f x11grab -i :0.0` |
| Windows | `tvpipe play --to TV -- -f gdigrab -i desktop` |

On macOS, `ffmpeg -f avfoundation -list_devices true -i ""` lists the device numbers.

## Passthrough

:::aside Check
Run with `-v` to see whether a file was passed through and, if not, why.
:::

Some files are sent without re-encoding, which saves CPU and keeps the original quality. A file is passed through when it is:

- an MP4, MOV or MKV with H.264 video,
- without B-frames,
- with a keyframe at least every 4 seconds,
- at a size every receiver accepts.

Everything else is transcoded by ffmpeg.

## Size, frame rate and bitrate

:::aside Defaults
Taken from the input and from what the receivers report they can show.
:::

```sh
$ tvpipe play --to TV --size 1280x720 --fps 30 --bitrate 4000 movie.mp4
```

These are limits for the output, not a request to upscale. All options are listed in [Options and encoders](11-options.md).
