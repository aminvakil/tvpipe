/**
 * Replays the protocol vectors (crates/tvpipe-party/tests/vectors/,
 * written from this implementation by `node test/vectors.ts --bless`):
 * the Rust host replays the same files.
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { applyDiff as clientApply } from "../client/party-client.js";
import { cleanText, MAX_NAME, nameKey } from "../src/names.ts";
import { applyDiff, diff, type Json, type JsonObject } from "../src/sync.ts";
import { all, get, readVectors, runSession, VECTORS } from "./vectors.ts";

test("vectors: diffs, and the client's merge", () => {
  const blocks = readVectors("diff.txt");
  assert.ok(blocks.length > 40);
  for (const b of blocks) {
    const a = JSON.parse(get(b, "a")) as JsonObject;
    const want = JSON.parse(get(b, "b")) as JsonObject;
    const d = diff(a, want);
    const expected = get(b, "diff");
    assert.deepEqual(d ?? "none", expected === "none" ? "none" : JSON.parse(expected), get(b, "name"));
    if (d) {
      assert.deepEqual(applyDiff(a, d), want);
      assert.deepEqual(clientApply(structuredClone(a), JSON.parse(JSON.stringify(d))), want);
    }
  }
});

test("vectors: names", () => {
  for (const b of readVectors("names.txt")) {
    const input = JSON.parse(get(b, "input")) as string;
    const clean = cleanText(input, MAX_NAME);
    assert.equal(clean, JSON.parse(get(b, "clean")), get(b, "name"));
    assert.equal(nameKey(clean), JSON.parse(get(b, "key")), get(b, "name"));
  }
});

test("vectors: sessions", async () => {
  const blocks = readVectors("sessions.txt");
  assert.ok(blocks.length > 20);
  for (const b of blocks) {
    const options = b.entries.some(([k]) => k === "options") ? (JSON.parse(get(b, "options")) as Json as JsonObject) : undefined;
    try {
      await runSession(options, all(b, "step"), false);
    } catch (err) {
      throw new Error(`session "${get(b, "name")}" (line ${b.line}): ${err instanceof Error ? err.message : String(err)}`);
    }
  }
});

test("the browser client is the Rust crate's, byte for byte", () => {
  const here = readFileSync(fileURLToPath(new URL("../client/party-client.js", import.meta.url)));
  const there = readFileSync(`${VECTORS}../../web/party-client.js`);
  assert.ok(here.equals(there), "npm/party/client/party-client.js differs from crates/tvpipe-party/web/party-client.js: copy the canonical one (the crate's)");
});
