---
title: Overview
summary: What tvpipe is, how it works and what it does not do.
group: Start
spec:
  Document: Operations Manual
  Applies to: tvpipe 0.1
  Platforms: macOS; Linux; Windows
---

tvpipe is a command line tool that mirrors video and audio to AirPlay and Google Cast receivers with low latency. It is one program that works the same on macOS, Linux and Windows.

## What it does

:::aside In short
Every streaming command takes `--to` one or more times. Everything else is optional.
:::

- Plays a video file or URL on the TV in real time, or anything else ffmpeg can read: a camera, a capture card, the screen.
- Shows a browser tab, a window or the whole screen, captured by the browser itself.
- Puts a web page on the TV, such as a dashboard, rendered by a browser of its own.
- Takes raw frames and sound from a game on standard input.
- Sends to several receivers at once from a single encode, including browser tabs on the computer it runs on.

## How it works

:::aside Protocols
**AirPlay.** Screen mirroring with realtime audio, the session an iPhone opens with Screen Mirroring.

**Google Cast.** Cast Streaming: the mirroring receiver app, RTP and RTCP over UDP.
:::

Most tools that cast a video hand the TV a URL and let it download and buffer. tvpipe instead drives the real-time mirroring sessions a phone uses for screen mirroring: it encodes each frame as it arrives and sends it straight to the receiver, which shows it after a fixed playout delay.

:::figure One encode feeds every receiver. Receivers share a playout delay but are not frame-locked to each other.
<svg class="diagram" viewBox="0 0 720 268" role="img" aria-label="Sources flow into tvpipe, which sends to AirPlay, Cast and browser receivers">
<text class="s" x="0" y="12">SOURCES</text><text class="s" x="268" y="12">TVPIPE</text><text class="s" x="520" y="12">RECEIVERS</text>
<rect class="box" x="0.5" y="28.5" width="190" height="44"/><text class="t" x="14" y="55">File, URL, capture</text>
<rect class="box" x="0.5" y="88.5" width="190" height="44"/><text class="t" x="14" y="115">Browser tab or page</text>
<rect class="box" x="0.5" y="148.5" width="190" height="44"/><text class="t" x="14" y="175">Game on stdin</text>
<rect class="box" x="0.5" y="208.5" width="190" height="44"/><text class="t" x="14" y="235">Test pattern</text>
<path class="ln" d="M191 50.5 H229.5 V230.5 H191 M191 110.5 H229.5 M191 170.5 H229.5 M229.5 140.5 H268"/>
<rect class="core" x="268" y="28" width="190" height="225"/><rect class="ac" x="268" y="28" width="4" height="225"/>
<text class="t core-t" x="286" y="58">Read or capture</text><text class="t core-t" x="286" y="80" opacity=".65">ffmpeg when needed</text>
<text class="t core-t" x="286" y="128">Encode once</text><text class="t core-t" x="286" y="150" opacity=".65">H.264, AAC, PCM</text>
<text class="t core-t" x="286" y="198">One clock</text><text class="t core-t" x="286" y="220" opacity=".65">Same playout delay</text>
<path class="ln" d="M458 140.5 H489.5 M489.5 50.5 V230.5 M489.5 50.5 H520 M489.5 140.5 H520 M489.5 230.5 H520"/>
<rect class="box" x="520.5" y="28.5" width="199" height="44"/><text class="t" x="534" y="55">Apple TV, AirPlay TVs</text>
<rect class="box" x="520.5" y="118.5" width="199" height="44"/><text class="t" x="534" y="145">Chromecast, Google TV</text>
<rect class="box" x="520.5" y="208.5" width="199" height="44"/><text class="t" x="534" y="235">Browser tab, OBS</text>
</svg>
:::

tvpipe is written in Rust with three dependencies. Discovery, pairing, the RTP senders and the H.264 and AAC encoders for the test pattern and game input are part of the program. ffmpeg is only started when a source needs decoding or re-encoding.

## What it does not do

:::aside Scope
:::

- Act as a receiver. tvpipe only sends.
- Play a URL on the TV. URL playback is easy to add, but it is not low latency.
- Audio-only targets such as HomePods or Cast speaker groups.

## Sections of this manual

:::index
:::
