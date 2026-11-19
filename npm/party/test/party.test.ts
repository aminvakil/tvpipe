import assert from "node:assert/strict";
import { test } from "node:test";

import { PartyActionError } from "../src/client.ts";
import { PartyError, type Player } from "../src/party.ts";
import { setup, tick, until } from "./helpers.ts";

test("players join, see the roster and their own state; the first is VIP", async () => {
  const s = await setup();
  try {
    const joined: string[] = [];
    s.party.on("join", (p) => joined.push(p.name));
    const a = s.phone();
    const wa = await a.join("  Ann\u202e  ");
    assert.equal(wa.you.name, "Ann");
    assert.equal(wa.you.vip, true);
    const b = s.phone();
    await b.join("Bob");
    await until(() => a.view["roster"]?.length === 2, 2000, "roster");
    assert.deepEqual(
      a.view["roster"].map((r: { name: string; vip: boolean; connected: boolean }) => [r.name, r.vip, r.connected]),
      [
        ["Ann", true, true],
        ["Bob", false, true],
      ],
    );
    assert.deepEqual(joined, ["Ann", "Bob"]);
    const bob = s.party.players[1]!;
    bob.state.set({ secret: "pineapple" });
    s.party.room.set({ phase: "lobby" });
    await until(() => b.view["me"]?.secret === "pineapple" && a.view["room"]?.phase === "lobby");
    assert.equal(a.view["me"]?.secret, undefined, "Ann does not see Bob's state");
  } finally {
    await s.close();
  }
});

test("names: taken (case/accent-insensitive), invalid, moderated", async () => {
  const s = await setup({
    party: { moderate: (text) => (/badword/i.test(text) ? false : text.replace("darn", "d**n")) },
  });
  try {
    await s.phone().join("Zoë");
    await assert.rejects(s.phone().join("ZOE"), (e: PartyActionError) => e.code === "name-taken");
    await assert.rejects(s.phone().join("   "), (e: PartyActionError) => e.code === "name-invalid");
    await assert.rejects(s.phone().join("xbadwordx"), (e: PartyActionError) => e.code === "name-invalid");
    const w = await s.phone().join("darn it");
    assert.equal(w.you.name, "d**n it");
    const long = await s.phone().join("abcdefghijklmnopqrstuvwxyz");
    assert.equal(long.you.name, "abcdefghijklmnop");
  } finally {
    await s.close();
  }
});

test("wrong keys and unknown rooms are refused for good", async () => {
  const s = await setup();
  try {
    const bad = s.client({ key: "nope" });
    await assert.rejects(bad.join("Eve"), (e: PartyActionError) => e.code === "bad-key");
    const noroom = s.client({ room: "ZZZZ", key: s.party.joinKey });
    await assert.rejects(noroom.join("Eve"), (e: PartyActionError) => e.code === "no-room");
    const scr = s.client({ key: s.party.joinKey, role: "screen" });
    await assert.rejects(scr.connect(), (e: PartyActionError) => e.code === "bad-key");
    assert.equal(s.party.players.length, 0);
  } finally {
    await s.close();
  }
});

test("a phone that drops comes back as the same player with the full view", async () => {
  const s = await setup();
  try {
    const events: string[] = [];
    s.party.on("away", (p) => events.push(`away ${p.name}`));
    s.party.on("reconnect", (p) => events.push(`back ${p.name}`));
    const a = s.phone();
    await a.join("Ann");
    const id = a.you!.id;
    a.drop();
    await until(() => events.includes("away Ann"));
    assert.equal(s.party.players[0]!.connected, false);
    // Changes while away arrive with the welcome.
    s.party.room.set({ phase: "draw" });
    s.party.players[0]!.state.set({ prompt: "a nervous cactus" });
    await until(() => events.includes("back Ann"), 3000);
    await until(() => a.status === "open");
    assert.equal(a.you!.id, id);
    assert.equal(a.view["room"].phase, "draw");
    assert.equal(a.view["me"].prompt, "a nervous cactus");
    assert.equal(s.party.players.length, 1);
  } finally {
    await s.close();
  }
});

