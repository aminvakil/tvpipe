import assert from "node:assert/strict";
import { test } from "node:test";

import {
  alike,
  autoVerdict,
  breakOf,
  chainAt,
  chainSteps,
  cleanGuess,
  Deck,
  fold,
  hunt,
  lastText,
  marks,
  parseList,
  pickLang,
  pickWrecker,
  POINTS,
  says,
  scoreRound,
  seatOf,
  seeded,
  smuggles,
  stem,
  stepKind,
  twistOf,
  validateDrawing,
  type Chain,
} from "../game/rules.ts";

test("folding and stems", () => {
  assert.equal(fold("Piratenoma!"), "piratenoma");
  assert.equal(fold("Größe  Füße"), "grosse fusse");
  assert.equal(fold("  a CAT's   hat "), "a cat s hat");
  assert.equal(fold("Crème brûlée"), "creme brulee");
  assert.equal(stem("pirates"), "pirat");
  assert.equal(stem("pirate"), "pirat");
  assert.equal(stem("piraten"), "pirat");
  assert.equal(stem("boxes"), "box");
  assert.equal(stem("bus"), "bus", "three letters stay");
  assert.equal(stem("cat"), "cat");
});

test("says: words, plurals, compounds", () => {
  const yes: [string, string][] = [
    ["pirate grandma", "pirate"],
    ["two pirates on a boat", "pirate"],
    ["Piratenoma", "Pirat"],
    ["eine Oma mit Piratenhut", "Pirat"],
    ["a ROCKETSHIP", "rocket"],
    ["Schneckenhaus", "Schnecke"],
    ["ein Würstchen", "Würstchen"],
    ["ein wurstchen", "Würstchen"],
    ["kites", "kite"],
    ["the crowned king", "crown"],
  ];
  for (const [text, word] of yes) assert.ok(says(text, word), `${text} says ${word}`);
  const no: [string, string][] = [
    ["a kitten", "kite"],
    ["grandma", "pirate"],
    ["an ant", "pants"],
    ["", "pirate"],
    ["pirate", ""],
    ["a cow", "cowboy"],
  ];
  for (const [text, word] of no) assert.ok(!says(text, word), `${text} does not say ${word}`);
});

test("marks: where a text says a word", () => {
  assert.deepEqual(marks("two Pirates and a pirate-ship", "pirate"), [
    [4, 11],
    [18, 24],
  ]);
  assert.deepEqual(marks("eine Piratenoma", "Pirat"), [[5, 15]]);
  assert.deepEqual(marks("nothing here", "pirate"), []);
});

test("alike: the machine's idea of 'still the same'", () => {
  assert.ok(alike("a cat doing yoga", "cat yoga"));
  assert.ok(alike("a snowman at the beach", "snowmen on holiday"));
  assert.ok(alike("Oma auf dem Skateboard", "Skateboard-Oma"));
  assert.ok(!alike("a cat doing yoga", "a dog sleeping"));
  assert.ok(!alike("a shark in a bathtub", "the end of the world"));
  assert.ok(!alike("Oma auf dem Skateboard", "ein Mann mit Hut"));
});

test("drawings from phones", () => {
  assert.deepEqual(validateDrawing({ strokes: [[16, 0, 0, 1000, 1000], [7, 5, 5]] }), { strokes: [[16, 0, 0, 1000, 1000], [7, 5, 5]] });
  assert.deepEqual(validateDrawing({ strokes: [], extra: 1 }), { strokes: [] });
  for (const bad of [null, {}, { strokes: [[16, 0]] }, { strokes: [[0, 1, 1]] }, { strokes: [[16, 1, 1001]] }, { strokes: [[16, 1.5, 1]] }, { strokes: [["16", 1, 1]] }, { strokes: Array(401).fill([7, 1, 1]) }]) {
    assert.throws(() => validateDrawing(bad), /Bad drawing/, JSON.stringify(bad).slice(0, 40));
  }
  assert.throws(() => validateDrawing({ strokes: Array.from({ length: 10 }, () => [7, ...Array(3400).fill(5)]) }), /too many points/);
});

