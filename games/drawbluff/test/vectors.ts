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
 * - `title`: `input` (a JSON string), `clean` (`cleanTitle`), `key`
 *   (`titleKey` of the input).
 * - `drawing`: `input` (JSON; `repeat = N` repeats its first stroke N
 *   times, `pad = N` appends N `1`s to its first stroke), then `output`
 *   (the normalised drawing; `input` when that is the input) or `error`
 *   (the code).
 * - `turn`: one drawing's turn as a seeded game plays it: `artist`,
 *   `truth`, `lies` (`[author, text]` in arrival order), `spares`,
 *   `seed`, `min`, `round`, `votes` (`[voter, option id]`), then
 *   `options` (`buildOptions(…, seeded(seed), min)`), `shown` (per voter:
 *   the ids `optionsFor` gives), `points`, `voters` (`score`), `reveal`
 *   (the ids in `revealOrder`).
 */

import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { PartyError } from "@tvpipe/party";

import { PROMPTS } from "../game/prompts.ts";
import {
  buildOptions,
  cleanTitle,
  optionsFor,
  revealOrder,
  score,
  seeded,
  shuffle,
  titleKey,
  validateDrawing,
} from "../game/rules.ts";

export const RULES = fileURLToPath(new URL("./vectors/rules.txt", import.meta.url));

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
  const n = 3 + Math.floor(r() * 6);
  const players = Array.from({ length: n }, (_, i) => `p${i + 1}`);
  const artist = pick(players);
  const others = players.filter((p) => p !== artist);
  const truth = pick(PROMPTS);
  const lies: [string, string][] = [];
  for (const p of others) {
    const x = r();
    if (x < 0.15) continue; // no lie in time
    if (x < 0.25) lies.push([p, truth.toUpperCase() + "!"]); // the truth (refused when typed; never shown)
    else if (x < 0.4) lies.push([p, pick(PROMPTS)]);
    else lies.push([p, pick(LIES)]);
  }
  const spares = shuffle(PROMPTS, r).slice(0, 6);
  const min = Math.min(4, others.length + 1);
  const options = buildOptions(truth, new Map(lies), spares, seeded(seed), min);
  const ids = [...options.map((o) => o.id), "nope"];
  const votes: [string, string][] = [];
  for (const p of r() < 0.2 ? players : others) {
    if (r() < 0.1) continue;
    votes.push([p, r() < 0.7 ? pick(optionsFor(p, options)).id : pick(ids)]);
  }
  const round = 1 + Math.floor(r() * 2);
  const scoring = score(artist, options, new Map(votes), round);
  return [
    ["test", "turn"],
    ["artist", JSON.stringify(artist)],
    ["truth", JSON.stringify(truth)],
    ["lies", JSON.stringify(lies)],
    ["spares", JSON.stringify(spares)],
    ["seed", String(seed)],
    ["min", String(min)],
    ["round", String(round)],
    ["votes", JSON.stringify(votes)],
    ["options", JSON.stringify(options)],
    ["shown", JSON.stringify(Object.fromEntries(players.map((p) => [p, optionsFor(p, options).map((o) => o.id)])))],
    ["points", JSON.stringify(Object.fromEntries(scoring.points))],
    ["voters", JSON.stringify(Object.fromEntries(scoring.voters))],
    ["reveal", JSON.stringify(revealOrder(options, scoring.voters).map((o) => o.id))],
  ];
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
    out.push(block([["test", "title"], ["input", t], ["clean", JSON.stringify(cleanTitle(t))], ["key", JSON.stringify(titleKey(t))]]));
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
  for (let seed = 1; seed <= 40; seed++) out.push(block(turn(seed)));
  writeFileSync(RULES, out.join("\n\n") + "\n");
  console.log(`wrote ${RULES}`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  if (process.argv.includes("--bless")) bless();
  else console.log("usage: node test/vectors.ts --bless");
}
