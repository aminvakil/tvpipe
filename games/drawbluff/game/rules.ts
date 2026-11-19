/**
 * drawbluff's rules as pure functions: drawings, titles, options, votes
 * and points. No I/O, no clocks; `game.ts` drives them.
 */

import { cleanText, PartyError } from "@tvpipe/party";

/** The 8 colours a drawing may use (index → CSS colour). */
export const PALETTE = ["#1d1d1f", "#ffffff", "#e5383b", "#f9a620", "#ffd60a", "#2a9d8f", "#3a86ff", "#8338ec"] as const;

/** A drawing on a 1000×1000 grid: strokes of `[colour, width, x0, y0, x1, y1, …]`. */
export interface Drawing {
  strokes: number[][];
}

export const GRID = 1000;
export const MAX_STROKES = 300;
export const MAX_POINTS = 12_000;
export const MAX_TITLE = 40;

/** Checks and normalises a drawing from a phone (throws `PartyError("bad-drawing")`). */
export function validateDrawing(data: unknown): Drawing {
  const bad = (why: string) => new PartyError("bad-drawing", `That drawing cannot be sent (${why})`);
  if (typeof data !== "object" || data === null || !Array.isArray((data as Drawing).strokes)) throw bad("no strokes");
  const strokes = (data as Drawing).strokes;
  if (strokes.length > MAX_STROKES) throw bad("too many strokes");
  let points = 0;
  const out: number[][] = [];
  for (const s of strokes) {
    if (!Array.isArray(s) || s.length < 4 || s.length % 2 !== 0) throw bad("a broken stroke");
    if (!s.every((v) => typeof v === "number" && Number.isFinite(v))) throw bad("not numbers");
    const [color, width, ...xy] = s as number[];
    if (!Number.isInteger(color) || color! < 0 || color! >= PALETTE.length) throw bad("unknown colour");
    points += xy.length / 2;
    if (points > MAX_POINTS) throw bad("too many points");
    out.push([color!, clamp(Math.round(width!), 2, 120), ...xy.map((v) => clamp(Math.round(v), 0, GRID))]);
  }
  return { strokes: out };
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}

/** A title as typed: cleaned up for display (or "" when nothing is left). */
export function cleanTitle(text: unknown): string {
  return cleanText(text, MAX_TITLE);
}

/**
 * How titles compare: case, accents, punctuation, spacing and a leading
 * article do not matter ("A nervous cactus!" = "nervous cactus").
 */
export function titleKey(text: string): string {
  return text
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .trim()
    .replace(/^(a|an|the) /, "")
    .replace(/ /g, "");
}

/** One choice on the vote screen. */
export interface Option {
  id: string;
  text: string;
  /** The real title. */
  truth: boolean;
  /** Who wrote this lie (ids; several when their lies were the same). */
  authors: string[];
  /** A lie the game added (too few players' lies). */
  decoy: boolean;
}

/**
 * The options for one drawing: the truth, the players' lies (equal lies
 * merged), decoys from `spares` until there are at least `min`, in a
 * shuffled order.
 */
export function buildOptions(
  truth: string,
  lies: ReadonlyMap<string, string>,
  spares: readonly string[],
  rng: () => number,
  min = 4,
): Option[] {
  const byKey = new Map<string, Option>();
  const truthKey = titleKey(truth);
  let n = 0;
  for (const [author, text] of lies) {
    const k = titleKey(text);
    if (!k || k === truthKey) continue; // refused when typed; never shown
    const o = byKey.get(k);
    if (o) o.authors.push(author);
    else byKey.set(k, { id: `o${++n}`, text, truth: false, authors: [author], decoy: false });
  }
  const options = [{ id: `o${++n}`, text: truth, truth: true, authors: [], decoy: false }, ...byKey.values()];
  for (const s of spares) {
    if (options.length >= min) break;
    const k = titleKey(s);
    if (k === truthKey || byKey.has(k)) continue;
    byKey.set(k, { id: "", text: s, truth: false, authors: [], decoy: true });
    options.push({ id: `o${++n}`, text: s, truth: false, authors: [], decoy: true });
  }
  return shuffle(options, rng);
}

export function shuffle<T>(items: readonly T[], rng: () => number): T[] {
  const a = [...items];
  for (let i = a.length - 1; i > 0; i--) {
    const j = Math.floor(rng() * (i + 1));
    [a[i], a[j]] = [a[j]!, a[i]!];
  }
  return a;
}

/** Points for one drawing: voter id → option id. */
export interface Scoring {
  /** Points per player id. */
  points: Map<string, number>;
  /** Per option: who picked it. */
  voters: Map<string, string[]>;
}

export const POINTS = {
  /** A voter who found the truth. */
  guess: 1000,
  /** The artist, per voter who found the truth. */
  artist: 1000,
  /** A liar, per voter who fell for the lie. */
  fooled: 500,
} as const;

export function score(artist: string, options: readonly Option[], votes: ReadonlyMap<string, string>, round: number): Scoring {
  const points = new Map<string, number>();
  const voters = new Map<string, string[]>(options.map((o) => [o.id, []]));
  const add = (id: string, n: number) => points.set(id, (points.get(id) ?? 0) + n * round);
  for (const [voter, optionId] of votes) {
    const o = options.find((x) => x.id === optionId);
    if (!o || voter === artist || o.authors.includes(voter)) continue;
    voters.get(o.id)!.push(voter);
    if (o.truth) {
      add(voter, POINTS.guess);
      add(artist, POINTS.artist);
    } else {
      for (const a of o.authors) add(a, POINTS.fooled);
    }
  }
  return { points, voters };
}

/** The options a voter sees: all but the ones they wrote. */
export function optionsFor(voter: string, options: readonly Option[]): { id: string; text: string }[] {
  return options.filter((o) => !o.authors.includes(voter)).map((o) => ({ id: o.id, text: o.text }));
}

/**
 * The reveal order: lies nobody picked are skipped, the others from the
 * fewest to the most votes, the truth last.
 */
export function revealOrder(options: readonly Option[], voters: ReadonlyMap<string, string[]>): Option[] {
  const lies = options.filter((o) => !o.truth && (voters.get(o.id)?.length ?? 0) > 0);
  lies.sort((a, b) => (voters.get(a.id)!.length - voters.get(b.id)!.length));
  return [...lies, ...options.filter((o) => o.truth)];
}

/** A seeded generator (mulberry32) for tests and replays. */
export function seeded(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