test("guesses are cleaned", () => {
  assert.equal(cleanGuess("  a   cat\u200b on\ta hat "), "a cat on a hat");
  assert.equal(cleanGuess(42), "");
  assert.equal(cleanGuess("x".repeat(60)).length, 40);
});

test("languages", () => {
  assert.equal(pickLang("de-AT"), "de");
  assert.equal(pickLang("fr"), "en");
  assert.equal(pickLang(undefined), "en");
});

test("data files", () => {
  assert.deepEqual(parseList("# comment\n\na cat\nA Cat\n  a dog  \n"), ["a cat", "a dog"]);
});

test("the seating: every step a permutation, no chain visits anyone twice", () => {
  for (let n = 4; n <= 12; n++) {
    for (const len of [4, 6, 8]) {
      const steps = chainSteps(n, len);
      assert.equal(steps % 2, 0);
      assert.ok(steps <= n && steps <= len && steps >= 4, `${n} players, length ${len}: ${steps}`);
      for (let k = 0; k < steps; k++) {
        const seats = new Set<number>();
        for (let c = 0; c < n; c++) {
          const s = seatOf(c, k, n);
          seats.add(s);
          assert.equal(chainAt(s, k, n), c);
        }
        assert.equal(seats.size, n);
      }
      for (let c = 0; c < n; c++) {
        const visited = new Set(Array.from({ length: steps }, (_, k) => seatOf(c, k, n)));
        assert.equal(visited.size, steps);
        assert.equal(seatOf(c, 0, n), c, "the author draws first");
      }
    }
  }
  assert.equal(chainSteps(5, 6), 4);
  assert.equal(chainSteps(7, 8), 6);
  assert.equal(chainSteps(12, 6), 6);
  assert.equal(chainSteps(3, 6), 2);
  assert.deepEqual([0, 1, 2, 3].map(stepKind), ["draw", "guess", "draw", "guess"]);
});

test("twists rotate when on", () => {
  assert.deepEqual([1, 2, 3, 4].map((r) => twistOf(r, true)), ["clear", "glimpse", "peephole", "clear"]);
  assert.deepEqual([1, 2, 3].map((r) => twistOf(r, false)), ["clear", "clear", "clear"]);
});

test("the Wrecker rotates", () => {
  const rng = seeded(3);
  const times = new Map<string, number>();
  const ids = ["a", "b", "c", "d"];
  for (let i = 0; i < 8; i++) {
    const w = pickWrecker(ids, times, rng)!;
    times.set(w, (times.get(w) ?? 0) + 1);
  }
  assert.deepEqual(ids.map((id) => times.get(id)), [2, 2, 2, 2]);
  assert.equal(pickWrecker([], times, rng), undefined);
});

test("decks deal everything before repeating, and skip what does not fit", () => {
  const d = new Deck(["a", "b", "c"], seeded(1));
  const first = [d.draw(), d.draw(), d.draw()].sort();
  assert.deepEqual(first, ["a", "b", "c"]);
  const e = new Deck(["a", "b", "c"], seeded(2));
  for (let i = 0; i < 10; i++) assert.notEqual(e.draw((c) => c !== "b"), "b");
  assert.equal(new Deck(["x"], seeded(1)).draw(() => false), "x", "nothing fits: any card");
});

const D = { strokes: [[16, 1, 1, 2, 2]] };
/** prompt "a pirate ship"? no: "grandma on a skateboard", seats a b c d e f */
function chain(): Chain {
  return {
    id: 0,
    author: "a",
    prompt: "grandma on a skateboard",
    links: [
      { kind: "draw", by: "a", drawing: D },
      { kind: "guess", by: "b", text: "skateboarding granny" },
      { kind: "draw", by: "c", drawing: D },
      { kind: "guess", by: "d", text: "pirate grandma on wheels" },
      { kind: "draw", by: "e", drawing: D },
      { kind: "guess", by: "f", text: "a parrot in a shopping cart" },
    ],
  };
}

