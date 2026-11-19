/**
 * Rule vectors shared with the Rust version (docs/multiscreen-design.md §4):
 * the Node rules are the reference. `node test/vectors.ts --bless` writes
 * `test/vectors/rules.txt` from `game/rules.ts`; `vectors.test.ts` and
 * the Rust tests (`src/rules.rs`) replay it.
 *
 * Format: tvpipe's test vector files (blocks of `key = value` lines,
 * separated by blank lines, `#` comments). Every block has a `test`:
 *
 * - `rng`: `seed`, `values` (the first outputs of `seeded(seed)` times
 *   2^32, so integers).
 * - `shuffle`: `seed`, `n`, `order` (`shuffle([0..n), seeded(seed))`).
 * - `title`: `input` (a JSON string), `clean` (`cleanTitle`), `display`
 *   (`displayTitle` of the cleaned input), `key` (`titleKey` of the input).
 * - `drawing`: `input` (JSON; `repeat = N` repeats its first stroke N
 *   times, `pad = N` appends N `1`s to its first stroke), then `output`
 *   (the normalised drawing; `input` when that is the input) or `error`
 *   (the code).
 * - `plan`: `table`, `[n, spotlights, liars, pipelined]` for n = 0…60.
 * - `share`: `cases`, `[n, base, seen, share(n, base, seen)]`.
 * - `liars`: `pool`, `lied` (`{id: lies so far}`), `n`, `seed`, `first`
 *   (or `null`), then `liars` (`pickLiars(…, seeded(seed), first)`).
 * - `rotate`: `pool`, `keys` (`{id: key}`), `n`, `seed`, then `picked`.
 * - `turn`: one drawing's turn as a seeded game plays it: `artist`,
 *   `truth`, `lies` (`[author, text]` in arrival order), `spares`,
 *   `seed`, `min`, `round`, `voting` (who votes), `dealSeed`, `votes`
 *   (`[voter, option id]`), then `options` (`buildOptions(…,
 *   seeded(seed), min)`), `own` (per player: the ids `optionsFor`
 *   gives), `shown` (`deal(options, voting, PHONE_LIES,
 *   seeded(dealSeed))`), `points`, `voters`, `seen` (`score`), `reveal`
 *   (the ids in `revealOrder`).
 */

import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { PartyError } from "@tvpipe/party";

import { doodle } from "../game/bots.ts";
import { PROMPTS } from "../game/prompts.ts";
import {
  buildOptions,
  cleanTitle,
  deal,
  displayTitle,
  optionsFor,
  PHONE_LIES,
  pickLiars,
  plan,
  revealOrder,
  rotate,
  score,
  seeded,
  share,
  shuffle,
  titleKey,
  validateDrawing,
} from "../game/rules.ts";

export const RULES = fileURLToPath(new URL("./vectors/rules.txt", import.meta.url));
/** Screen views of a 50-player game, drawn by both video screens' tests. */
export const SCREENS = fileURLToPath(new URL("./vectors/screens-50.json", import.meta.url));

export type Block = { line: number; entries: [string, string][] };

export function parseVectors(text: string): Block[] {
  const blocks: Block[] = [];
  let cur: Block | undefined;
  text.split("\n").forEach((raw, i) => {
    const line = raw.replace(/\r$/, "");
    if (line.trim() === "") {
      cur = undefined;
      return;
    }
    if (line.startsWith("#")) return;
    const at = line.indexOf(" = ");
    if (at < 0) throw new Error(`rules.txt:${i + 1}: expected "key = value"`);
    if (!cur) {
      cur = { line: i + 1, entries: [] };
      blocks.push(cur);
    }
    cur.entries.push([line.slice(0, at).trim(), line.slice(at + 3)]);
  });
  return blocks;
}

export function get(b: Block, key: string): string {
  const e = b.entries.find(([k]) => k === key);
  if (!e) throw new Error(`rules.txt:${b.line}: no ${key}`);
  return e[1];
}

export function has(b: Block, key: string): boolean {
  return b.entries.some(([k]) => k === key);
}

/** A `drawing` block's input, with `repeat` and `pad` applied. */
export function drawingInput(b: Block): unknown {
  const input = JSON.parse(get(b, "input")) as { strokes: unknown[][] };
  if (has(b, "repeat")) input.strokes = Array(Number(get(b, "repeat"))).fill(input.strokes[0]);
  if (has(b, "pad")) input.strokes[0] = [...input.strokes[0]!, ...Array(Number(get(b, "pad"))).fill(1)];
  return input;
}

