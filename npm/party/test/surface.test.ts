import assert from "node:assert/strict";
import { test } from "node:test";

import { codecs, slrwRecord, wallMicros } from "../src/surface.ts";

test("surface records are SLRW: kind, pts (µs, LE), length, payload", () => {
  const r = slrwRecord(3, 0x0102030405, new Uint8Array([9, 8]), 1);
  assert.deepEqual([...r], [3, 5, 4, 3, 2, 1, 0, 0, 0, 3, 0, 0, 0, 1, 9, 8]);
  const a = slrwRecord(2, 1.6, new Uint8Array([1, 2, 3, 4]));
  assert.equal(a.length, 17);
  assert.equal(a[1], 2, "pts rounded");
  assert.equal(new DataView(a.buffer).getUint32(9, true), 4);
  // The wall clock in µs (what tvpipe compares the lag against).
  assert.ok(Math.abs(wallMicros() / 1000 - Date.now()) < 50);
});

test("surface codecs ask for the H.264 level of their size and rate", () => {
  const level = (w: number, h: number, fps: number) => codecs(w, h, fps)[0]!.slice(-2);
  assert.deepEqual(codecs(1280, 720, 30), ["avc1.42E01F", "avc1.4D401F", "avc1.64001F"]);
  assert.equal(level(1920, 1080, 30), "28");
  assert.equal(level(1920, 1080, 60), "2A");
  assert.equal(level(3840, 2160, 30), "33");
  assert.equal(level(3840, 2160, 60), "34");
  // Larger than any level: the largest.
  assert.equal(level(7680, 4320, 30), "34");
});
