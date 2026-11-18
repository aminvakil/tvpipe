---
title: tvpipe: low-latency mirroring to AirPlay and Google Cast
source: https://github.com/mitsuhiko/tvpipe
---

<h1><svg viewBox="0 0 560 520" aria-hidden="true"><use href="#logo"/></svg><span>tv<b>pipe</b></span></h1>

A command line tool that mirrors video and audio to AirPlay and Google Cast receivers, a quarter of a second behind the source. Files, capture devices, browser tabs, web pages and games, to one TV or several at once, from a single binary on macOS, Linux and Windows.

:::status Experimental
tvpipe is under active development. Expect bugs, and changes to commands and options.
:::

:::buttons
[Read the manual](manual/01-overview.md)
[Source on GitHub](https://github.com/mitsuhiko/tvpipe)
:::

```sh
$ cargo build --release
$ tvpipe discover
NAME          ADDRESS       PROTOCOLS             MODEL
Living Room   192.168.1.20  airplay(mirror),raop  AppleTV6,2
Kitchen       192.168.1.31  cast                  Chromecast
$ tvpipe pair "Living Room"
$ tvpipe play --to "Living Room" --to Kitchen movie.mp4
```

:::features
### Google Cast
Chromecast, Google TV and TVs with Cast built in, through Cast Streaming.

### AirPlay
Apple TV and AirPlay TVs, through AirPlay screen mirroring.

### Browsers
A browser tab as a receiver, in sync with the TVs. Also as an OBS source.

### Linux, Windows, macOS
One binary, the same code on every platform.

### Videos and websites
Files, URLs and capture devices, or any web page or browser tab.
:::

![The tvpipe test pattern: colour bars, a UTC clock with milliseconds and a frame counter](shots/test-pattern.png "`tvpipe test-pattern` burns the time each frame was made into the picture. On an Apple TV 4K over Wi-Fi the picture runs about 260 ms behind that clock. [How it is measured](manual/09-latency.md)."){.crt}
