import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { dictionary, parseWordList, type WordEntry } from "../game/match.ts";
import {
  addLate,
  awards,
  guesserOrder,
  guessesEach,
  mvpOfNight,
  pairKey,
  payouts,
  pot,
  rank,
  related,
  roomTitle,
  seeded,
  validateFace,
  web,
  WordDeck,
  type PlayerStats,
} from "../game/rules.ts";

const WORDS = parseWordList(readFileSync(new URL("../data/words.txt", import.meta.url), "utf8"));
const known = dictionary(readFileSync(new URL("../data/dict.txt", import.meta.url), "utf8"));

const entry = (word: string, difficulty = "easy", category = "thing"): WordEntry => ({ word, difficulty, category, alsoRight: [], banned: [] });

test("the pot: 10, −1 per jinxed clue, −2 per card after the first, never below 2", () => {
  assert.equal(pot(0, 0), 10);
  assert.equal(pot(0, 1), 10);
  assert.equal(pot(0, 2), 8);
  assert.equal(pot(2, 3), 4);
  assert.equal(pot(3, 3), 3);
  assert.equal(pot(6, 4), 2);
  assert.equal(pot(20, 9), 2);
});

test("payouts per §4", () => {
  const cards = [
    { author: "b", bold: false, fate: "alive" as const, turned: true },
    { author: "c", bold: true, fate: "alive" as const, turned: true },
    { author: "d", bold: false, fate: "alive" as const, turned: false },
    { author: "e", bold: true, fate: "jinxed" as const, turned: false },
    { author: "f", bold: false, fate: "jinxed" as const, turned: false },
  ];
  assert.deepEqual(payouts("a", "right", 6, cards), [
    { id: "a", delta: 6, why: "pot" },
    { id: "b", delta: 2, why: "turned" },
    { id: "c", delta: 4, why: "bold" },
    { id: "e", delta: -2, why: "bold-jinxed" },
  ]);
  assert.deepEqual(payouts("a", "wrong", 6, cards), [
    { id: "a", delta: -2, why: "wrong" },
    { id: "e", delta: -2, why: "bold-jinxed" },
  ]);
  assert.deepEqual(payouts("a", "pass", 6, cards), [{ id: "e", delta: -2, why: "bold-jinxed" }]);
  // 3 players: two cards of one author are summed per reason.
  const two = [
    { author: "b", bold: false, fate: "alive" as const, turned: true },
    { author: "b", bold: false, fate: "alive" as const, turned: true },
    { author: "c", bold: true, fate: "alive" as const, turned: true },
    { author: "c", bold: true, fate: "jinxed" as const, turned: false },
  ];
  assert.deepEqual(payouts("a", "right", 8, two), [
    { id: "a", delta: 8, why: "pot" },
    { id: "b", delta: 4, why: "turned" },
    { id: "c", delta: 4, why: "bold" },
    { id: "c", delta: -2, why: "bold-jinxed" },
  ]);
});

test("related words: one is an invalid clue for the other", () => {
  assert.equal(related(entry("fire"), entry("firework"), { known }), true);
  assert.equal(related(entry("cat"), entry("cats"), { known }), true);
  assert.equal(related(entry("snow"), entry("snowman"), { known }), true);
  assert.equal(related(entry("cat"), entry("dog"), { known }), false);
  assert.equal(related(entry("ant"), entry("elephant"), { known }), false);
  assert.equal(related(entry("cat", "easy", "animal"), { ...entry("chat"), banned: [] }, { known }), false);
  assert.equal(related({ ...entry("cat"), banned: ["gato"] }, entry("gato"), { known }), true);
});

test("the deck: no word twice until all are dealt, no related words in a game, by difficulty", () => {
  const words = ["apple", "pear", "plum", "grape", "melon", "fire", "firework", "kiwi"].map((w) => entry(w));
  const deck = new WordDeck(words, seeded(3), [], { known });
  const seen: string[] = [];
  for (let i = 0; i < 6; i++) seen.push(deck.deal("easy", []).word);
  assert.equal(new Set(seen).size, 6);
  // A game never gets fire and firework.
  for (let s = 0; s < 40; s++) {
    const d = new WordDeck(words, seeded(s), [], { known });
    const game: WordEntry[] = [];
    for (let i = 0; i < 7; i++) game.push(d.deal("easy", game));
    const ws = game.map((g) => g.word);
    assert.ok(!(ws.includes("fire") && ws.includes("firework")), ws.join(" "));
    assert.equal(new Set(ws).size, ws.length);
  }
  // Across games (the kept `used`), every word before any repeats.
  let used: string[] = [];
  const all: string[] = [];
  for (let g = 0; g < 4; g++) {
    const d = new WordDeck(words, seeded(100 + g), used, { known });
    all.push(d.deal("easy", []).word, d.deal("easy", []).word);
    used = d.used;
  }
  assert.equal(new Set(all).size, 8, all.join(" "));
  // Difficulty: easy only; hard: no easy word.
  const real = new WordDeck(WORDS, seeded(9), [], { known });
  const diff = (w: string) => WORDS.find((e) => e.word === w)!.difficulty;
  const easy: WordEntry[] = [];
  for (let i = 0; i < 16; i++) easy.push(real.deal("easy", easy));
  assert.ok(easy.every((e) => e.difficulty === "easy"));
  const hard: WordEntry[] = [];
  for (let i = 0; i < 16; i++) hard.push(real.deal("hard", hard));
  assert.ok(hard.every((e) => diff(e.word) !== "easy"));
  const counts: Record<string, number> = { easy: 0, medium: 0, hard: 0 };
  const normal = new WordDeck(WORDS, seeded(10), [], { known });
  for (let i = 0; i < 600; i++) counts[normal.deal("normal", []).difficulty]!++;
  assert.ok(counts["hard"]! > 30 && counts["hard"]! < 100, JSON.stringify(counts));
  assert.ok(counts["medium"]! > counts["easy"]!, JSON.stringify(counts));
});

