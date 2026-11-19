// The client's continuous input and its WebRTC fast path. Node hosts do
// not offer WebRTC, so the upgrade runs against a scripted host (a fake
// WebSocket) and a fake RTCPeerConnection; the real thing is checked
// against the Rust host with Chrome (scripts/live/party-rtc.ts).

import assert from "node:assert/strict";
import { after, test } from "node:test";

import { memoryStorage, PartyClient } from "../client/party-client.js";
import type { Player } from "../src/party.ts";
import { setup, until } from "./helpers.ts";

test("input: a Node host gets it over the WebSocket, latest wins, at most every 100 ms", async () => {
  const s = await setup();
  try {
    const got: unknown[] = [];
    s.party.on("input", (p: Player, data) => got.push([p.name, data]));
    const a = s.phone();
    await a.join("Ann");
    assert.equal(a.transport, "ws");
    for (let i = 1; i <= 10; i++) a.input({ x: i });
    await until(() => got.some((g) => JSON.stringify(g) === JSON.stringify(["Ann", { x: 10 }])), 2000);
    assert.deepEqual(got, [
      ["Ann", { x: 1 }],
      ["Ann", { x: 10 }],
    ]);
    assert.deepEqual(s.party.players[0]!.input, { x: 10 });
  } finally {
    await s.close();
  }
});

/** A host in a fake WebSocket: answers `hello` (offering WebRTC) and records the rest. */
function scriptedHost(offer: boolean) {
  const sent: any[] = [];
  let socket: FakeWs | undefined;
  class FakeWs {
    readyState = 0;
    onopen: (() => void) | null = null;
    onmessage: ((ev: { data: string }) => void) | null = null;
    onclose: ((ev: { reason: string }) => void) | null = null;
    constructor(_url: string) {
      socket = this;
      setTimeout(() => {
        this.readyState = 1;
        this.onopen?.();
      });
    }
    send(text: string) {
      const msg = JSON.parse(text);
      sent.push(msg);
      if (msg.t === "hello") {
        const welcome: any = { t: "welcome", v: 1, room: { code: "ABCD" }, you: { id: "p1", role: "player", name: "Ann", resume: "r", lastAct: 0 }, now: Date.now(), view: {}, rev: 0 };
        if (offer) welcome.rtc = true;
        this.deliver(welcome);
      } else if (msg.t === "rtc" && msg.sdp) {
        this.deliver({ t: "rtc", sdp: "answer" });
      }
    }
    deliver(msg: unknown) {
      setTimeout(() => this.onmessage?.({ data: JSON.stringify(msg) }));
    }
    close() {
      this.readyState = 3;
    }
  }
  return { FakeWs, sent, get socket() { return socket!; } };
}

class FakeChannel {
  readyState = "connecting";
  sent: any[] = [];
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  label: string;
  init: any;
  constructor(label: string, init: any) {
    this.label = label;
    this.init = init;
  }
  send(text: string) {
    this.sent.push(JSON.parse(text));
  }
  open() {
    this.readyState = "open";
    this.onopen?.();
  }
}

function fakeRtc(opens: boolean) {
  const pcs: FakePc[] = [];
  class FakePc {
    channels: FakeChannel[] = [];
    closed = false;
    localDescription: { sdp: string } | null = null;
    remote: unknown;
    connectionState = "new";
    onconnectionstatechange: (() => void) | null = null;
    config: unknown;
    constructor(config: unknown) {
      this.config = config;
      pcs.push(this);
    }
    createDataChannel(label: string, init: unknown) {
      const c = new FakeChannel(label, init);
      this.channels.push(c);
      return c;
    }
    async createOffer() {
      return { type: "offer", sdp: "offer" };
    }
    async setLocalDescription(d: { sdp: string }) {
      this.localDescription = d;
    }
    async setRemoteDescription(d: unknown) {
      this.remote = d;
      if (opens) setTimeout(() => this.channels.forEach((c) => c.open()));
    }
    close() {
      this.closed = true;
    }
  }
  return { FakePc, pcs };
}

const clients: PartyClient[] = [];
after(() => clients.forEach((c) => c.close()));

function client(host: ReturnType<typeof scriptedHost>, rtc: ReturnType<typeof fakeRtc> | undefined, extra: object = {}) {
  const c = new PartyClient({
    url: "ws://host/party/ws",
    room: "ABCD",
    key: "k",
    storage: memoryStorage(),
    WebSocket: host.FakeWs as unknown as typeof WebSocket,
    RTCPeerConnection: rtc?.FakePc as unknown as typeof RTCPeerConnection,
    rtc: rtc !== undefined,
    rtcTimeout: 100,
    ...extra,
  });
  clients.push(c);
  return c;
}

