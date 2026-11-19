import assert from "node:assert/strict";
import { test } from "node:test";

import { PartyError } from "@tvpipe/party";

import { PROMPTS } from "../game/prompts.ts";
import { buildOptions, cleanTitle, optionsFor, revealOrder, score, seeded, titleKey, validateDrawing } from "../game/rules.ts";

test("titles compare without case, accents, punctuation and a leading article", () => {
  assert.equal(titleKey("A Nervous  Cactus!"), titleKey("nervous cactus"));
  assert.equal(titleKey("The café"), titleKey("cafe"));
  assert.notEqual(titleKey("a cactus"), titleKey("a cactuses"));
  assert.equal(cleanTitle("  hi\u0000 there  "), "hi there");
  assert.equal(cleanTitle("x".repeat(80)).length, 40);
  assert.equal(new Set(PROMPTS.map(titleKey)).size, PROMPTS.length, "prompts are distinct");
});

test("drawings: limits, clamping, refusals", () => {
  assert.deepEqual(validateDrawing({ strokes: [[1, 500, -5, 2000, 10.4, 20.6]] }), { strokes: [[1, 120, 0, 1000, 10, 21]] });
  assert.deepEqual(validateDrawing({ strokes: [] }), { strokes: [] });
  for (const bad of [null, {}, { strokes: [[1, 2, 3]] }, { strokes: [[9, 2, 3, 4]] }, { strokes: [[1, 2, "x", 4]] }, { strokes: [[1, 2, NaN, 4]] }, { strokes: Array(301).fill([0, 4, 1, 1]) }, { strokes: [[0, 4, ...Array(24002).fill(1)]] }]) {
    assert.throws(() => validateDrawing(bad), (e: unknown) => e instanceof PartyError && e.code === "bad-drawing", JSON.stringify(bad)?.slice(0, 50));
  }
});

test("options: lies merged, truth-equal lies dropped, decoys fill up, shuffled", () => {
  const lies = new Map([
    ["p2", "A spooky toaster"],
    ["p3", "spooky toaster!"],
    ["p4", "a haunted toaster"], // the truth, typed anyway
    ["p5", "a toaster in love"],
  ]);
  const opts = buildOptions("a haunted toaster", lies, ["a sad potato", "a toaster in love", "a lonely sock"], seeded(1), 5);
  const texts = opts.map((o) => o.text).sort();
  assert.deepEqual(texts, ["A spooky toaster", "a haunted toaster", "a lonely sock", "a sad potato", "a toaster in love"]);
  const merged = opts.find((o) => o.text === "A spooky toaster")!;
  assert.deepEqual(merged.authors, ["p2", "p3"]);
  assert.equal(opts.filter((o) => o.truth).length, 1);
  assert.equal(opts.filter((o) => o.decoy).length, 2);
  assert.equal(new Set(opts.map((o) => o.id)).size, opts.length);
  // A voter never sees their own lie.
  assert.ok(!optionsFor("p3", opts).some((o) => o.text === "A spooky toaster"));
  assert.equal(optionsFor("p9", opts).length, 5);
});

test("scoring: guessers, the artist, liars, decoys, round 2 doubles, own lies and the artist ignored", () => {
  const opts = [
    { id: "t", text: "truth", truth: true, authors: [], decoy: false },
    { id: "l1", text: "lie1", truth: false, authors: ["b", "c"], decoy: false },
    { id: "l2", text: "lie2", truth: false, authors: ["d"], decoy: false },
    { id: "d", text: "decoy", truth: false, authors: [], decoy: true },
  ];
  const votes = new Map([
    ["b", "t"], // correct
    ["c", "l2"], // fooled by d
    ["d", "l1"], // fooled by b and c
    ["e", "d"], // a decoy: nobody scores
    ["a", "t"], // the artist: ignored
    ["f", "nope"], // unknown option: ignored
  ]);
  const r1 = score("a", opts, votes, 1);
  assert.deepEqual(Object.fromEntries(r1.points), { b: 1000 + 500, a: 1000, d: 500, c: 500 });
  assert.deepEqual(r1.voters.get("t"), ["b"]);
  const r2 = score("a", opts, votes, 2);
  assert.equal(r2.points.get("b"), 3000);
  // Voting for your own lie counts for nothing.
  const own = score("a", opts, new Map([["d", "l2"]]), 1);
  assert.equal(own.points.size, 0);
  // Reveal: lies with votes, fewest first, then the truth.
  // Reveal: picked lies and decoys, fewest votes first, then the truth.
  assert.deepEqual(revealOrder(opts, r1.voters).map((o) => o.id), ["l1", "l2", "d", "t"]);
  assert.deepEqual(revealOrder(opts, new Map()).map((o) => o.id), ["t"]);
});
