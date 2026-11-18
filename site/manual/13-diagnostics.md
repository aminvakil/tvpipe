---
title: Diagnostics
summary: Logs, traces and common problems.
group: Reference
spec:
  Flags: `-v`, `--debug`
  Command: `debug-pipeline`
---

What to run when something does not show up, stutters or is out of sync.

## Logs and traces

:::aside Output
Logs go to standard error. In `play`, standard output is reserved for keyframe requests to games.
:::

`-v` prints debug output: the chosen plan, the encoder, passthrough decisions and receiver reports. `--debug` adds protocol traces, which are very verbose.

```sh
$ tvpipe -v test-pattern --to TV
plan: 1280x720@30 2764 kbit/s, gop 15, latency 100 ms (built-in encoder)
```

## Checking an input

`debug-pipeline` runs ffmpeg and the demuxer without a receiver and prints every frame. `--raw` skips the encoding plan and shows what ffmpeg produces by default.

```sh
$ tvpipe debug-pipeline movie.mp4
```

## Common problems

| Symptom | Cause and remedy |
|---|---|
| No receivers found | The computer and TV are on different networks, or a firewall blocks mDNS. Try `--interface`, or give `--to` an address. |
| No sound in a browser tab | Browsers start audio only after a click. Click the page once. |
| Drops on Cast over Wi-Fi | Raise `--latency`. |
| A site does not load with `share URL` | It refuses automated browsers. See [limitations](07-web-page.md#s4). |
