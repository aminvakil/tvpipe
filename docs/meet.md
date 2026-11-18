# Presenting tvpipe into video calls

Research from 2026-11-18. The question was whether tvpipe can present its
stream to a Google Meet room device: Meet hardware has no local Cast
receiver and only takes presentations through meet.google.com.

**The decision (owner, 2026-11-18):** tvpipe does not join meetings by
itself. It gets a generic `--to browser` receiver instead: a local player
page that plays tvpipe's stream in a browser tab. Presenting that tab into
Meet, Zoom or Teams is one use of it. `tvpipe preview SOURCE` uses the same
page. Below are the findings behind the decision, the manual flow, and the
technical notes for the player page.

## How presenting to Meet works

Sources: [Present to a video meeting (Meet hardware)][hw-present],
[Use the Meet touchscreen][hw-touch], [Join a meeting (Meet hardware)][hw-join],
[Present during a video meeting][meet-present], [Companion mode][companion],
[Connect room][connect-room].

- **No local path into Meet hardware.** A room device takes a
  presentation only as a participant of a meeting on meet.google.com.
  Laptops present through the meeting, not to the device over the LAN.
  The only non-network input is HDMI into the touchscreen (Poly and
  Logitech kits), which presents locally without a meeting [hw-touch].
- **A room always needs a meeting.** The hardware FAQ "Can I use the room
  to do a presentation, even if I'm not using it for a video meeting?"
  answers: *"Just start a new meeting in Google Meet on the Chromebox
  device in the room and present to it from your laptop."* [hw-present]
  On the touchscreen, that is **Enter a code or nickname → Join** without
  a code. This starts an ad-hoc meeting and shows its code. Such codes
  expire when everyone has left [hw-touch].