// ---------------------------------------------------------------------------
// Writing the file

type Entries = [string, unknown][];

function block(entries: Entries): string {
  return entries.map(([k, v]) => `${k} = ${typeof v === "string" && k !== "input" ? v : JSON.stringify(v)}`).join("\n");
}

const TITLES = [
  "A Nervous  Cactus!",
  "nervous cactus",
  "The café",
  "cafe",
  "an apple",
  "an",
  "a",
  "the the cat",
  "theatre",
  "a-b-c",
  "  hi\u0000 there  ",
  "x".repeat(80),
  "Ⅻ angry men",
  "½ a sandwich",
  "ΟΔΟΣ",
  "İstanbul",
  "Straße",
  "ﬁsh ﬂakes",
  "ｆｕｌｌ ｗｉｄｔｈ",
  "Ⓐ circled",
  "🅐 negative",
  "東京タワー",
  "한국어 제목",
  "🍕 pizza 🍕",
  "👩‍👩‍👧 family",
  "!!!",
  "   ",
  "e\u0301te\u0301",
  "Ǆungla",
  "٣ cats",
  "rock'n'roll",
  "A\tB\nC",
  "\u202eevil",
  "zero\u200bwidth",
  // The display form: nothing gives the truth away.
  "A Cat.",
  "a cat",
  '"A Cat!"',
  "Why?!?",
  "wait…",
  "...",
  '""',
  "'tis the season",
  "the dogs'",
  "'single quoted'",
  "\u2018curly\u2019 and \u201Cdouble\u201D",
  "\u00ABguillemets\u00BB",
  "it\u2019s fine",
  "ΣΟΦΟΣ ΟΔΟΣ",
  "  Spaced   OUT  !! ",
  '"half quoted',
  "Paris!? Really.",
];

const DRAWINGS: Entries[] = [
  [["input", { strokes: [[1, 500, -5, 2000, 10.4, 20.6]] }]],
  [["input", { strokes: [] }]],
  [["input", { strokes: [[0, 1, 0.5, 1.5, 2.5, -0.5, 999.5, 1000.49]] }]],
  [["input", { strokes: [[7, 2.5, 3, 4], [6, 119.5, 1, 1, 2, 2]] }]],
  [["input", { strokes: [[1.0, 4, 1, 1]] }]],
  [["input", { strokes: [[0, -10, 5, 5]] }]],
  [["input", null]],
  [["input", {}]],
  [["input", []]],
  [["input", { strokes: {} }]],
  [["input", { strokes: [[1, 2, 3]] }]],
  [["input", { strokes: [[1, 2, 3, 4, 5]] }]],
  [["input", { strokes: [[9, 2, 3, 4]] }]],
  [["input", { strokes: [[-1, 2, 3, 4]] }]],
  [["input", { strokes: [[1.5, 2, 3, 4]] }]],
  [["input", { strokes: [[1, 2, "x", 4]] }]],
  [["input", { strokes: [[1, 2, true, 4]] }]],
  [["input", { strokes: [[1, 2, null, 4]] }]],
  [["input", { strokes: ["1234"] }]],
  [["input", { strokes: [[0, 4, 1, 1]] }], ["repeat", 300]],
  [["input", { strokes: [[0, 4, 1, 1]] }], ["repeat", 301]],
  [["input", { strokes: [[0, 4]] }], ["pad", 24_000]],
  [["input", { strokes: [[0, 4]] }], ["pad", 24_002]],
];

const LIES = [
  "a sad potato",
  "A sad potato!",
  "a confused banana",
  "the cat's revenge",
  "a dancing lamp",
  "abstract art",
  "Abstract   art",
  "a sneaky pancake",
  "the evil teapot",
  "a very tired frog",
];