test("rtc: capabilities only with a peer connection, an offer only when the host offers", async () => {
  const plain = scriptedHost(true);
  const a = client(plain, undefined);
  await a.join("Ann");
  assert.equal(plain.sent[0].caps, undefined);
  a.close();

  const quiet = scriptedHost(false);
  const rtc = fakeRtc(true);
  const b = client(quiet, rtc);
  await b.join("Ann");
  assert.deepEqual(quiet.sent[0].caps, ["rtc"]);
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(rtc.pcs.length, 0);
  b.close();

  const screen = scriptedHost(true);
  const c = client(screen, fakeRtc(true), { role: "screen" });
  await c.connect();
  assert.equal(screen.sent[0].caps, undefined);
  c.close();
});

test("rtc: upgrades, acts go on r, input on u; when r closes the WebSocket takes over", async () => {
  const host = scriptedHost(true);
  const rtc = fakeRtc(true);
  const a = client(host, rtc);
  const transports: string[] = [];
  a.on("transport", (t) => transports.push(t));
  await a.join("Ann");
  await until(() => a.transport === "rtc", 2000);
  const pc = rtc.pcs[0]!;
  const [r, u] = pc.channels as [FakeChannel, FakeChannel];
  assert.deepEqual([r.label, r.init], ["r", { negotiated: true, id: 0, ordered: true }]);
  assert.deepEqual([u.label, u.init], ["u", { negotiated: true, id: 1, ordered: false, maxRetransmits: 0 }]);
  assert.deepEqual(pc.remote, { type: "answer", sdp: "answer" });
  assert.ok(host.sent.some((m) => m.t === "rtc" && m.sdp === "offer"));

  const done = a.act("go", 1);
  assert.deepEqual(r.sent.at(-1), { t: "act", id: 1, type: "go", data: 1 });
  r.onmessage!({ data: JSON.stringify({ t: "ack", id: 1, ok: true, result: "yes" }) });
  assert.equal(await done, "yes");

  // State from both paths: late (older) revisions are dropped.
  r.onmessage!({ data: JSON.stringify({ t: "state", rev: 1, set: { room: { n: 1 } } }) });
  host.socket.deliver({ t: "state", rev: 1, set: { room: { n: 0 } } });
  await new Promise((res) => setTimeout(res, 10));
  assert.deepEqual(a.view["room"], { n: 1 });

  a.input([0.5, -1]);
  assert.deepEqual(u.sent, [{ t: "in", s: 1, data: [0.5, -1] }]);

  // r dies with an action on it: the action goes again over the WebSocket.
  const pending = a.act("later");
  assert.deepEqual(r.sent.at(-1), { t: "act", id: 2, type: "later", data: null });
  r.onclose!();
  assert.equal(a.transport, "ws");
  assert.equal(pc.closed, true);
  assert.deepEqual(transports, ["rtc", "ws"]);
  assert.deepEqual(host.sent.slice(-2), [
    { t: "rtc", state: "closed" },
    { t: "act", id: 2, type: "later", data: null },
  ]);
  host.socket.deliver({ t: "ack", id: 2, ok: true });
  assert.equal(await pending, null);
  a.close();
});

test("rtc: channels that do not open in time: the client says so and stays on the WebSocket", async () => {
  const host = scriptedHost(true);
  const rtc = fakeRtc(false);
  const a = client(host, rtc);
  await a.join("Ann");
  await until(() => host.sent.some((m) => m.t === "rtc" && m.state === "failed"), 2000);
  assert.equal(rtc.pcs[0]!.closed, true);
  assert.equal(a.transport, "ws");
  const done = a.act("go");
  assert.deepEqual(host.sent.at(-1), { t: "act", id: 1, type: "go", data: null });
  host.socket.deliver({ t: "ack", id: 1, ok: true });
  await done;
  a.close();
});

test("rtc: the host ending it is not answered", async () => {
  const host = scriptedHost(true);
  const rtc = fakeRtc(true);
  const a = client(host, rtc);
  await a.join("Ann");
  await until(() => a.transport === "rtc", 2000);
  const before = host.sent.length;
  host.socket.deliver({ t: "rtc", state: "closed" });
  await until(() => a.transport === "ws", 2000);
  assert.equal(host.sent.length, before);
  assert.equal(rtc.pcs[0]!.closed, true);
  a.close();
});
