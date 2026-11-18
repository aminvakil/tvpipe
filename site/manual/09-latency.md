---
title: Test pattern and latency
summary: Measure the delay and tune it.
group: Use
spec:
  Command: `test-pattern`
  Option: `--latency MS`
  Needs: Nothing
---

The test pattern shows how far the TV runs behind the source. `--latency` trades that delay against robustness on busy networks.

## The test pattern

:::aside Built in
The pattern is generated and encoded inside tvpipe; ffmpeg is not needed. Naming an ffmpeg encoder (`--encoder x264`) streams ffmpeg’s `testsrc2` instead.
:::

```sh
$ tvpipe test-pattern --to TV
```

![The test pattern: colour bars, a UTC clock, frame number and media time](shots/test-pattern.png "Colour bars, the UTC time in milliseconds at which the frame was made, the frame number and the media time. A white square flashes and a beep sounds once a second.")

## Measuring glass to glass

:::aside Measured
**250–270 ms.** Apple TV 4K, Wi-Fi, built-in encoder, TV in its normal picture mode.
:::

1. Stream the test pattern to the TV.
2. Put a clock with milliseconds next to it, for example the sender’s own screen.
3. Photograph both. The difference between the two clocks is the delay.

Phones stamp photos with a delay of their own. Calibrate by photographing a millisecond clock on the computer’s screen alone, and subtract that offset.

## Playout delay

:::aside Range
5 to 2000 ms. The same value applies to every receiver in the session.
:::

Every frame carries the time at which receivers should show it. `--latency` sets how far behind the source that is.

| Receivers | Default |
|---|---|
| AirPlay | Automatic: video 100 ms, audio 170 ms |
| Cast | 300 ms |
| AirPlay and Cast | 300 ms |
| Browser only | 100 ms |

Cast defaults to 300 ms because Wi-Fi feedback gaps of around 220 ms made lower values drop frames on a Google TV. On a wired or quiet network you can go lower.
