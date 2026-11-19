# drawbluff

A Drawful-style party game (our own name, prompts and look) on
[`@tvpipe/party`](../../npm/party): 3–8 players, everyone draws a secret
prompt on their phone, the others invent fake titles for each drawing,
everyone votes for the real one. Points for finding the truth (1000), for
the artist per player who found it (1000), and for every player fooled by
your lie (500); round 2 counts double.

It shows the two ways a game can put its main screen on the TV; the rules
(`game/`) and the phone page (`web/phone/`) are the same for both:

| | Main screen | How |
|---|---|---|
| `node main.ts --to TV` | a web page, `web/screen/` | tvpipe's own browser renders it and streams the tab (`share URL`) |
| `node main.ts --to TV --video` | video frames, `screen-video.ts` | drawn into a canvas in Node and streamed (`stream()`) |

Both draw the same screen view (`{ roster, screen, join }`), and drawings
with the same code (`web/phone/strokes.js`) on the phone, the web screen
and the video screen.

## Running it

```sh
cargo build --release                       # the tvpipe binary (or set TVPIPE_BINARY)
(cd npm/tvpipe && npm ci && npm run build)
(cd npm/party && npm ci && npm run build)
cd games/drawbluff && npm install

node main.ts --to "Living Room"             # a TV, the web screen
node main.ts --to "Living Room" --video     # a TV, the video screen
node main.ts --to browser --open            # a browser tab instead of a TV
node main.ts                                # no TV: open the printed screen link (HDMI)
node main.ts --to browser --bots 3 --fast   # alone, with three fake players
```

`--tunnel=cloudflare|opentunnel|auto` (default `auto`), `--no-tunnel`
(phones on this computer only), `--rounds N`, `--fast`, `--size WxH` (video).
Scan the QR code on the TV; the first player is the VIP and starts the
game once three are in.

## Why `@napi-rs/canvas`

The video screen is mostly text: names, prompts, lies, in several sizes
and any language, plus anti-aliased strokes. A 2D canvas does that well;
raw RGBA with a bitmap font would look retro and could not show "é" or
emoji in names. `@napi-rs/canvas` is prebuilt Skia with no install scripts
and no dependencies, and it is an optional dependency of this game only
(not of tvpipe or `@tvpipe/party`): without it the web screen still works.
It uses tvpipe's fonts (`web/phone/fonts/`, SIL OFL 1.1, `OFL.txt`).

## Tests

`npm test`: the rules and scoring, whole games with 3 and 8 fake phones
over real WebSockets (a player who drops mid-game and comes back, one who
leaves, too few players, a late joiner, the VIP removing a player), and the
video screen drawing every phase. `node test/live/play.ts [--video]
[--tunnel=cloudflare]` plays a game with a phone in headless Chrome and the
TV in tvpipe's browser player, and saves screenshots of both per phase.
