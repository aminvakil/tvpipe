import assert from "node:assert/strict";
import { request } from "node:http";
import { connect } from "node:net";
import { test } from "node:test";

import { setup, until } from "./helpers.ts";

/** A raw HTTP request (any Host, path bytes as given). */
function raw(port: number, text: string): Promise<string> {
  return new Promise((resolve) => {
    const s = connect(port, "127.0.0.1");
    let out = "";
    s.on("data", (d) => (out += d.toString("latin1")));
    s.on("close", () => resolve(out));
    s.on("error", () => resolve(out));
    s.write(text);
    setTimeout(() => s.end(), 300);
  });
}

function get(port: number, path: string, headers: Record<string, string> = {}, method = "GET") {
  return new Promise<{ status: number; headers: Record<string, string | string[] | undefined>; body: string }>((resolve, reject) => {
    const req = request({ host: "127.0.0.1", port, path, method, headers }, (res) => {
      let body = "";
      res.on("data", (d) => (body += d));
      res.on("end", () => resolve({ status: res.statusCode ?? 0, headers: res.headers, body }));
    });
    req.on("error", reject);
    req.end();
  });
}

test("static files: pages, client, MIME, HEAD, ETag, CSP; nothing outside", async () => {
  const s = await setup();
  const port = s.server.port;
  try {
    let r = await get(port, "/?r=ABCD");
    assert.equal(r.status, 200);
    assert.equal(r.body, "<!doctype html><title>phone</title>");
    assert.match(String(r.headers["content-type"]), /text\/html/);
    assert.match(String(r.headers["content-security-policy"]), /default-src 'self'/);
    assert.equal(r.headers["x-content-type-options"], "nosniff");
    const etag = String(r.headers["etag"]);
    assert.equal((await get(port, "/", { "If-None-Match": etag })).status, 304);
    r = await get(port, "/app.js");
    assert.match(String(r.headers["content-type"]), /text\/javascript/);
    assert.equal(r.headers["content-security-policy"], undefined);
    r = await get(port, "/app.js", {}, "HEAD");
    assert.equal(r.status, 200);
    assert.equal(r.body, "");
    assert.equal(r.headers["content-length"], "20");
    r = await get(port, "/screen/");
    assert.equal(r.body, "<!doctype html><title>screen</title>");
    r = await get(port, "/screen");
    assert.equal(r.status, 301);
    r = await get(port, "/sub");
    assert.equal(r.status, 301);
    assert.equal(r.headers["location"], "/sub/");
    r = await get(port, "/party/client.js");
    assert.equal(r.status, 200);
    assert.match(r.body, /export class PartyClient/);
    for (const p of ["/.secret", "/%2e%2e/outside.txt", "/sub/%2e%2e/%2e%2e/outside.txt", "/..%5coutside.txt", "/%00", "/nope.html", "/party/nope"]) {
      assert.equal((await get(port, p)).status, 404, p);
    }
    const t = await raw(port, `GET /../outside.txt HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\nConnection: close\r\n\r\n`);
    assert.doesNotMatch(t, /outside/);
    assert.equal((await get(port, "/%E0%A4%A")).status, 400);
    assert.equal((await get(port, "/", {}, "POST")).status, 405);
  } finally {
    await s.close();
  }
});

test("Host: loopback names and allowed public hosts only", async () => {
  const s = await setup();
  const port = s.server.port;
  try {
    assert.equal((await get(port, "/", { Host: `localhost:${port}` })).status, 200);
    assert.equal((await get(port, "/", { Host: "evil.example" })).status, 421);
    assert.equal((await get(port, "/", { Host: `127.0.0.1:${port + 1}` })).status, 421);
    assert.equal((await get(port, "/", { Host: "words.trycloudflare.com" })).status, 421);
    s.server.allowHost("Words.trycloudflare.com");
    assert.equal((await get(port, "/", { Host: "words.trycloudflare.com" })).status, 200);
    assert.equal((await get(port, "/", { Host: "words.trycloudflare.com:443" })).status, 200);
  } finally {
    await s.close();
  }
});

function upgrade(port: number, headers: Record<string, string>): Promise<string> {
  const h = {
    Host: `127.0.0.1:${port}`,
    Upgrade: "websocket",
    Connection: "Upgrade",
    "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==",
    "Sec-WebSocket-Version": "13",
    ...headers,
  };
  const text = `GET /party/ws HTTP/1.1\r\n${Object.entries(h)
    .map(([k, v]) => `${k}: ${v}`)
    .join("\r\n")}\r\n\r\n`;
  return raw(port, text).then((t) => t.split("\r\n")[0] ?? "");
}

test("WebSocket Origin and Host checks", async () => {
  const s = await setup();
  const port = s.server.port;
  try {
    assert.match(await upgrade(port, {}), / 101 /);
    assert.match(await upgrade(port, { Origin: `http://127.0.0.1:${port}` }), / 101 /);
    assert.match(await upgrade(port, { Origin: `http://localhost:${port}` }), / 101 /);
    assert.match(await upgrade(port, { Origin: "https://evil.example" }), / 403 /);
    assert.match(await upgrade(port, { Origin: `http://127.0.0.1:${port}.evil.example` }), / 403 /);
    assert.match(await upgrade(port, { Host: "evil.example" }), / 421 /);
    s.server.allowHost("pub.example");
    assert.match(await upgrade(port, { Host: "pub.example", Origin: "https://pub.example" }), / 101 /);
    assert.match(await upgrade(port, { Host: "pub.example", Origin: "http://pub.example" }), / 403 /);
  } finally {
    await s.close();
  }
});

test("failed hellos from one address are throttled", async () => {
  const s = await setup({ server: { joinsPerMinute: 3 } });
  try {
    const { PartyClient, memoryStorage } = await import("../client/party-client.js");
    for (let i = 0; i < 3; i++) {
      const c = new PartyClient({ url: s.ws, room: s.party.code, key: "wrong", storage: memoryStorage() });
      await assert.rejects(c.join("x"));
    }
    assert.match(await upgrade(s.server.port, {}), / 429 /);
  } finally {
    await s.close();
  }
});

test("a socket without hello is closed; garbage hello is refused", async () => {
  const s = await setup();
  try {
    const ws = new WebSocket(s.ws);
    const closes: number[] = [];
    ws.onclose = (e) => closes.push(e.code);
    await new Promise((r) => (ws.onopen = r));
    ws.send("not json");
    await until(() => closes.length === 1);
    assert.equal(closes[0], 4002);
  } finally {
    await s.close();
  }
});
