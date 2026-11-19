# drawbluff

A Drawful-style party game (our own name, prompts and look) on
[`@tvpipe/party`](../../npm/party): 3–50 players in one mode, everyone
draws a secret prompt on their phone, others invent fake titles for each
drawing, everyone votes for the real one. Points for finding the truth
(1000), for the artist per player who found it (1000), and for every
player fooled by your lie (500); round 2 counts double.

Up to 8 players it is the classic game: every drawing on the TV, every
other player lies. From 9 the round has six spotlight drawings (the rest
in a gallery on the TV, a page of eight at a time), at most seven liars
per drawing, rotating so everybody lies about as often, and from 13 the
next drawing's liars write while the TV shows the current vote. Every
phone gets at most six options, sampled so each lie is shown about
equally often, and the artist's and the liars' points are per exposure
(× min(1, 7 / voters who saw it)), so a big room does not inflate them
and the spotlight draw does not decide the game. Players can join
mid-game and play from the next round; every phone shows its role and
its rank ("#12 of 50"). The design and the numbers:
[`docs/drawbluff-scale.md`](../../docs/drawbluff-scale.md).

There are two hosts with the same rules, prompts (`data/prompts.txt`) and
phone page: Node (`main.ts`, `game/`, the protocol reference) and Rust
(`src/`, one binary with the TV screen drawn in Rust; see below).

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
node main.ts --to browser --bots 49 --fast  # a full room of 50
```

`--tunnel=cloudflare|opentunnel|auto` (default `auto`), `--no-tunnel`
(phones on this computer only), `--rounds N`, `--fast`, `--size WxH` (video).
Scan the QR code on the TV; the first player is the VIP and starts the
game once three are in. `--bots` takes up to 49.

## The Rust version

```sh
cargo run --release -p drawbluff -- --to "Living Room"       # video drawn in Rust
cargo run --release -p drawbluff -- --to "Living Room" --web-screen
cargo run --release -p drawbluff -- --to browser --open --bots 3 --fast
```

One binary: the party (`tvpipe-party`), the game (`src/game.rs`, a port
of `game/game.ts` on the same views), the TV screen rendered with
`tvpipe-canvas` (`src/screen.rs`, a port of `screen-video.ts`, with the
TrueType fonts in `fonts/`), sound cues (`src/sound.rs`), bots
(`src/bots.rs`), and the phone and web screen pages embedded. Phones
join through a public link by default (`--tunnel`), else on the LAN
(`--lan`, `http://IP:7420`, same Wi-Fi), or `--local`; `--rounds N`,
`--fast`, `--size WxH`, `--seed N`, `-v`.

The rules agree with the Node version by test: `node test/vectors.ts
--bless` writes `test/vectors/rules.txt` from `game/rules.ts` (seeded
turns, titles, drawings), and both `test/vectors.test.ts` and
`tests/vectors.rs` replay it. `cargo test -p drawbluff` also plays whole
games with bots (`tests/game.rs`, as `test/game.test.ts`) and draws every
screen (`tests/screen.rs`). `node test/live/play.ts --host rust
[--video]` runs the live check against the binary.

## Why `@napi-rs/canvas`

The video screen is mostly text: names, prompts, lies, in several sizes
and any language, plus anti-aliased strokes. A 2D canvas does that well;
raw RGBA with a bitmap font would look retro and could not show "é" or
emoji in names. `@napi-rs/canvas` is prebuilt Skia with no install scripts
and no dependencies, and it is an optional dependency of this game only
(not of tvpipe or `@tvpipe/party`): without it the web screen still works.
It uses tvpipe's fonts (`web/phone/fonts/`, SIL OFL 1.1, `OFL.txt`).

## Tests

`npm test`: the rules and scoring (and the vectors shared with Rust),
whole games with 3, 8 and 50 fake phones over real WebSockets (a player
who drops mid-game and comes back, one who leaves, too few players, a
late joiner, the VIP removing a player), and the video screen drawing
every phase (also with 50 names, `test/vectors/screens-50.json`, which
`tests/screen.rs` draws too). `node test/live/play.ts [--video] [--bots
N] [--rounds N] [--host rust] [--tunnel=cloudflare]` plays a game with a
phone in headless Chrome (it draws, lies and votes when asked) and up to
49 bots, the TV in tvpipe's browser player, and saves screenshots of both
per phase (gallery pages, votes, reveals and their end); with the web
screen it also reads the screen page for `null`/`undefined`/`NaN` and
anything laid out below the bottom.
