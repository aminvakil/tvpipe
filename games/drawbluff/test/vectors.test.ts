import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { PartyError } from "@tvpipe/party";

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
  type Option,
} from "../game/rules.ts";
import { drawingInput, get, has, parseVectors, RULES } from "./vectors.ts";

test("the shared rule vectors (test/vectors/rules.txt)", () => {
  const blocks = parseVectors(readFileSync(RULES, "utf8"));
  const seen = new Map<string, number>();
  for (const b of blocks) {
    const kind = get(b, "test");
    seen.set(kind, (seen.get(kind) ?? 0) + 1);
    const at = `rules.txt:${b.line}`;
    const json = (key: string) => JSON.parse(get(b, key));
    switch (kind) {
      case "rng": {
        const r = seeded(Number(get(b, "seed")));
        assert.deepEqual(json("values"), (json("values") as number[]).map(() => r() * 2 ** 32), at);
        break;
      }
      case "shuffle": {
        const n = Number(get(b, "n"));
        assert.deepEqual(shuffle(Array.from({ length: n }, (_, i) => i), seeded(Number(get(b, "seed")))), json("order"), at);
        break;
      }
      case "title":
        assert.equal(cleanTitle(json("input")), json("clean"), at);
        assert.equal(titleKey(json("input")), json("key"), at);
        break;
      case "drawing": {
        const input = drawingInput(b);
        if (has(b, "error")) {
          assert.throws(() => validateDrawing(input), (e: unknown) => e instanceof PartyError && e.code === get(b, "error"), at);
        } else {
          const out = get(b, "output");
          assert.deepEqual(validateDrawing(input), out === "input" ? input : JSON.parse(out), at);
        }
        break;
      }
      case "turn": {
        const lies = new Map(json("lies") as [string, string][]);
        const options = buildOptions(json("truth"), lies, json("spares"), seeded(Number(get(b, "seed"))), Number(get(b, "min")));
        assert.deepEqual(options, json("options"), at);
        const shown = json("shown") as Record<string, string[]>;
        for (const [p, ids] of Object.entries(shown)) assert.deepEqual(optionsFor(p, options).map((o) => o.id), ids, at);
        const scoring = score(json("artist"), options, new Map(json("votes") as [string, string][]), Number(get(b, "round")));
        assert.deepEqual(Object.fromEntries(scoring.points), json("points"), at);
        assert.deepEqual(Object.fromEntries(scoring.voters), json("voters"), at);
        assert.deepEqual(revealOrder(options as Option[], scoring.voters).map((o) => o.id), json("reveal"), at);
        break;
      }
      default:
        assert.fail(`${at}: unknown test ${kind}`);
    }
  }
  for (const kind of ["rng", "shuffle", "title", "drawing", "turn"]) assert.ok((seen.get(kind) ?? 0) > 0, kind);
});