- **Presenting from a laptop.** Go to meet.google.com, pick the meeting or
  enter its code (the room's display shows it), click **Present**, choose
  a tab, window or screen, then **Share** [hw-present].
- **Present = companion mode.** *"When you click Present in the green room
  before you join a meeting, you join in Companion mode … your mic and
  speaker are unavailable."* [meet-present] g.co/present is the short link
  for this flow. Someone presenting in companion mode shows up twice in
  the people list: once as the person and once as the presentation
  [companion].
- **Audio.** A presentation carries audio only from a shared **tab** (with
  "Also share tab audio"), or from a window or screen with "Also share
  system audio" [meet-present].
- **Accounts and admission.** The hardware help says *"Presenters do not
  need a Google Account"* [hw-present]. However:
  - The meeting's host must approve guests who are not signed in before
    they can connect in companion mode [companion].
  - On hardware, *"You must also be approved if you're not signed in to a
    Google Workspace account."* [hw-join]
  - The meeting's access type decides who must ask to join [spaces]:
    - **OPEN:** anyone with the link joins without asking.
    - **TRUSTED:** the host's organization, invitees and dial-in users join
      without asking.
    - **RESTRICTED:** only invitees join without asking.
  - Admins can block external and anonymous participants entirely
    ([admin-access], [edu-anon]). In practice, presenting from a person's
    own signed-in browser in their own organization needs no admission.
  - Signed out, only a meeting code URL works. meet.google.com/new,
    /landing and g.co/present lead to Google sign-in (observed), so a
    guest cannot start a meeting.
- **Connect room** uses ultrasound proximity detection, which a Workspace
  admin must turn on. It brings the room into the call from the laptop's
  green room. It is not a way to present, and it does not work from
  g.co/present [connect-room].
- Third-party systems can join Meet over SIP/H.323 through Pexip. That is
  an admin-configured, paid interop service [interop], far outside
  tvpipe's scope.

## No API sends media into a meeting

- **Meet REST API** (`meet.googleapis.com/v2`): creates and configures
  meeting spaces (`spaces.create`, `accessType`, entry points) and lists
  conferences, participants, recordings and transcripts. It needs OAuth as
  a Google user and carries no media [spaces].
- **Meet Media API** (Developer Preview):
  - It was **receive-only**. The reference clients add only `recvonly`
    audio and video transceivers (`web/internal/meetmediaapiclient_impl.ts`
    in [meet-media-api-samples][mmas]).
  - It is **discontinued**. It *"will not proceed to General Availability
    … Existing participants will retain access until endpoints shut down
    on January 15, 2027."* [meet-dev]
- **Meet add-ons SDK / Embed SDK:** these provide in-meeting panels and an
  embedding preview (whose start guide returns 404 [embed-forum]). Neither
  takes media input.
- Commercial "meeting bot" services drive the Meet web client in a
  browser, because no API exists ([recall-faq], [meetstream]).

## Why tvpipe does not automate Meet

- The Google Terms of Service (effective 2026-07-30) forbid *"bypassing
  our systems or protective measures"* and *"using automated means to
  access content from any of our services in violation of the
  machine-readable instructions on our web pages (for example, robots.txt
  …)"* [tos].
- meet.google.com/robots.txt has `User-agent: *` with `Disallow: /`. Only
  the start and info pages are allowed, so meeting URLs are off-limits to
  robots.
- Meet refuses browsers it recognizes as automated. A Chrome driven over
  the DevTools protocol gets "You can't join this video call" on every
  meeting URL, before the code is even checked (observed with Chrome 157).
  Joining automatically would mean defeating that check. Bot vendors also
  report Google tightening it further ([screenapp-46], 2026-06).
- Even aside from the terms, automation would be fragile:
  - An anonymous guest still waits for someone to admit it.
  - Meet's UI is obfuscated and changes without notice.
  - Locale and consent dialogs differ per user.
  - Meet re-encodes everything anyway.

So tvpipe stays on its side of the line: it plays the stream locally, and
a person presents it with Meet's normal UI.

## The manual flow with `--to browser`

1. Run `tvpipe play SOURCE --to browser` (or `tvpipe preview SOURCE`).
   tvpipe serves the player page on 127.0.0.1 and opens it (or prints its
   URL).
2. In the same browser, signed in as yourself, open the meeting:
   `https://meet.google.com/CODE`, or g.co/present and pick the meeting.
   For a room without a meeting, first start one on the room's
   touchscreen and read the code off its display.
3. Click **Present** (green room, companion mode) or **Present now** (in
   the call). Choose **A tab**, pick the tvpipe player tab, and keep
   **Also share tab audio** on. Then click **Share**.
4. Stop with **Stop presenting**, or by stopping tvpipe. The player page
   then shows that the stream ended.

This works the same in Zoom, Teams and other web or desktop clients that
can share a browser tab or window. Tab sharing is the only way to get
audio into a Meet presentation without system-audio capture.

Expect Meet's latency and quality on top of tvpipe's, because Meet
re-encodes the captured tab (VP8, VP9 or AV1 with simulcast and its own
bitrate ladder). The player should therefore favor a steady picture over
the lowest latency. It also must not show anything besides the video in
the shared tab.

## Technical findings for the player page

These come from a throw-away prototype (Chrome 157, macOS) that is kept
out of the repository.

- **WebCodecs decoding of fed H.264 works.** tvpipe-style SLRW records
  (kind 3: flags byte + one access unit, Annex B with in-band SPS/PPS)
  came over a WebSocket on 127.0.0.1. `VideoDecoder` decoded them as
  `{codec: 'avc1.42E01F', optimizeForLatency: true}` without a
  `description`: 148 of 148 frames at 30 fps. AVCC plus an avcC
  `description` is the other option, matching `h264-config` in
  `docs/share-protocol.md`.
- **Frames into a MediaStream.** `MediaStreamTrackGenerator({kind:
  'video'})` takes decoded `VideoFrame`s directly. It works on the window
  in Chrome; the standard `VideoTrackGenerator` is worker-only. The
  resulting track plays in a `<video>` element and survives a WebRTC send
  (re-encoded at 30 fps in a loopback `RTCPeerConnection`, headless and
  headed). Drawing the frames to a `<canvas>` with `drawImage(frame)` and
  using `canvas.captureStream()` worked as well. For the player page, the
  simplest route is a full-window canvas or `<video>`. Close each
  `VideoFrame` after use, and drop frames while the generator's writer
  has no `desiredSize` left.
- **PCM without an AudioWorklet.** s16le 48 kHz stereo records (10 ms
  each) were converted to `AudioBuffer`s and scheduled back to back with
  `AudioBufferSourceNode.start(t)`. A ~40 ms lead was used, resyncing
  when the lead ran below 10 ms or above 200 ms. Playback was clean: the
  1 kHz test beep arrived at the expected level. An AudioWorklet ring
  buffer is the more precise option, but scheduling buffers needs no
  module loading.
- **Autoplay.** Audio needs a running `AudioContext`. Without a user
  gesture, Chrome keeps the context suspended. The player page should
  start or resume it from its first click (a "Start" button that also
  explains the sharing steps).
- **Local Network Access** only concerns public sites (https origins)
  reaching 127.0.0.1. Chrome blocked such a socket with
  `net::ERR_BLOCKED_BY_LOCAL_NETWORK_ACCESS_CHECKS`. tvpipe's own page is
  served from 127.0.0.1 and talks to its own origin, so this does not
  apply to it.
- **Secure context.** `http://127.0.0.1` is a secure context, so
  WebCodecs and AudioContext work without TLS (as in the share page).
- **Tab capture.** A captured tab is re-rendered and re-encoded by the
  browser. A fixed-size video, centered with black around it in the
  stream's aspect ratio (`browser::layout::place`), avoids scaling
  surprises on the far end.

[hw-present]: https://support.google.com/meethardware/answer/3350117?hl=en
[hw-touch]: https://support.google.com/meethardware/answer/7552840?hl=en
[hw-join]: https://support.google.com/meethardware/answer/3341433?hl=en
[meet-present]: https://support.google.com/meet/answer/9308856?hl=en&co=GENIE.Platform%3DDesktop
[companion]: https://support.google.com/meet/answer/11295507?hl=en&co=GENIE.Platform%3DDesktop
[connect-room]: https://support.google.com/meet/answer/16765739?hl=en
[spaces]: https://developers.google.com/workspace/meet/api/reference/rest/v2/spaces
[admin-access]: https://knowledge.workspace.google.com/admin/meet/manage-meeting-access-settings-for-your-users
[edu-anon]: https://workspaceupdates.googleblog.com/2020/07/anonymous-users-will-be-blocked-from.html
[meet-dev]: https://developers.google.com/workspace/meet
[mmas]: https://github.com/googleworkspace/meet-media-api-samples
[embed-forum]: https://discuss.google.dev/t/meet-embed-sdk-web-current-access-production-availability-and-pricing/408526
[interop]: https://support.google.com/a/answer/9039847?hl=en
[recall-faq]: https://docs.recall.ai/docs/google-meet-faq
[meetstream]: https://docs.meetstream.ai/guides/platforms/google-meet
[screenapp-46]: https://github.com/screenappai/meeting-bot/issues/46
[tos]: https://policies.google.com/terms?hl=en-US
