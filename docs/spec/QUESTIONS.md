# Questions for the spec writers

Clean implementers file questions here; spec writers answer by amending the
spec and linking the section. Never look in tainted sources yourself.

(none yet)

## W7 (airplay-mirroring.md), 2026-11-18

Non-blocking; W7 picked the behaviour in brackets and the Apple TV
accepted it at protocol level.

1. §4.5/§4.6/§4.7: does any receiver time out between the audio SETUP
   (#2) and the video SETUP (#3), or between SETUP #3 and the first codec
   packet / first `/feedback`? tvpipe must start ffmpeg after it knows
   the canvas, so it sends SETUP #2 in its connect phase and SETUP #3 when
   the encoder's first output is ready (gap ~0.1 s here, up to ~1–2 s with
   slow inputs). [Gap allowed; `/feedback` starts right after SETUP #3.]

   **Answer (S2):** fine on the dev Apple TV. Tested there: 5 s between
   SETUP #2 and #3 without `/feedback`, and 10 s between setup and the
   first codec packet with `/feedback` every 2 s; both streamed normally.
   Keep starting `/feedback` right after SETUP #3. Other receivers
   untested. Spec: §4.7 ("Gaps").
2. §8.5: is the first sync packet sent before or after the first RTP
   packet, and does it matter? [Sent just before the first RTP packet.]

   **Answer (S2):** before, as you do (the reference does the same; the
   Apple TV accepted it). Spec: §8.5.
3. §8.5: "rtp_now at the sampled local instant t": is sampling `t` = the
   capture instant of the frame just sent (so `rtp_now` = that frame's RTP
   timestamp, no extrapolation) equivalent to what the reference does?
   [Yes, assumed: same sample↔time mapping.]

   **Answer (S2):** yes, equivalent, provided the frames' capture instants
   are derived from their sample positions (true for ffmpeg/SLRW
   timestamps). With jittery wall-clock capture instants the announcements
   would carry the jitter; the reference samples t = now and extrapolates
   rtp_now instead. Compute T from the current clock mapping when building
   the packet. Spec: §8.5.

## W7 legacy profile (Appendix A, §5), 2026-11-18

4. Appendix A: do raw `/pair-setup` and `/pair-verify` requests carry
   `X-Apple-HKP: 3`? The header lists for raw requests do not mention it,
   while the first paragraph says such receivers "use `X-Apple-HKP: 3`".
   [Not sent.]

   **Answer (S2):** correct, not sent. The raw requests carry no
   `X-Apple-HKP`; modelled legacy receivers treat its absence as type 3.
   Only TLV8 pairing requests to such receivers use `X-Apple-HKP: 3`.
   Spec: Appendix A (first paragraph).
5. Appendix A: "try TLV8 transient pair-setup with HKP 3": tvpipe's
   existing transient pair-setup sends `X-Apple-HKP: 4`. Which is right
   for third-party receivers? [Unchanged: the HAP fallback is W5's
   `pair::connect`.]

   **Answer (S2):** both, depending on the receiver: HKP 4 for Apple
   receivers (CoreUtils bits, no bit 26/51), HKP 3 for legacy-preferring
   receivers (no CoreUtils bits, or bit 26/51, e.g. LG), on the transient
   pair-setup and the pair-verify after it. It matters for LG-like TVs (the
   model rejects non-3). The reference retries the other value once on an
   early rejection. Also: the reference uses an empty SRP password for
   transient pairing, tvpipe uses `3939`; unverified which real
   third-party receivers want. Spec: Appendix A ("TLV8 transient
   pair-setup").
6. §5.0/§5.2/§5.3: legacy profile **without** FairPlay (feature 14
   absent): is video sent in the clear, or AES-CTR keyed from the random
   `shk` we put in the video SETUP? [Session start fails with an error.]

   **Answer (S2):** the reference sends random `shk`/`shiv` and AES-CTR
   video keyed from that random `shk` (§5.2 derivation), with audio in the
   clear; untested on any receiver, so failing is acceptable for now. It
   does not apply to LG-like receivers that pair over HAP: their control
   channel is encrypted, so they use ChaCha video/audio (§5.0 HAP column)
   with or without FairPlay. Spec: §5.2 ("Legacy profile without
   FairPlay"), §5.3.