test("a reloaded page resumes from storage; a second tab replaces the first", async () => {
  const s = await setup();
  try {
    const a = s.phone();
    await a.join("Ann");
    // Same storage, a new page.
    const again = s.client({ key: s.party.joinKey, storage: a.storage as ReturnType<typeof import("../src/client.ts").memoryStorage> });
    assert.equal(again.canResume, true);
    const w = await again.connect();
    assert.equal(w.you.name, "Ann");
    await until(() => a.status === "closed");
    assert.equal(a.byeReason, "replaced");
    assert.equal(s.party.players.length, 1);
    again.close();
  } finally {
    await s.close();
  }
});

test("actions are acked, refused with codes, and applied once across reconnects", async () => {
  const s = await setup();
  try {
    let calls = 0;
    let release: () => void = () => {};
    const gate = new Promise<void>((r) => (release = r));
    s.party.handle("slow", async (data) => {
      calls++;
      await gate;
      return { echo: data };
    });
    s.party.handle("nope", () => {
      throw new PartyError("not-now", "Wait for your turn");
    });
    s.party.handle("boom", () => {
      throw new Error("bug");
    });
    const a = s.phone();
    await a.join("Ann");
    await assert.rejects(a.act("nope"), (e: PartyActionError) => e.code === "not-now" && e.message === "Wait for your turn");
    await assert.rejects(a.act("boom"), (e: PartyActionError) => e.code === "internal");
    await assert.rejects(a.act("missing"), (e: PartyActionError) => e.code === "unknown-action");
    // The connection dies while the host is still working on the action:
    // the client sends it again after the reconnect, the host runs it once.
    const p = a.act("slow", { n: 1 });
    await until(() => calls === 1);
    a.drop();
    await until(() => a.status === "reconnecting" || a.status === "open");
    await until(() => a.status === "open", 3000);
    release();
    assert.deepEqual(await p, { echo: { n: 1 } });
    await tick(50);
    assert.equal(calls, 1);
  } finally {
    await s.close();
  }
});

test("actions queued while disconnected are sent after the reconnect", async () => {
  const s = await setup();
  try {
    const got: unknown[] = [];
    s.party.handle("say", (d) => {
      got.push(d);
      return got.length;
    });
    const a = s.phone();
    await a.join("Ann");
    a.drop();
    const r = a.act("say", "hello");
    assert.equal(await r, 1);
    assert.deepEqual(got, ["hello"]);
  } finally {
    await s.close();
  }
});

test("VIP passes on when the VIP leaves; kick removes a player", async () => {
  const s = await setup();
  try {
    const a = s.phone();
    await a.join("Ann");
    const b = s.phone();
    await b.join("Bob");
    const c = s.phone();
    await c.join("Cy");
    a.leave();
    await until(() => s.party.players.length === 2);
    assert.equal(s.party.vip?.name, "Bob");
    await until(() => b.view["roster"].length === 2 && b.view["roster"][0].vip === true);
    s.party.kick(s.party.players.find((p) => p.name === "Cy")!);
    await until(() => c.status === "closed");
    assert.equal(c.byeReason, "kicked");
    assert.equal(s.party.players.length, 1);
  } finally {
    await s.close();
  }
});

test("lock and maxPlayers refuse new players, resumes still work", async () => {
  const s = await setup({ party: { maxPlayers: 2 } });
  try {
    const a = s.phone();
    await a.join("Ann");
    await s.phone().join("Bob");
    await assert.rejects(s.phone().join("Cy"), (e: PartyActionError) => e.code === "full");
    s.party.lock();
    s.party.kick(s.party.players[1]!);
    await assert.rejects(s.phone().join("Dee"), (e: PartyActionError) => e.code === "locked");
    a.drop();
    await until(() => !s.party.players[0]!.connected);
    await until(() => a.status === "open" && s.party.players[0]!.connected, 3000);
  } finally {
    await s.close();
  }
});