function turn(seed: number): Entries {
  const r = seeded(seed * 7919 + 1);
  const pick = <T>(xs: readonly T[]): T => xs[Math.floor(r() * xs.length)]!;
  // 3–8 players (today's game), then up to 50.
  const n = seed <= 24 ? 3 + Math.floor(r() * 6) : 9 + Math.floor(r() * 42);
  const players = Array.from({ length: n }, (_, i) => `p${i + 1}`);
  const artist = pick(players);
  const others = players.filter((p) => p !== artist);
  const liars = pickLiars(others, new Map(), plan(n).liars, r);
  const truth = pick(PROMPTS);
  const lies: [string, string][] = [];
  for (const p of liars) {
    const x = r();
    if (x < 0.15) continue; // no lie in time
    if (x < 0.25) lies.push([p, truth.toUpperCase() + "!"]); // the truth (refused when typed; never shown)
    else if (x < 0.4) lies.push([p, pick(PROMPTS)]);
    else lies.push([p, pick(LIES)]);
  }
  const spares = shuffle(PROMPTS, r).slice(0, 6);
  const min = Math.min(4, liars.length + 1);
  const options = buildOptions(truth, new Map(lies), spares, seeded(seed), min);
  const voting = r() < 0.2 ? players : others;
  const dealSeed = seed + 1000;
  const shown = deal(options, voting, PHONE_LIES, seeded(dealSeed));
  const ids = [...options.map((o) => o.id), "nope"];
  const votes: [string, string][] = [];
  for (const p of voting) {
    if (r() < 0.1) continue;
    votes.push([p, r() < 0.8 ? pick(shown.get(p)!) : pick(ids)]);
  }
  const round = 1 + Math.floor(r() * 2);
  const scoring = score(artist, options, shown, new Map(votes), round);
  return [
    ["test", "turn"],
    ["artist", JSON.stringify(artist)],
    ["truth", JSON.stringify(truth)],
    ["lies", JSON.stringify(lies)],
    ["spares", JSON.stringify(spares)],
    ["seed", String(seed)],
    ["min", String(min)],
    ["round", String(round)],
    ["voting", JSON.stringify(voting)],
    ["dealSeed", String(dealSeed)],
    ["votes", JSON.stringify(votes)],
    ["options", JSON.stringify(options)],
    ["own", JSON.stringify(Object.fromEntries(players.map((p) => [p, optionsFor(p, options).map((o) => o.id)])))],
    ["shown", JSON.stringify(Object.fromEntries(shown))],
    ["points", JSON.stringify(Object.fromEntries(scoring.points))],
    ["voters", JSON.stringify(Object.fromEntries(scoring.voters))],
    ["seen", JSON.stringify(Object.fromEntries(scoring.seen))],
    ["reveal", JSON.stringify(revealOrder(options, scoring.voters).map((o) => o.id))],
  ];
}

function liarsBlock(seed: number): Entries {
  const r = seeded(seed * 31 + 5);
  const n = 2 + Math.floor(r() * 49);
  const pool = Array.from({ length: n }, (_, i) => `p${i + 1}`);
  const lied = new Map(pool.filter(() => r() < 0.6).map((p) => [p, Math.floor(r() * 3)]));
  const want = Math.floor(r() * 9);
  const first = r() < 0.5 ? pick(r, pool) : r() < 0.5 ? "nobody" : null;
  const liars = pickLiars(pool, lied, want, seeded(seed), first ?? undefined);
  return [
    ["test", "liars"],
    ["pool", JSON.stringify(pool)],
    ["lied", JSON.stringify(Object.fromEntries(lied))],
    ["n", String(want)],
    ["seed", String(seed)],
    ["first", JSON.stringify(first)],
    ["liars", JSON.stringify(liars)],
  ];
}

function rotateBlock(seed: number): Entries {
  const r = seeded(seed * 17 + 3);
  const n = 1 + Math.floor(r() * 50);
  const pool = Array.from({ length: n }, (_, i) => `p${i + 1}`);
  const keys = new Map(pool.map((p) => [p, Math.floor(r() * 4)]));
  const want = Math.floor(r() * (n + 2));
  const picked = rotate(pool, (id) => keys.get(id)!, want, seeded(seed));
  return [
    ["test", "rotate"],
    ["pool", JSON.stringify(pool)],
    ["keys", JSON.stringify(Object.fromEntries(keys))],
    ["n", String(want)],
    ["seed", String(seed)],
    ["picked", JSON.stringify(picked)],
  ];
}

function pick<T>(r: () => number, xs: readonly T[]): T {
  return xs[Math.floor(r() * xs.length)]!;
}

const NAMES = ["Ada", "Bea", "Cy", "Dot", "Eli", "Fay", "Gus", "Hal", "Ivy", "Jo", "Kai", "Lou", "Max", "Nell", "Oz", "Pia", "Quinn"];

