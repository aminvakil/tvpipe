---
title: ffmpeg and encoding
summary: When tvpipe needs ffmpeg, and checking an input.
group: Development
spec:
  Command: `debug pipeline`
  Flag: `-v`
---

For development: which inputs go through ffmpeg, and how to check what tvpipe reads.

## When ffmpeg runs

| Input | ffmpeg |
|---|---|
| An H.264 file that the receivers can play as it is | No, it is sent unchanged. |
| The test pattern | No, tvpipe’s own encoder. |
| Raw game frames | No with `--encoder builtin` or without ffmpeg. |
| A game’s own H.264 | No. |
| A browser tab or a web page | No, the browser encodes. |
| Anything else | Yes. |

A file is sent unchanged when it is an MP4, MOV or MKV with H.264 video, without B-frames, with a keyframe at least every 4 seconds, at a size every receiver accepts.

`-v` prints which way tvpipe took and why.

## Checking an input

`debug pipeline` reads an input without a receiver and prints every frame. `--raw` shows what ffmpeg delivers before tvpipe’s settings.

```sh
$ tvpipe debug pipeline movie.mp4
```