test("late join: closed refuses new names in a game, wait/open let them wait until activated", async () => {
  const s = await setup({ party: { lateJoin: "closed" } });
  try {
    const a = s.phone();
    await a.join("Ann");
    s.party.inProgress = true;
    await assert.rejects(s.phone().join("Bob"), (e: PartyActionError) => e.code === "in-progress");
    assert.equal(s.party.players.length, 1);
    a.drop();
    await until(() => a.status === "open" && s.party.players[0]!.connected, 3000);
    assert.equal(s.party.players[0]!.waiting, false);
    s.party.lateJoin = "open";
    const active: string[] = [];
    s.party.on("active", (p) => active.push(p.name));
    const b = s.phone();
    await b.join("Bob");
    const bob = s.party.players[1]!;
    assert.equal(bob.waiting, true);
    await until(() => a.view["roster"]?.[1]?.waiting === true);
    assert.equal(s.party.activate(s.party.players[0]!), false);
    assert.equal(s.party.activate(bob), true);
    assert.equal(s.party.activate(bob), false);
    assert.deepEqual(active, ["Bob"]);
    await until(() => a.view["roster"]?.[1] && !("waiting" in a.view["roster"][1]));
    assert.throws(() => (s.party.lateJoin = "maybe" as never), TypeError);
    s.party.inProgress = false;
    await s.phone().join("Cy");
    assert.equal(s.party.players[2]!.waiting, false);
  } finally {
    await s.close();
  }
});

test("screens get the screen view with the join info; watchers cannot act", async () => {
  const s = await setup();
  try {
    s.party.screen.set({ phase: "lobby" });
    const scr = s.screen();
    await scr.connect();
    assert.equal(scr.view["screen"].phase, "lobby");
    assert.equal(scr.view["join"].code, s.party.code);
    assert.deepEqual(scr.view["join"].qr, { size: 1, rows: ["1"] });
    await s.phone().join("Ann");
    await until(() => scr.view["roster"]?.length === 1);
    let from = "";
    s.party.handle("done", (_d, sender) => {
      from = sender.role;
      return null;
    });
    await scr.act("done");
    assert.equal(from, "screen");
    const w = s.screen("watch");
    await w.connect();
    await assert.rejects(w.act("done"), (e: PartyActionError) => e.code === "read-only");
    assert.equal(s.party.screenView()["screen"] !== undefined, true);
  } finally {
    await s.close();
  }
});

test("changes in one tick are one state message; transient messages", async () => {
  const s = await setup();
  try {
    const a = s.phone();
    await a.join("Ann");
    let states = 0;
    a.on("view", (_v, d) => {
      if (d) states++;
    });
    const msgs: [string, unknown][] = [];
    a.on("msg", (type: string, data: unknown) => msgs.push([type, data]));
    await tick(30);
    states = 0;
    s.party.room.set({ a: 1 });
    s.party.room.set({ b: 2 });
    s.party.players[0]!.state.set({ c: 3 });
    s.party.broadcast("ding", { n: 1 });
    s.party.send(s.party.players[0]!, "toast", "hi");
    await until(() => msgs.length === 2);
    await tick(30);
    assert.equal(states, 1);
    assert.deepEqual(a.view["room"], { a: 1, b: 2 });
    assert.deepEqual(msgs, [
      ["ding", { n: 1 }],
      ["toast", "hi"],
    ]);
    // Removing a key.
    s.party.room.set({ a: undefined });
    await until(() => !("a" in a.view["room"]));
  } finally {
    await s.close();
  }
});

