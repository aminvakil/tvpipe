import assert from "node:assert/strict";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { test } from "node:test";

import { qr, tunnel, TvpipeError } from "../src/index.ts";
import { opts, realBinary } from "./helpers.ts";

test("qr: rows from the binary, SVG, errors", async () => {
  const code = await qr("https://example.com/?r=BCDF#k=abc", { binary: realBinary() });
  assert.equal(code.size, 29);
  assert.equal(code.rows.length, 29);
  // Finder patterns in three corners.
  assert.equal(code.rows[0]!.slice(0, 7), "1111111");
  assert.equal(code.rows[0]!.slice(-7), "1111111");
  assert.equal(code.rows.at(-1)!.slice(0, 7), "1111111");
  assert.equal(code.dark(0, 0), true);
  assert.equal(code.dark(7, 0), false);
  assert.equal(code.dark(-1, 0), false);
  const svg = code.svg({ quiet: 2 });
  assert.match(svg, /^<svg xmlns="http:\/\/www.w3.org\/2000\/svg" viewBox="0 0 33 33"/);
  assert.match(svg, /<path d="M2 2h7v1h-7z/);
  assert.deepEqual(JSON.parse(JSON.stringify(code)), { size: 29, rows: code.rows });
  await assert.rejects(qr("x".repeat(4000), { binary: realBinary() }), TvpipeError);
});

test("tunnel: the port must be a port", async (t) => {
  await assert.rejects(tunnel({ ...opts(t), port: 0 }), (e: TvpipeError) => e.code === "invalid-argument");
});

// Opens a real public link: TVPIPE_LIVE_TUNNEL=cloudflare|opentunnel.
const live = process.env["TVPIPE_LIVE_TUNNEL"];
test("tunnel: a local server on a public link", { skip: !live && "set TVPIPE_LIVE_TUNNEL=cloudflare|opentunnel" }, async (t) => {
  const server = createServer((req, res) => {
    res.setHeader("Content-Type", "text/plain");
    res.end(`hello from ${req.headers.host}`);
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", () => r()));
  t.after(() => server.close());
  const port = (server.address() as AddressInfo).port;
  const s = await tunnel({ ...opts(t), port, provider: live as "cloudflare" | "opentunnel" });
  t.after(() => s.kill());
  assert.match(s.url, /^https:\/\/[^/]+\/$/);
  assert.equal(s.host, new URL(s.url).host);
  const body = await (await fetch(s.url)).text();
  assert.equal(body, `hello from ${s.host}`);
  const end = await s.stop();
  assert.equal(end.reason, "stopped");
});
