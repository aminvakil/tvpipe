# Latency measurements

## 2026-11-18: Apple TV 4K (AppleTV6,2, tvOS 26.6.1) "Livingroom (2)"

Setup: `tvpipe test-pattern --encoder builtin` (built-in H.264,
1280x720@30, 2764 kbit/s), default latency profile ("wireless": video
lead 100 ms, audio 170 ms), Wi-Fi sender (MacBook Pro), TV in its normal
picture mode. Burned-in clock = UTC wallclock at frame generation.

Method: iPhone photos of the TV; latency = photo EXIF time
(`DateTimeOriginal` + `SubSecTimeOriginal`) − clock on the TV.
Calibrated with a photo of a millisecond clock on the sender's own monitor
(`/tmp/streamlib/clock.html`): the iPhone's EXIF time ran **~555 ms late**
relative to the image content.

| | |
|---|---|
| Raw (10 photos) | 682–914 ms, mean 800 ms |
| Phone timestamp offset | −555 ms (one calibration photo) |
| Sender monitor lag | +10–30 ms (estimate) |
| **Glass-to-glass** | **≈ 250–270 ms (±50 ms)** |

Receiver reports during the run: 0 lost, 0 late, 0 glitches.
Sender side: frames leave on schedule (−5..+3 ms) with ≥ 96 ms headroom.

To do: more calibration photos; TV game mode; lower `--latency` (the
spec's normal 85 ms audio lead glitched about once per second on this
Apple TV); the ffmpeg path; Cast hardware.