test("collect: everyone answers, refusals, deadlines, away players", async () => {
  const s = await setup({ party: { grace: 50 } });
  try {
    const phones = [s.phone(), s.phone(), s.phone()];
    await Promise.all(phones.map((p, i) => p.join(`P${i}`)));
    const [a, b, c] = phones as [typeof phones[0], typeof phones[0], typeof phones[0]];
    // All answer: resolves before the deadline.
    const all = s.party.collect<string>("answer", {
      until: s.party.now() + 5000,
      validate: (d) => {
        if (typeof d !== "string" || !d) throw new PartyError("empty", "Say something");
        return d.toUpperCase();
      },
    });
    await assert.rejects(a.act("answer", ""), (e: PartyActionError) => e.code === "empty");
    await a.act("answer", "x");
    await assert.rejects(a.act("answer", "y"), (e: PartyActionError) => e.code === "already-answered");
    await b.act("answer", "y");
    await c.act("answer", "z");
    const got = await all;
    assert.deepEqual([...got.values()].sort(), ["X", "Y", "Z"]);
    await assert.rejects(a.act("answer", "late"), (e: PartyActionError) => e.code === "unknown-action");

    // A deadline with one silent player.
    const t0 = Date.now();
    const partial = s.party.collect("answer", { until: s.party.now() + 200 });
    await a.act("answer", 1);
    await b.act("answer", 2);
    const r = await partial;
    assert.equal(r.size, 2);
    assert.ok(Date.now() - t0 >= 200);

    // skipAway: an away player is not waited for.
    const only: Player[] = [s.party.players[0]!, s.party.players[1]!];
    const skip = s.party.collect("answer", { from: only, until: s.party.now() + 10_000, skipAway: true });
    b.close();
    await until(() => !s.party.players[1]!.connected);
    await a.act("answer", "only me");
    const r2 = await skip;
    assert.equal(r2.size, 1);
    // Not asked.
    const one = s.party.collect("answer", { from: [s.party.players[0]!] });
    await assert.rejects(c.act("answer", 1), (e: PartyActionError) => e.code === "not-asked");
    await a.act("answer", 1);
    assert.equal((await one).size, 1);
  } finally {
    await s.close();
  }
});

test("a flood of messages is cut off", async () => {
  const s = await setup({ party: { limits: { rate: 5, burst: 10 } } });
  try {
    s.party.handle("x", () => null);
    const a = s.phone();
    await a.join("Ann");
    const all = Array.from({ length: 30 }, () => a.act("x").catch((e: PartyActionError) => e.code));
    const results = await Promise.all(all);
    assert.ok(results.includes("rate-limited"), String(results));
    assert.equal(a.byeReason, "rate-limited");
  } finally {
    await s.close();
  }
});

test("a busy phone gets latest-wins views: one diff when it drains", async () => {
  const s = await setup();
  try {
    const slow = s.phone();
    await slow.join("Slow");
    const fast = s.phone();
    await fast.join("Fast");
    await until(() => slow.view["roster"]?.length === 2);
    await new Promise((r) => setTimeout(r, 20));
    const conn = [...s.relay.connections()][0]!;
    let states = 0;
    slow.on("view", (_v: unknown, msg: unknown) => {
      if (msg) states++;
    });
    s.relay.setBusy(conn, true);
    for (let i = 0; i < 20; i++) {
      s.party.room.set({ n: i });
      await tick(2);
    }
    await until(() => fast.view["room"]?.n === 19);
    assert.equal(states, 0, "nothing while busy");
    s.relay.setBusy(conn, false);
    await until(() => slow.view["room"]?.n === 19);
    assert.equal(states, 1, "one catch-up diff");
  } finally {
    await s.close();
  }
});

test("tvpipe's own closes are final for the client (4011, 4008, 1009)", async () => {
  const s = await setup();
  try {
    const a = s.phone();
    await a.join("Ann");
    const conn = [...s.relay.connections()][0]!;
    conn.close(4011, "closed");
    await until(() => a.status === "closed");
    assert.equal(a.byeReason, "room-closed");
  } finally {
    await s.close();
  }
});

test("the end: closing, then the room closes for every phone", async () => {
  const s = await setup();
  const a = s.phone();
  await a.join("Ann");
  const tv = s.screen();
  await tv.connect();
  await s.relay.end();
  await until(() => a.status === "closed" && tv.status === "closed");
  assert.equal(a.byeReason, "room-closed");
  assert.ok(s.party.closed);
  await s.close();
});

test("a host restart (1012): phones reconnect, and a new party turns them away for good", async () => {
  const s = await setup();
  try {
    const a = s.phone();
    await a.join("Ann");
    // Another host on the same relay, as after a reload of the host page.
    const { Party } = await import("../src/party.ts");
    const fresh = new Party({ code: s.party.code });
    for (const c of [...s.relay.connections()]) c.close(1012, "host restart");
    fresh.listen(s.relay);
    await until(() => a.status === "closed", 5000);
    // The old identity and key mean nothing to the new party.
    assert.equal(a.byeReason, "bad-key");
  } finally {
    await s.close();
  }
});
