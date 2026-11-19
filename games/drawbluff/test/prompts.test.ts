import assert from "node:assert/strict";
import { test } from "node:test";

import { DECK_RESERVE, displayTitle, LANGS, LIE_SOFT, PromptDeck, seeded, titleKey, type Prompt } from "../game/rules.ts";
import { nearDuplicates, perStructure, repeats } from "../scripts/prompt-stats.ts";
import { PROMPTS_BY_LANG } from "./prompts.ts";

/** What a prompt may contain, per language (its display form, so lowercase). */
const LETTERS = { en: /^[a-z' ]+$/, de: /^[a-zäöüß' ]+$/ } as const;
/** The longest word a prompt may have: the phone's prompt and the TV's options break it, but not nicely. */
const LONGEST_WORD = 26;

for (const lang of LANGS) {
  const prompts = PROMPTS_BY_LANG[lang];

  test(`prompts (${lang}): display form, at most ${LIE_SOFT} characters, nothing a lie could not be`, () => {
    for (const { text } of prompts) {
      assert.equal(displayTitle(text), text, text);
      assert.equal(text.normalize("NFC"), text, `${text}: not NFC`);
      assert.ok([...text].length <= LIE_SOFT, `${text}: ${[...text].length} characters`);
      assert.match(text, LETTERS[lang], text);
      assert.ok(!/^ | $|  /.test(text), `${text}: spacing`);
      for (const w of text.split(" ")) assert.ok([...w].length <= LONGEST_WORD, `${text}: ${w} is too long a word`);
    }
  });

  test(`prompts (${lang}): no two alike, not even nearly`, () => {
    const keys = new Map<string, string>();
    for (const { text } of prompts) {
      const k = titleKey(text, lang);
      assert.ok(!keys.has(k), `${text} = ${keys.get(k)}`);
      keys.set(k, text);
    }
    assert.deepEqual(nearDuplicates(prompts, lang), []);
  });

  test(`prompts (${lang}): many, in balanced structures`, () => {
    assert.ok(prompts.length >= 600, String(prompts.length));
    assert.ok(prompts.every((p) => /^[a-z]+$/.test(p.tag)), "every prompt has a structure");
    const tags = perStructure(prompts);
    assert.ok(tags.size >= 15, `${tags.size} structures`);
    for (const [tag, n] of tags) {
      assert.ok(n >= 30, `${tag}: ${n}`);
      assert.ok(n <= prompts.length / 10, `${tag}: ${n} of ${prompts.length}`);
    }
  });

  test(`prompts (${lang}): a room plays 5 games of 8 players, 2 rounds, without a repeat`, () => {
    const s = { games: 5, players: 8, rounds: 2 };
    assert.deepEqual(repeats(prompts, s, false, 20), { mean: 0, sessions: 0 });
    // A fresh deck per game (as before) would repeat some.
    assert.ok(repeats(prompts, s, true, 20).mean > 0);
  });
}

test("the room's deck: no prompt twice until all were dealt, decoys out of the pass, the rest first", () => {
  const prompts: Prompt[] = Array.from({ length: 50 }, (_, i) => ({ text: `p${i}`, tag: ["a", "b", "c"][i % 3]! }));
  const d = new PromptDeck(prompts, seeded(3));
  const dealt: string[] = [];
  for (let h = 0; h < 20; h++) {
    const left = d.left.map((p) => p.text);
    const hand = d.deal(7).map((p) => p.text);
    // What was left comes first.
    assert.deepEqual(hand.slice(0, Math.min(7, left.length)), left.slice(0, 7));
    assert.ok(d.left.length >= DECK_RESERVE, "decoys to come from");
    dealt.push(...hand);
  }
  assert.equal(new Set(dealt.slice(0, 50)).size, 50, "the first 50 are all of them");
  // Again only after a whole pass: the 50 but those left when it ended (a reserve at least).
  const last = new Map<string, number>();
  dealt.forEach((t, i) => {
    if (last.has(t)) assert.ok(i - last.get(t)! > DECK_RESERVE, `${t} again after ${i - last.get(t)!}`);
    last.set(t, i);
  });
  // A decoy shown is not dealt in this pass.
  const decoy = d.left[1]!.text;
  d.discard([decoy]);
  assert.ok(!d.left.some((p) => p.text === decoy));
  // Fewer prompts than a hand: they repeat rather than run out.
  const tiny = new PromptDeck(prompts.slice(0, 3), seeded(1));
  assert.equal(tiny.deal(8).length, 8);
  assert.throws(() => new PromptDeck([], seeded(1)));
});
