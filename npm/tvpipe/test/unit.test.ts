import assert from "node:assert/strict";
import { existsSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { camelize, parseEvent } from "../src/events.ts";
import { PLATFORMS, platformPackage, resolveBinary, TvpipeError } from "../src/index.ts";
import * as slrw from "../src/slrw.ts";

test("records have the documented bytes", () => {
  const r = slrw.record(slrw.Kind.video, 0x0102, new Uint8Array([0x61, 0x62]));
  assert.deepEqual([...r], [1, 2, 1, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0x61, 0x62]);
  // Encoded kinds carry a prefix byte (flags, format) in the payload.
  const h = slrw.record(slrw.Kind.h264, 0x10n, new Uint8Array([0, 0, 1, 0x65]), slrw.H264_KEYFRAME | slrw.H264_ANNEX_B);
  assert.deepEqual([...h], [3, 0x10, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 3, 0, 0, 1, 0x65]);
  assert.deepEqual([...slrw.control(1)], [5, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 1, 0, 0, 0]);
  const c = slrw.command({ type: "stop" });
  assert.equal(c[0], 6);
  assert.equal(c.readUInt32LE(9), 15);
  assert.equal(c.subarray(13).toString(), '{"type":"stop"}');
  assert.deepEqual([...slrw.MAGIC], [0x53, 0x4c, 0x52, 0x57]);
  // The payload is copied: callers may reuse their buffers.
  const frame = new Uint8Array([1, 2, 3]);
  const rec = slrw.record(slrw.Kind.video, 0, frame);
  frame[0] = 9;
  assert.equal(rec[13], 1);
  assert.throws(() => slrw.record(slrw.Kind.video, -1, frame), RangeError);
});

test("float PCM becomes s16le", () => {
  const out = slrw.floatToS16(new Float32Array([0, 1, -1, 0.5, 2, -2]));
  const v = new DataView(out.buffer);
  assert.deepEqual(
    [0, 1, 2, 3, 4, 5].map((i) => v.getInt16(i * 2, true)),
    [0, 32767, -32768, 16384, 32767, -32768],
  );
});

test("annex b detection", () => {
  assert.ok(slrw.isAnnexB(new Uint8Array([0, 0, 0, 1, 0x65])));
  assert.ok(slrw.isAnnexB(new Uint8Array([0, 0, 1, 0x65])));
  assert.ok(!slrw.isAnnexB(new Uint8Array([0, 0, 0, 5, 0x65])));
});

test("events are camelCased", () => {
  assert.deepEqual(camelize({ a_b: [{ c_d_e: 1 }], x: "snake_case_value" }), {
    aB: [{ cDE: 1 }],
    x: "snake_case_value",
  });
  const ev = parseEvent('{"type":"stats","elapsed_ms":5,"receivers":[{"name":"tv","dropped_full":1}]}');
  assert.deepEqual(ev, { type: "stats", elapsedMs: 5, receivers: [{ name: "tv", droppedFull: 1 }] });
  assert.equal(parseEvent("not json"), undefined);
  assert.equal(parseEvent("{broken"), undefined);
  assert.equal(parseEvent('{"no":"type"}'), undefined);
});

test("binary resolution", () => {
  assert.equal(platformPackage("linux-x64"), "@tvpipe/linux-x64");
  assert.equal(PLATFORMS.length, 6);
  const saved = process.env["TVPIPE_BINARY"];
  try {
    delete process.env["TVPIPE_BINARY"];
    // An explicit path that does not exist.
    assert.throws(
      () => resolveBinary({ binary: "/nonexistent/tvpipe" }),
      (e: unknown) => e instanceof TvpipeError && e.code === "binary-missing" && /binary option/.test(e.message),
    );
    process.env["TVPIPE_BINARY"] = "/nonexistent/tvpipe";
    assert.throws(
      () => resolveBinary(),
      (e: unknown) => e instanceof TvpipeError && /TVPIPE_BINARY/.test(e.message),
    );
    const dir = mkdtempSync(join(tmpdir(), "tvpipe-bin-"));
    const fake = join(dir, "tvpipe");
    writeFileSync(fake, "");
    process.env["TVPIPE_BINARY"] = fake;
    assert.equal(resolveBinary(), fake);
    assert.equal(resolveBinary({ binary: fake }), fake);
    // No platform package installed in this checkout: a helpful error.
    delete process.env["TVPIPE_BINARY"];
    const installed = existsSync(join(import.meta.dirname, "..", "node_modules", "@tvpipe"));
    if (!installed) {
      assert.throws(
        () => resolveBinary(),
        (e: unknown) =>
          e instanceof TvpipeError &&
          (e.code === "binary-missing" || e.code === "unsupported-platform") &&
          /TVPIPE_BINARY/.test(e.message),
      );
    }
  } finally {
    if (saved === undefined) delete process.env["TVPIPE_BINARY"];
    else process.env["TVPIPE_BINARY"] = saved;
  }
});