/** A 50-player game's screen views (`{roster, join, now, views: [[name, screen]]}`). */
export function bigScreens(): Record<string, unknown> {
  const r = seeded(50);
  const roster = Array.from({ length: 50 }, (_, i) => ({
    id: `p${i + 1}`,
    name: i === 7 ? "Maximiliane-Josephine" : `${NAMES[i % NAMES.length]}${i >= NAMES.length ? Math.floor(i / NAMES.length) + 1 : ""}`,
    vip: i === 0,
    connected: i % 13 !== 5,
  }));
  const ids = roster.map((p) => p.id);
  const now = 1_000_000;
  const rows = Array.from({ length: 21 }, (_, y) => Array.from({ length: 21 }, (_, x) => ((x < 7 && y < 7) || (x > 13 && y < 7) || (x < 7 && y > 13) ? "1" : "0")).join(""));
  const join = { code: "BCDF", url: "https://words-words-words-words.trycloudflare.com/?r=BCDF#k=abcdefghijklmnopqrstuv", qr: { size: 21, rows }, lateJoin: "open", inProgress: true };
  const drawing = doodle(seeded(9));
  // Newcomers this round ("+ Anna" in the corner): the last three and a count.
  const joined = { joined: ["Anna", "Maximiliane-Josephine", "Bob"], joinedMore: 2 };
  const artist = { id: "p4", name: roster[3]!.name };
  const liars = ["p4", "p9", "p17", "p23", "p30", "p41", "p48"];
  const voters = ids.filter((id) => id !== "p4" && !liars.includes(id));
  const gallery = Array.from({ length: 8 }, (_, i) => ({ id: ids[10 + i]!, name: roster[10 + i]!.name, prompt: PROMPTS[i * 7]!, drawing: doodle(seeded(100 + i)) }));
  const options = ["a haunted toaster", "a sad potato", "the cat's revenge", "my uncle on monday", "a very tired frog", "abstract art", "a sneaky pancake", "a convincing lie that is rather long indeed"].map((text, i) => ({ id: `o${i + 1}`, text }));
  const names = (n: number) => voters.slice(0, n).map((id) => roster.find((p) => p.id === id)!.name);
  const item = (id: string, text: string, n: number, seen: number, o: Record<string, unknown> = {}) => ({
    id, text, truth: false, decoy: false, authors: [roster[(n * 7) % 50]!.name], voters: names(Math.min(n, 6)), count: n, seen,
    points: share(n, 500, seen), artistPoints: 0, same: false, ...o,
  });
  const lines = roster.slice(0, 8).map((p, i) => ({ id: p.id, name: p.name, score: 9000 - i * 650, delta: 1200 - i * 100 }));
  const views: [string, Record<string, unknown>][] = [
    ["lobby-50", { phase: "lobby", minPlayers: 3, maxPlayers: 50 }],
    ["draw-50", { phase: "draw", round: 1, rounds: 2, deadline: now + 42_000, waiting: ids, done: ids.filter((_, i) => i % 9 !== 2), ...joined }],
    ["gallery-50", { phase: "gallery", round: 1, rounds: 2, page: 2, pages: 6, count: 44, items: gallery, liars, done: liars.slice(0, 3), artist, ...joined }],
    ["lie-50", { phase: "lie", round: 1, spot: 2, spots: 6, deadline: now + 4000, drawing, artist, waiting: liars, done: liars.slice(0, 5), ...joined }],
    ["vote-50", { phase: "vote", round: 1, spot: 2, spots: 6, deadline: now + 14_000, drawing, artist, options, waiting: voters, done: voters.filter((_, i) => i % 4 !== 0), writing: 7, ...joined }],
    [
      "reveal-50",
      {
        phase: "reveal", round: 2, spot: 3, spots: 6, drawing, artist, prompt: "a haunted toaster", step: 5,
        items: [
          item("o2", "a sad potato", 2, 30),
          item("o5", "a very tired frog", 4, 31, { same: true, authors: ["Bea", "Lou"] }),
          item("o7", "a sneaky pancake", 9, 29),
          item("o3", "the cat's revenge", 14, 31),
          { id: "o1", text: "a haunted toaster", truth: true, decoy: false, authors: [], voters: names(6), count: 14, seen: 43, points: 2000, artistPoints: 2 * share(14, 1000, 43), same: false },
        ],
      },
    ],
    [
      // Seven liars and the truth: the most a drawing gets.
      "reveal-50-full",
      {
        phase: "reveal", round: 1, spot: 1, spots: 6, drawing, artist, prompt: "a haunted toaster", step: 8, ...joined,
        items: [
          item("o2", "a sad potato", 1, 30),
          item("o4", "my uncle on monday", 2, 31, { authors: ["Maximiliane-Josephine", "Quinn2"] }),
          item("o5", "a very tired frog", 3, 31, { same: true, authors: ["Bea", "Lou"] }),
          item("o6", "abstract art", 4, 29, { decoy: true, authors: [] }),
          item("o7", "a sneaky pancake", 6, 29),
          item("o8", "a convincing lie that is rather long indeed", 9, 30),
          item("o3", "the cat's revenge", 14, 31),
          { id: "o1", text: "a haunted toaster", truth: true, decoy: false, authors: [], voters: names(6), count: 11, seen: 43, points: 2000, artistPoints: 2 * share(11, 1000, 43), same: false },
        ],
      },
    ],
    [
      "scores-50",
      {
        phase: "scores", round: 1, rounds: 2, scores: lines, total: 50, last: false, ...joined,
        highlights: [{ kind: "liar", id: "p9", name: roster[8]!.name, count: 23 }, { kind: "eye", id: "p12", name: roster[11]!.name, count: 5, of: 6 }],
      },
    ],
    ["final-50", { phase: "final", scores: lines.map((l) => ({ ...l, delta: 0 })), total: 50 }],
  ];
  void r;
  return { roster, join, now, views };
}

