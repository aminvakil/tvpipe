# Questions for the spec writers

Clean implementers file questions here; spec writers answer by amending the
spec and linking the section. Never look in tainted sources yourself.

(none yet)

## W7 (airplay-mirroring.md), 2026-11-18

Non-blocking; W7 picked the behaviour in brackets and the Apple TV
accepted it at protocol level.

1. §4.5/§4.6/§4.7: does any receiver time out between the audio SETUP
   (#2) and the video SETUP (#3), or between SETUP #3 and the first codec
   packet / first `/feedback`? streamlib must start ffmpeg after it knows
   the canvas, so it sends SETUP #2 in its connect phase and SETUP #3 when
   the encoder's first output is ready (gap ~0.1 s here, up to ~1–2 s with
   slow inputs). [Gap allowed; `/feedback` starts right after SETUP #3.]
2. §8.5: is the first sync packet sent before or after the first RTP
   packet, and does it matter? [Sent just before the first RTP packet.]
3. §8.5: "rtp_now at the sampled local instant t": is sampling `t` = the
   capture instant of the frame just sent (so `rtp_now` = that frame's RTP
   timestamp, no extrapolation) equivalent to what the reference does?
   [Yes, assumed: same sample↔time mapping.]
