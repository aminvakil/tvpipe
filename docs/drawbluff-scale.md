# drawbluff at scale (design)

drawbluff for 3 to 50 players in one mode, on one TV. Today's game
(`games/drawbluff`, `docs/party-design.md` §10) is the small end of it:
with 3–6 players nothing changes but a few extra lines on the screens.
The goal for 50: everybody's phone always has something to do, the TV
stays readable from the sofa, everybody watches the same moment, a round
lasts about as long as with 8, and the scores stay fair although not
everybody gets the same turns.

## 1. The shape of a round

Everyone draws their own prompt at the same time, as today. Then the
round puts **K spotlight drawings** on the stage, one after the other,
and every spotlight goes through the classic moments on the TV:

```
draw ─▶ [gallery] ─▶ spotlight 1 ─▶ spotlight 2 ─▶ … ─▶ spotlight K ─▶ scores
          (N > K)    lie ▸ vote ▸ reveal
```

- **K = min(N, 6)** spotlights per round. With up to 6 players every
  drawing gets the stage (today's game); with more, the rest of the
  round's drawings are shown in a short **gallery** right after drawing
  (§4), and the choice of spotlights rotates so that over the game
  everybody's drawing gets the stage before anyone's gets it twice
  (§5).
- Per spotlight a **rotating subset of players are its liars**
  (L = min(N−1, 7); everybody but the artist up to 8 players, as today).
  Every phone says what its player does for this drawing (§3).
- Everybody but the artist (and, when pipelined, the next drawing's
  liars) **votes**. Each phone shows the truth and up to 5 lies, a fair
  sample when there are more (§6); the TV shows all of them (at most 8
  lines, as today's 8-player game).
- **Pipelining** (from 13 players): the next spotlight's liars get their
  drawing on the phone when the current vote starts and write while the
  others vote and watch the reveal; they sit that one vote out. When the
  reveal ends their lies are in, so the next vote follows at once (§2).

There is one state machine for every N. What N changes is only *when*
a spotlight's liars get their drawing: at the start of its lie moment
(small N), during the gallery (the first spotlight, when there is one),
or during the previous vote (pipelined). The TV always shows the same
lie screen ("Bob drew this · the liars are writing"), but only while
lies are still missing: for 30–45 s with few players, for a few seconds
(often none) when the liars had a head start. Nothing on the TV or the
phones announces a mode; a late joiner who tips the room from 12 to 13
players only makes the next round a bit brisker.

## 2. When liars write: the plan per N

| | liars L | pipelined | voters per spotlight |
|---|---|---|---|
| N ≤ 8 | N−1 (everybody but the artist) | no | N−1 (liars vote too, never for their own lie) |
| 9–12 | 7 | no | N−1 |
| 13–14 | 6 | yes | N−L (= 7–8) |
| ≥ 15 | 7 | yes | N−7 (8 at 15, 13 at 20, 43 at 50) |

Pipelined, the next spotlight's liars are taken from those who are free
or least needed during the current vote:

- **the current artist always** (they do not vote on their own drawing,
  so they are free during it),
- never the current liars (they watch their lies fool people: the best
  moment of their turn),
- never the next artist,
- the rest by rotation (fewest lies so far this game, ties at random).

That needs N−1 (the next artist) − L (the current liars) ≥ L, so N ≥
2L+1: L=6 from 13, L=7 from 15. Below 13 the voters of a spotlight would
be too few if the next liars sat out (4 at N=10), so liars write in the
spotlight's own lie moment, as today; at 9–12 the 1–4 players who are
not liars that time see "you're not lying on this one · you vote next".

The gallery (N > 6) covers the first spotlight's lies in every mode:
its liars get the drawing when the gallery starts.

## 3. Who does what (the phone always says it)

| Task (`me.task`) | Phone shows | When |
|---|---|---|
| `draw` | the prompt, the pad | draw phase, everybody in the round |
| `wait` | "You're in! You join at the next round." | a late joiner, until the next round |
| `lie` | "You're a liar for Bob's drawing (1 of 7)" + the drawing + the input; pipelined also "It's next on the TV: you sit this vote out" | from when the liars get the drawing until they answer |
| `artist` | "That's your drawing! Watch the others try to name it." | lie and vote of their spotlight (unless they are the next liars) |
| `vote` | the drawing, the truth and ≤ 5 lies (in the TV's order) | vote |
| `watch` | the note of the moment (gallery: "your drawing is in it"; lie: "not lying on this one"; reveal: your result "+1000, you found it" / "fooled by Anna") + own score and **rank "#12 of 50"** | everything else |
| `scores`, `final` | score, rank of N | between rounds, the end |

Every state carries `score`, `rank` and `of`, so the phone always shows
"#12 of 50".

## 4. What the TV shows (legible for 50)

The TV is the shared moment; it never shows anything that is not the
same for everyone. Name lists become counts and bars when they would
not fit at sofa distance:

- **Lobby**: the QR code and the players; up to 8 as cards (today),
  more as a grid of name chips that shrinks to fit 50 (5 columns).
- **Draw**: the timer and who is done: chips up to 12, else "37 of 50
  done", a progress bar and the names of the last ≤ 6 still drawing.
- **Gallery** (N > 6): this round's other drawings, 8 per page (4×2)
  with prompt and artist, 5 s a page; a strip "Meanwhile 7 liars are
  writing for the first drawing".
- **Lie**: the drawing, "Bob drew this", the liars' chips (≤ 7) ticking
  off.
- **Vote**: the drawing and all options (≤ 8 lines, like today with 8
  players); who voted as chips up to 12, else "31 of 43 voted" and a bar.
- **Reveal**: as today, lies one by one (only those that fooled
  someone, fewest first), the truth last. Each line says who wrote it
  and "fooled Ada, Cy" up to 4 names, else **"fooled 14 of 31"** (31 =
  voters whose phone showed it); the truth "found by 20 of 43".
- **Scores**: the top lines (all up to 8 players; top 5 with more, "your
  rank is on your phone"), and the round's **highlights**: the best liar
  ("Anna fooled 23"), the sharpest eye ("Bob found 5 of 6"); at every N.
- **Final**: the winner, the top lines.

Views are a pure function of the game state, as today: the web screen
(`web/screen/`), the Node video screen (`screen-video.ts`) and the Rust
screen (`src/screen.rs`) draw the same `screen` view.

## 5. Spotlights: rotation and fairness

After drawing, the round picks K of the submitted drawings: drawings of
players still connected, the fewest spotlights so far first, then
non-empty ones, ties at random (shuffle, then a stable sort; both hosts
do it with the shared seeded generator). Over two rounds that gives
12 spotlights: everybody up to 12 players has the stage at least once;
beyond, everybody is in a gallery or on the stage every round, and
nobody gets the stage twice before everybody had it once (with three
rounds: 18 players).

Liars rotate the same way: fewest lies so far this game, ties at random
(the current artist first when pipelined, §2). At N=50 over two rounds
there are 12 × 7 = 84 liar seats: everybody lies once or twice.

## 6. Bounded choice and fair samples

The options are built as today: the truth, the liars' lies (equal lies
by `titleKey` merged into one option with several authors), decoys
(unused prompts) up to 4 options, shuffled once. The TV lists all of
them (≤ 1 + 7).

A phone shows the truth and at most **5 lies** (6 buttons and the
drawing fit a phone without scrolling), never its own lie, in the TV's
order (so "the third one" means the same thing in the room). When a
voter has more than 5 lies to choose from, the phone gets a sample that
balances exposure: voters are dealt in random order, each gets the 5
lies that were dealt least so far (ties at random). Every lie is shown
to the same number of voters ±1 (at N=50: each of 7 lies to ~30 of 43).
Up to 7 players nobody is ever sampled (today's game); from 8 on
voters are (at 8: 5 of the 6 other lies).

## 7. Scoring: per exposure

Points as today (round r counts r times), scaled so that a big room
does not inflate them:

- **found the truth**: 1000 to the voter.
- **artist**: 1000 per voter who found it, × min(1, 7 / V) (V = voters
  who voted).
- **fooled**: 500 per voter fooled, × min(1, 7 / S) to each author (S =
  voters whose phone showed that lie and who voted: its exposure).
- **same idea**: 100 to each author of a merged lie (two liars thought
  the same thing).

7 is the most voters today's game has, so up to 8 players every number
is today's. Rounded half up in integers (`(2·n·p·7 + S) div 2S`), the
same in both hosts. Examples at N=50: a lie shown to 30 voters that
fools 14 earns 500·14·7/30 = 1633 (not 7000); an artist found by 20 of
43 earns 3256 (not 20 000). One correct vote is still worth 1000, so a
good guesser (≈10 votes a game) keeps up with somebody whose drawing got
the stage, and the luck of the spotlight draw does not decide the game.

Highlights (per round, from raw counts): best liar = most voters fooled
in the round; sharpest eye = most truths found (ties: by name).

## 8. Numbers per N (default timings: draw 90 s, lie 45 s, vote 25 s, reveal 2.6 s a line + 4.5 s, gallery 5 s a page, scores 9 s)

| | N=3 | N=8 | N=20 | N=50 |
|---|---|---|---|---|
| spotlights K / round | 3 (all) | 6 | 6 | 6 |
| gallery drawings / pages | – | 2 / 1 | 14 / 2 | 44 / 6 |
| liars L / pipelined | 2 / no | 7 / no | 7 / yes | 7 / yes |
| voters per spotlight | 2 | 7 | 13 | 43 |
| options on TV / phone | 3 / 2 (all but your own) | 8 / 6 | 8 / 6 | 8 / 6 |
| TV lie moment | ~30 s each | first: gallery 5 s + ~25 s; then ~30 s | ~0–5 s (first: after gallery, ~35 s) | ~0–5 s (first: gallery 30 s + ~10 s) |
| per spotlight on TV | lie 30 + vote 12 + reveal 12 | 30 + 20 + 20 | ~2 + 25 + 25 | ~2 + 25 + 25 |
| round | ≈ 4 min | ≈ 9 min | ≈ 7.5 min | ≈ 7.5 min |
| idle phones | none but the artist | the artist | none (artist writes the next lie) | none |
| stage per player per game (2 rounds) | 2 | 1.5 | 0.6 + 1.4 gallery | 0.24 + 1.76 gallery |
| lies per player per game | 4 | 10.5 | 4.2 | 1.7 |
| votes per player per game | 4 | 10.5 | 7.8 | 10.3 |

(Small rooms end phases early when everybody answered; big rooms
usually wait for the deadline, hence the round lengths.)

## 9. Joining, leaving, the VIP

- The room is no longer locked during a game: a new name joins at once
  and sees `wait` ("you join at the next round"); the next draw phase
  takes everybody who is in the room. The lobby's maximum is the game's
  (`maxPlayers` 50).
- A player who leaves during a round is dropped from the pools; their
  spotlight is skipped (as today); lies already in stay.
- VIP and kick: as today (the VIP starts, kicks only in the lobby, the
  next connected player becomes VIP).

## 10. The platform at 50 (`tvpipe-party`, `@tvpipe/party`)

No protocol change. The hosts send the same messages, cheaper:

- **Section-wise diffs**: a view is `{roster, room, me}` (players) or
  `{roster, screen, join}`; a flush diffs only the sections that changed
  and computes the shared sections' diff once for all clients that had
  the same version (the room's drawing is not re-diffed 50 times when one
  player's `me` changes). The bytes on the wire are unchanged (the
  session vectors still pass).
- **Sockets**: the default socket limit follows `maxPlayers` (players +
  screens + reconnect headroom) instead of a flat 64.
- **50-phone tests** in both hosts measure CPU per flush and bytes per
  state change (§11).

The sampling and rotation helpers stay in the game's rules: they need
the game's seeded generator and its rule vectors, and nothing else uses
them yet.

## 11. Measurements

(Filled in from the 50-phone tests and the live check.)