function bless(): void {
  const out: string[] = [
    "# drawbluff's rules, written by `node test/vectors.ts --bless` from game/rules.ts",
    "# (the reference); replayed by test/vectors.test.ts and src/rules.rs.",
  ];
  for (const seed of [0, 1, 7, 42, 0xdeadbeef, 0xffffffff]) {
    const r = seeded(seed);
    const values = Array.from({ length: 8 }, () => r() * 2 ** 32);
    out.push(block([["test", "rng"], ["seed", String(seed)], ["values", JSON.stringify(values)]]));
  }
  for (const [seed, n] of [[1, 1], [1, 2], [7, 5], [42, 10], [3, 52]] as const) {
    const order = shuffle(Array.from({ length: n }, (_, i) => i), seeded(seed));
    out.push(block([["test", "shuffle"], ["seed", String(seed)], ["n", String(n)], ["order", JSON.stringify(order)]]));
  }
  for (const t of TITLES) {
    out.push(block([["test", "title"], ["input", t], ["clean", JSON.stringify(cleanTitle(t))], ["display", JSON.stringify(displayTitle(cleanTitle(t)))], ["key", JSON.stringify(titleKey(t))]]));
  }
  for (const d of DRAWINGS) {
    const b: Block = { line: 0, entries: d.map(([k, v]) => [k, k === "input" ? JSON.stringify(v) : String(v)]) };
    let result: [string, unknown];
    try {
      const input = drawingInput(b);
      const output = JSON.stringify(validateDrawing(input));
      // Long ones that pass unchanged stay short.
      result = ["output", output.length > 400 && output === JSON.stringify(input) ? "input" : output];
    } catch (e) {
      if (!(e instanceof PartyError)) throw e;
      result = ["error", e.code];
    }
    out.push(block([["test", "drawing"], ...d, result]));
  }
  const table = Array.from({ length: 61 }, (_, n) => {
    const p = plan(n);
    return [n, p.spotlights, p.liars, p.pipelined];
  });
  out.push(block([["test", "plan"], ["table", JSON.stringify(table)]]));
  const cases: number[][] = [];
  for (const [n, base, seen] of [[0, 500, 0], [3, 500, 7], [7, 1000, 7], [1, 500, 8], [14, 500, 30], [20, 1000, 43], [1, 1000, 49], [5, 500, 14], [1, 500, 28], [3, 1000, 42]] as const) {
    cases.push([n, base, seen, share(n, base, seen)]);
  }
  out.push(block([["test", "share"], ["cases", JSON.stringify(cases)]]));
  for (let seed = 1; seed <= 12; seed++) out.push(block(liarsBlock(seed)));
  for (let seed = 1; seed <= 8; seed++) out.push(block(rotateBlock(seed)));
  for (let seed = 1; seed <= 48; seed++) out.push(block(turn(seed)));
  writeFileSync(RULES, out.join("\n\n") + "\n");
  console.log(`wrote ${RULES}`);
  writeFileSync(SCREENS, JSON.stringify(bigScreens()) + "\n");
  console.log(`wrote ${SCREENS}`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  if (process.argv.includes("--bless")) bless();
  else console.log("usage: node test/vectors.ts --bless");
}