test("smuggled words: after your step, by someone else, first appearance only", () => {
  const c = chain();
  const words = new Map([
    ["c", "pirate"], // drew the parrot: d said pirate → smuggled
    ["d", "parrot"], // wrote nothing of it; f said parrot after d's step → smuggled
    ["e", "pirate"], // pirate came up before e's step → nothing
    ["f", "cart"], // wrote it in their own guess → nothing
    ["b", "skate"], // in the prompt already → nothing
    ["a", "volcano"],
  ]);
  assert.deepEqual(smuggles(c, words), [
    { by: "c", word: "pirate", at: 3 },
    { by: "d", word: "parrot", at: 5 },
  ]);
});

test("verdicts, breaks and the fallback", () => {
  const c = chain();
  assert.equal(lastText(c), "a parrot in a shopping cart");
  assert.deepEqual(breakOf(c, 2), { drawer: "e", guesser: "f", at: 5 });
  assert.deepEqual(breakOf(c, 0), { drawer: "a", guesser: "b", at: 1 });
  assert.equal(breakOf(c, null), null);
  assert.equal(autoVerdict(c), 2, "granny and grandma: still the same; the parrot: not");
  const skipped: Chain = { ...c, links: [...c.links.slice(0, 3), { kind: "guess", by: "d", text: "" }] };
  assert.equal(lastText(skipped), "skateboarding granny", "a skipped guess passes the text before it on");
});

test("the hunt: the Wrecker is caught only with the most votes, alone", () => {
  assert.equal(hunt("w", new Map([["a", "w"], ["b", "w"], ["w", "a"]])).caught, true);
  assert.equal(hunt("w", new Map([["a", "w"], ["b", "a"], ["w", "a"]])).caught, false);
  assert.equal(hunt("w", new Map([["a", "w"], ["w", "a"]])).caught, false, "a tie");
  assert.equal(hunt("w", new Map()).caught, false);
  assert.equal(hunt("w", new Map([["w", "w"]])).votes.size, 0, "nobody votes for themselves");
});

test("a round's points", () => {
  const c = chain();
  const words = new Map([
    ["a", "volcano"],
    ["b", "igloo"],
    ["c", "pirate"],
    ["d", "parrot"],
    ["e", "crown"],
    ["f", "cart"],
  ]);
  // Died at the third guess (e's drawing, f's guess); e is the Wrecker; b and c found them.
  const caught = scoreRound(
    [{ chain: c, died: 2 }],
    words,
    "e",
    new Map([
      ["b", "e"],
      ["c", "e"],
      ["e", "a"],
    ]),
  );
  const L = POINTS.link;
  assert.deepEqual(caught.earned.get("a"), { links: L, smuggles: 0, wrecks: 0, spot: 0, escape: 0 });
  assert.deepEqual(caught.earned.get("b"), { links: L, smuggles: 0, wrecks: 0, spot: POINTS.spot, escape: 0 });
  assert.deepEqual(caught.earned.get("c"), { links: L, smuggles: POINTS.smuggle, wrecks: 0, spot: POINTS.spot, escape: 0 });
  assert.deepEqual(caught.earned.get("d"), { links: L, smuggles: POINTS.smuggle, wrecks: 0, spot: 0, escape: 0 });
  assert.deepEqual(caught.earned.get("e"), { links: 0, smuggles: 0, wrecks: 0, spot: 0, escape: 0 }, "caught: the wreck is void");
  assert.deepEqual(caught.earned.get("f"), { links: 0, smuggles: 0, wrecks: 0, spot: 0, escape: 0 });
  assert.deepEqual(caught.chains[0], { alive: 2, died: 2, break: { drawer: "e", guesser: "f", at: 5 }, smuggles: smuggles(c, words), wrecked: true });
  assert.equal(caught.hunt.caught, true);
  // Not caught: the wreck and the escape.
  const free = scoreRound([{ chain: c, died: 2 }], words, "e", new Map([["b", "a"]]));
  assert.deepEqual(free.earned.get("e"), { links: 0, smuggles: 0, wrecks: POINTS.wreck, spot: 0, escape: POINTS.escape });
  // Survived: every link pays, nobody wrecked.
  const all = scoreRound([{ chain: c, died: null }], words, "e", new Map());
  assert.equal(all.chains[0]!.alive, 3);
  assert.equal(all.earned.get("e")!.links, L);
  assert.equal(all.earned.get("e")!.wrecks, 0);
});