test("guesser order: join order rotated per game, twice with 3–5 players", () => {
  assert.equal(guessesEach("auto", 3), 2);
  assert.equal(guessesEach("auto", 5), 2);
  assert.equal(guessesEach("auto", 6), 1);
  assert.equal(guessesEach(1, 4), 1);
  assert.equal(guessesEach(2, 8), 2);
  const ids = (t: { id: string }[]) => t.map((x) => x.id).join("");
  assert.equal(ids(guesserOrder(["a", "b", "c"], 0, 2)), "abcabc");
  assert.equal(ids(guesserOrder(["a", "b", "c"], 1, 1)), "bca");
  assert.equal(ids(guesserOrder(["a", "b", "c"], 5, 1)), "cab");
  // A late joiner: at the end of each pass.
  const q = guesserOrder(["a", "b", "c"], 0, 2).slice(1); // a is guessing
  assert.equal(ids(addLate(q, "d", 2)), "bcdabcd");
  // In the second pass already: first after the next guesser, then at the end.
  const q2 = guesserOrder(["a", "b", "c"], 0, 2).slice(4);
  assert.equal(ids(addLate(q2, "d", 2)), "bdcd");
  assert.equal(ids(addLate([], "d", 1)), "d");
});

test("room titles", () => {
  assert.equal(roomTitle(8, 8), "telepaths");
  assert.equal(roomTitle(6, 8), "mind readers");
  assert.equal(roomTitle(4, 8), "in sync");
  assert.equal(roomTitle(2, 8), "mixed signals");
  assert.equal(roomTitle(1, 8), "total jinx");
  assert.equal(roomTitle(0, 0), "total jinx");
});

test("ranks share ties", () => {
  assert.deepEqual(
    rank([
      { id: "a", score: 3 },
      { id: "b", score: 9, delta: 4 },
      { id: "c", score: 3 },
      { id: "d", score: -2 },
    ]),
    [
      { id: "b", score: 9, delta: 4, rank: 1 },
      { id: "a", score: 3, delta: 0, rank: 2 },
      { id: "c", score: 3, delta: 0, rank: 2 },
      { id: "d", score: -2, delta: 0, rank: 4 },
    ],
  );
});

test("MVP of the night and awards", () => {
  const s = (id: string, score: number, mvpClues = 0, jinxed = 0, turnedRight = 0): PlayerStats => ({ id, score, mvpClues, jinxed, turnedRight });
  assert.equal(mvpOfNight([s("a", 10, 1, 0), s("b", 10, 2, 3), s("c", 4)])?.id, "b");
  assert.equal(mvpOfNight([s("a", 10, 2, 2), s("b", 10, 2, 1)])?.id, "b");
  assert.equal(mvpOfNight([s("a", 0), s("b", -2)]), undefined);
  const pairs = new Map([
    [pairKey("b", "a"), 3],
    [pairKey("c", "d"), 3],
    [pairKey("a", "c"), 1],
  ]);
  assert.deepEqual(awards([s("a", 1, 0, 4, 1), s("b", 2, 0, 3, 2), s("c", 0), s("d", 0)], pairs), [
    { kind: "mind meld", ids: ["a", "b"], count: 3 },
    { kind: "lone wolf", ids: ["b"], count: 2 },
    { kind: "kamikaze", ids: ["a"], count: 4 },
  ]);
  assert.deepEqual(awards([s("a", 1, 0, 1, 1)], new Map([[pairKey("a", "b"), 1]])), []);
});

test("the word web links clues used for more than one word", () => {
  const c = (display: string) => ({ display, key: display.toLowerCase().replace(/[^a-z0-9]/g, "") });
  const w = web([
    { word: "volcano", found: true, clues: [c("Lava"), c("hot"), c("LAVA")] },
    { word: "sun", found: false, clues: [c("Hot"), c("sky")] },
    { word: "oven", found: true, clues: [c("hot"), c("bake")] },
  ]);
  assert.deepEqual(w.words, [
    { word: "volcano", found: true, clues: ["Lava", "hot"] },
    { word: "sun", found: false, clues: ["Hot", "sky"] },
    { word: "oven", found: true, clues: ["hot", "bake"] },
  ]);
  assert.deepEqual(w.links, [{ clue: "hot", words: [0, 1, 2] }]);
});

test("faces", () => {
  assert.deepEqual(validateFace({ preset: 3 }), { preset: 3 });
  assert.equal(validateFace({ preset: 8 }), null);
  assert.deepEqual(validateFace({ strokes: [[0, 0, 1200.4, -3, 500.6, 500]] }), { strokes: [[0, 0, 1000, 0, 501, 500]] });
  assert.equal(validateFace({ strokes: [[1, 2, 3]] }), null);
  assert.equal(validateFace({ strokes: Array.from({ length: 41 }, () => [1, 1]) }), null);
  assert.equal(validateFace({ strokes: [Array.from({ length: 4002 }, () => 1)] }), null);
  assert.equal(validateFace("x"), null);
});
