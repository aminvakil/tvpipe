import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { connect, type Socket } from "node:net";
import { test } from "node:test";

import { accept, acceptKey, encodeFrame, type WsConnection } from "../src/ws.ts";
import { until } from "./helpers.ts";

/** An echo server built on `accept`. */
async function echoServer(maxMessage = 1024): Promise<{ server: Server; port: number; conns: WsConnection[]; closes: number[] }> {
  const conns: WsConnection[] = [];
  const closes: number[] = [];
  const server = createServer((_q, r) => r.end());
  server.on("upgrade", (req, socket, head) => {
    const ws = accept(req, socket, head, { maxMessage });
    if (!ws) return;
    conns.push(ws);
    ws.on("message", (d, binary) => ws.send(binary ? (d as Buffer) : `echo:${d as string}`));
    ws.on("close", (code) => closes.push(code));
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", () => r()));
  return { server, port: (server.address() as AddressInfo).port, conns, closes };
}

/** A raw client: the handshake, then bytes. */
async function raw(port: number): Promise<{ sock: Socket; data: () => Buffer }> {
  const sock = connect(port, "127.0.0.1");
  let buf = Buffer.alloc(0);
  sock.on("data", (d) => (buf = Buffer.concat([buf, d])));
  sock.on("error", () => {});
  await new Promise((r) => sock.once("connect", r));
  const key = randomBytes(16).toString("base64");
  sock.write(
    `GET / HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: ${key}\r\nSec-WebSocket-Version: 13\r\n\r\n`,
  );
  await until(() => buf.includes("\r\n\r\n"));
  const head = buf.subarray(0, buf.indexOf("\r\n\r\n")).toString();
  assert.match(head, /^HTTP\/1.1 101/);
  assert.ok(head.includes(`Sec-WebSocket-Accept: ${acceptKey(key)}`));
  buf = buf.subarray(buf.indexOf("\r\n\r\n") + 4);
  return { sock, data: () => buf };
}

const mask = () => randomBytes(4);

/** Parses unmasked server frames. */
function frames(b: Buffer): { op: number; payload: Buffer }[] {
  const out = [];
  let i = 0;
  while (i + 2 <= b.length) {
    let len = b[i + 1]! & 0x7f;
    let off = i + 2;
    if (len === 126) {
      len = b.readUInt16BE(off);
      off += 2;
    } else if (len === 127) {
      len = Number(b.readBigUInt64BE(off));
      off += 8;
    }
    if (off + len > b.length) break;
    out.push({ op: b[i]! & 0x0f, payload: b.subarray(off, off + len) });
    i = off + len;
  }
  return out;
}

test("RFC 6455 §1.3 accept key", () => {
  assert.equal(acceptKey("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
});

test("echo with Node's WebSocket client, text and binary, 100 KiB", async () => {
  const s = await echoServer(1 << 20);
  try {
    const ws = new WebSocket(`ws://127.0.0.1:${s.port}/`);
    ws.binaryType = "arraybuffer";
    const got: unknown[] = [];
    ws.onmessage = (e) => got.push(e.data);
    await new Promise((r) => (ws.onopen = r));
    ws.send("héllo");
    const big = randomBytes(100 * 1024);
    ws.send(big);
    await until(() => got.length === 2);
    assert.equal(got[0], "echo:héllo");
    assert.deepEqual(Buffer.from(got[1] as ArrayBuffer), big);
    ws.close(1000);
    await until(() => s.closes.length === 1);
    assert.equal(s.closes[0], 1000);
  } finally {
    s.server.close();
  }
});

test("fragments, interleaved ping, close handshake", async () => {
  const s = await echoServer();
  try {
    const c = await raw(s.port);
    c.sock.write(encodeFrame(0x1, Buffer.from("ab"), false, mask()));
    c.sock.write(encodeFrame(0x9, Buffer.from("p"), true, mask()));
    c.sock.write(encodeFrame(0x0, Buffer.from("cd"), true, mask()));
    await until(() => frames(c.data()).length === 2);
    const f = frames(c.data());
    assert.deepEqual([f[0]!.op, f[0]!.payload.toString()], [0xa, "p"]);
    assert.deepEqual([f[1]!.op, f[1]!.payload.toString()], [0x1, "echo:abcd"]);
    const close = Buffer.alloc(2);
    close.writeUInt16BE(1000);
    c.sock.write(encodeFrame(0x8, close, true, mask()));
    await until(() => frames(c.data()).length === 3);
    assert.equal(frames(c.data())[2]!.op, 0x8);
    assert.equal(frames(c.data())[2]!.payload.readUInt16BE(0), 1000);
  } finally {
    s.server.close();
  }
});

async function violation(write: (sock: Socket) => void, code: number): Promise<void> {
  const s = await echoServer();
  try {
    const c = await raw(s.port);
    write(c.sock);
    await until(() => frames(c.data()).some((f) => f.op === 0x8), 2000, `close ${code}`);
    const f = frames(c.data()).find((x) => x.op === 0x8)!;
    assert.equal(f.payload.readUInt16BE(0), code);
    await until(() => s.closes.length === 1);
  } finally {
    s.server.close();
  }
}

test("violations close with the right code and never throw", async () => {
  await violation((s) => s.write(encodeFrame(0x1, Buffer.from("x"))), 1002); // unmasked
  await violation((s) => s.write(encodeFrame(0x1, Buffer.from([0xff, 0xfe]), true, mask())), 1007);
  await violation((s) => s.write(encodeFrame(0x2, Buffer.alloc(2000), true, mask())), 1009);
  await violation((s) => s.write(encodeFrame(0x0, Buffer.from("x"), true, mask())), 1002); // continuation first
  await violation((s) => s.write(encodeFrame(0x3, Buffer.from("x"), true, mask())), 1002); // reserved opcode
  await violation((s) => s.write(encodeFrame(0x9, Buffer.alloc(126), true, mask())), 1002); // big control
  await violation((s) => {
    const f = encodeFrame(0x1, Buffer.from("x"), true, mask());
    f[0]! |= 0x40; // RSV1
    s.write(f);
  }, 1002);
  await violation((s) => {
    // A 64-bit length beyond the limit is refused before any payload.
    const h = Buffer.from([0x82, 0xff, 0, 0, 0, 1, 0, 0, 0, 0]);
    s.write(h);
  }, 1009);
});

test("garbage after the handshake does not crash", async () => {
  const s = await echoServer();
  try {
    for (let i = 0; i < 50; i++) {
      const c = await raw(s.port);
      c.sock.write(randomBytes(1 + (i * 37) % 300));
      c.sock.destroy();
    }
    // Still serving.
    const ws = new WebSocket(`ws://127.0.0.1:${s.port}/`);
    const got: unknown[] = [];
    ws.onmessage = (e) => got.push(e.data);
    await new Promise((r) => (ws.onopen = r));
    ws.send("ok");
    await until(() => got.length === 1);
    ws.close();
  } finally {
    s.server.close();
  }
});
