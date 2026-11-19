import assert from "node:assert/strict";
import { test } from "node:test";

import { PartyError } from "@tvpipe/party";

import { PROMPTS } from "../game/prompts.ts";
import { buildOptions, cleanTitle, deal, optionsFor, pickLiars, plan, revealOrder, score, seeded, share, titleKey, validateDrawing, type Option } from "../game/rules.ts";

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
  const all = new Map(["a", "b", "c", "d", "e", "f"].map((p) => [p, optionsFor(p, opts).map((o) => o.id)]));
  const r1 = score("a", opts, all, votes, 1);
  // b and c wrote the same lie: 100 each for the idea.
  assert.deepEqual(Object.fromEntries(r1.points), { b: 1000 + 500 + 100, a: 1000, d: 500, c: 500 + 100 });
  assert.deepEqual(r1.voters.get("t"), ["b"]);
  assert.equal(r1.seen.get("t"), 4, "b, c, d, e voted");
  const r2 = score("a", opts, all, votes, 2);
  assert.equal(r2.points.get("b"), 3200);
  // Voting for your own lie, or for one your phone did not show, counts for nothing.
  const own = score("a", opts, all, new Map([["d", "l2"]]), 1);
  assert.deepEqual(Object.fromEntries(own.points), { b: 100, c: 100 });
  const hidden = score("a", opts, new Map([["e", ["t", "d"]]]), new Map([["e", "l1"]]), 1);
  assert.equal(hidden.voters.get("l1")!.length, 0);
  // Reveal: lies with votes, fewest first, then the truth.
  // Reveal: picked lies and decoys, fewest votes first, then the truth.
  assert.deepEqual(revealOrder(opts, r1.voters).map((o) => o.id), ["l1", "l2", "d", "t"]);
  assert.deepEqual(revealOrder(opts, new Map()).map((o) => o.id), ["t"]);
});

test("scale: the plan per player count, today's game up to 8", () => {
  assert.deepEqual(plan(3), { spotlights: 3, liars: 2, pipelined: false });
  assert.deepEqual(plan(8), { spotlights: 6, liars: 7, pipelined: false });
  assert.deepEqual(plan(12), { spotlights: 6, liars: 7, pipelined: false });
  assert.deepEqual(plan(13), { spotlights: 6, liars: 6, pipelined: true });
  assert.deepEqual(plan(20), { spotlights: 6, liars: 7, pipelined: true });
  assert.deepEqual(plan(50), { spotlights: 6, liars: 7, pipelined: true });
  for (let n = 13; n <= 60; n++) {
    const p = plan(n);
    // The next liars come from everybody but the next artist and the current liars.
    assert.ok(n - 1 - p.liars >= p.liars, String(n));
  }
});

test("scale: points per exposure, today's numbers up to 7 voters", () => {
  assert.equal(share(3, 500, 7), 1500);
  assert.equal(share(14, 500, 30), 1633);
  assert.equal(share(20, 1000, 43), 3256);
  assert.equal(share(0, 500, 40), 0);
});

test("scale: liars rotate, the first one first", () => {
  const pool = ["a", "b", "c", "d", "e"];
  const lied = new Map([["a", 2], ["b", 1], ["c", 0], ["d", 1], ["e", 0]]);
  const two = pickLiars(pool, lied, 2, seeded(1));
  assert.deepEqual([...two].sort(), ["c", "e"]);
  const withA = pickLiars(pool, lied, 3, seeded(1), "a");
  assert.equal(withA[0], "a");
  assert.deepEqual(withA.slice(1).sort(), ["c", "e"]);
  assert.deepEqual(pickLiars(pool, lied, 2, seeded(1), "zed").sort(), ["c", "e"]);
});

test("scale: samples are fair and never show a voter's own lie", () => {
  const opts: Option[] = [{ id: "t", text: "truth", truth: true, authors: [], decoy: false }];
  for (let i = 1; i <= 7; i++) opts.push({ id: `l${i}`, text: `lie ${i}`, truth: false, authors: [`liar${i}`], decoy: false });
  const voters = [...Array.from({ length: 43 }, (_, i) => `v${i}`), "liar1", "liar2"];
  const shown = deal(opts, voters, 5, seeded(3));
  const count = new Map<string, number>();
  for (const [v, ids] of shown) {
    assert.ok(ids.includes("t"));
    assert.equal(ids.length, 6, v);
    if (v.startsWith("liar")) assert.ok(!ids.includes(`l${v.slice(4)}`));
    // In the options' order.
    assert.deepEqual(ids, opts.map((o) => o.id).filter((id) => ids.includes(id)));
    for (const id of ids) count.set(id, (count.get(id) ?? 0) + 1);
  }
  const lies = [...count].filter(([id]) => id !== "t").map(([, n]) => n);
  assert.ok(Math.max(...lies) - Math.min(...lies) <= 1, JSON.stringify(lies));
  // Few lies: everybody sees all of them.
  assert.deepEqual(deal(opts.slice(0, 4), ["x"], 5, seeded(1)).get("x"), ["t", "l1", "l2", "l3"]);
});
