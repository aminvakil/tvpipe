import assert from "node:assert/strict";
import { test } from "node:test";

import { Party, type LateJoin } from "@tvpipe/party";
import { MemoryRelay } from "@tvpipe/party/testing";

async function until(cond: () => boolean, ms = 3000): Promise<void> {
  const end = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > end) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 5));
  }
}

import { bots } from "../game/bots.ts";
import { Drawbluff } from "../game/game.ts";
import { seeded } from "../game/rules.ts";
import { PROMPT_LIST } from "./prompts.ts";

const fast = { draw: 1500, lie: 1200, vote: 1200, revealStep: 20, revealEnd: 20, scores: 20, gallery: 20 };

async function game(n: number, opts: { silent?: number; drop?: number; rounds?: number; think?: number; lateJoin?: LateJoin } = {}) {
  const relay = new MemoryRelay();
  const party = new Party({ grace: 100, maxPlayers: Math.max(8, n + 2) }).listen(relay);
  const server = { relay, close: async () => party.close() };
  const g = new Drawbluff(party, { rounds: opts.rounds ?? 2, timings: fast, rng: seeded(7), lateJoin: opts.lateJoin, prompts: PROMPT_LIST });
  g.open();
  const url = "ws://party.test/_tvpipe/ws";
  const phases: string[] = [];
  g.on("phase", (p) => phases.push(p));
  const players = await bots(n, { url, WebSocket: relay.WebSocket, room: party.code, key: party.joinKey, rng: seeded(n), think: () => opts.think ?? 5 });
  if (opts.silent !== undefined) {
    // One player who never answers: their client stays connected.
    players[opts.silent]!.client.on("view", () => {});
  }
  return { server, party, g, players, phases };
}

test("a whole game with 3 players", async () => {
  const { server, party, g, players, phases } = await game(3);
  try {
    const end = new Promise((r) => g.once("end", r));
    await players[0]!.client.act("start");
    const table = (await end) as { name: string; score: number }[];
    assert.equal(table.length, 3);
    assert.equal(phases.filter((p) => p === "draw").length, 2);
    assert.equal(phases.filter((p) => p === "reveal").length, 6, "each player's drawing, twice");
    assert.equal(g.phase, "final");
    const total = table.reduce((s, l) => s + l.score, 0);
    assert.ok(total > 0, "someone scored");
    assert.deepEqual(
      players.map((b) => b.errors),
      [[], [], []],
    );
    // Phones show the final screen with their rank.
    await until(() => players.every((b) => b.client.view["me"].task === "final"));
    assert.equal(party.screenView()["screen"]!.constructor, Object);
    assert.equal((party.screenView()["screen"] as { phase: string }).phase, "final");
    assert.equal(party.locked, false);
  } finally {
    for (const b of players) b.close();
    await server.close();
  }
});

test("8 players, one round (the classic game's most)", async () => {
  const { server, g, players } = await game(8, { rounds: 1 });
  try {
    const end = new Promise((r) => g.once("end", r));
    await players[0]!.client.act("start");
    const table = (await end) as unknown[];
    assert.equal(table.length, 8);
  } finally {
    for (const b of players) b.close();
    await server.close();
  }
});

test("too few players, non-VIP start, a late joiner plays from the next round", async () => {
  const { server, party, g, players } = await game(2);
  try {
    await assert.rejects(players[0]!.client.act("start"), (e: { code: string }) => e.code === "too-few");
    const more = await bots(1, { url: "ws://party.test/_tvpipe/ws", WebSocket: server.relay.WebSocket, room: party.code, key: party.joinKey, names: ["Zed"], think: () => 5 });
    players.push(...more);
    await assert.rejects(players[1]!.client.act("start"), (e: { code: string }) => e.code === "not-vip");
    const end = new Promise((r) => g.once("end", r));
    await players[0]!.client.act("start");
    await until(() => g.phase === "lie" || g.phase === "vote");
    const late = await bots(1, { url: "ws://party.test/_tvpipe/ws", WebSocket: server.relay.WebSocket, room: party.code, key: party.joinKey, names: ["Late"], think: () => 5 });
    players.push(...late);
    await until(() => late[0]!.client.view["me"]?.task === "wait");
    assert.equal(late[0]!.client.view["me"].note, "round");
    assert.equal(party.players.find((p) => p.name === "Late")!.waiting, true);
    await until(() => (party.screen.peek() as { joined?: string[] }).joined?.includes("Late") === true);
    await until(() => late[0]!.client.view["me"]?.task === "draw", 15_000);
    assert.equal(party.players.find((p) => p.name === "Late")!.waiting, false);
    const table = (await end) as { name: string }[];
    assert.deepEqual(table.map((l) => l.name).sort(), ["Ada", "Bea", "Late", "Zed"]);
  } finally {
    for (const b of players) b.close();
    await server.close();
  }
});

test("late join open: dealt in at the next drawing (never into a running vote), first in the next round's spotlights", async () => {
  // 13: pipelined, and in round 2 seven never-spotted players compete for six spotlights.
  const { server, party, g, players } = await game(13, { lateJoin: "open" });
  try {
    const url = "ws://party.test/_tvpipe/ws";
    const end = new Promise((r) => g.once("end", r));
    await players[0]!.client.act("start");
    assert.equal(party.inProgress, true);
    await until(() => g.phase === "vote", 15_000);
    const s = party.screen.peek() as { round: number; spot: number };
    const running = `${s.round}.${s.spot}`;
    const late = (await bots(1, { url, WebSocket: server.relay.WebSocket, room: party.code, key: party.joinKey, names: ["Late"], think: () => 5 }))[0]!;
    players.push(late);
    const tasks: string[] = [];
    late.client.on("view", (v) => {
      const me = v["me"] ?? {};
      const t = `${v["room"]?.round}:${me.task}:${me.key ?? me.note ?? ""}`;
      if (tasks[tasks.length - 1] !== t) tasks.push(t);
    });
    await until(() => late.client.view["me"]?.task === "wait");
    assert.equal(late.client.view["me"].note, "drawing");
    await until(() => (party.screen.peek() as { joined?: string[] }).joined?.includes("Late") === true);
    // Round 2: Late draws, and their drawing gets the stage.
    const id = late.client.you!.id;
    let spotted = false;
    g.on("phase", (p) => {
      const v = party.screen.peek() as { round?: number; artist?: { id: string } };
      if ((p === "lie" || p === "vote") && v.round === 2 && v.artist?.id === id) spotted = true;
    });
    const table = (await end) as { name: string }[];
    assert.ok(table.some((l) => l.name === "Late"));
    assert.ok(!tasks.some((t) => t.startsWith("1:draw")), `no drawing in round 1: ${tasks.join(" ")}`);
    assert.ok(!tasks.some((t) => t === `1:vote:${running}`), "not in the running vote");
    assert.ok(tasks.some((t) => t.startsWith("1:vote:") || t.startsWith("1:lie:")), `dealt in during round 1: ${tasks.join(" ")}`);
    assert.ok(tasks.some((t) => t.startsWith("2:draw")), "draws in round 2");
    assert.ok(spotted, "a spotlight for the newcomer in round 2");
    assert.equal(party.inProgress, false);
    assert.deepEqual(late.errors, []);
  } finally {
    for (const b of players) b.close();
    await server.close();
  }
});

test("late join closed: newcomers refused during the game, welcome after; the VIP's toggle", async () => {
  const { server, party, g, players } = await game(3, { rounds: 1 });
  try {
    const url = "ws://party.test/_tvpipe/ws";
    assert.equal(players[0]!.client.view["room"].lateJoin, "wait");
    await assert.rejects(players[1]!.client.act("late-join", { mode: "closed" }), (e: { code: string }) => e.code === "not-vip");
    await assert.rejects(players[0]!.client.act("late-join", { mode: "maybe" }), (e: { code: string }) => e.code === "bad-mode");
    await players[0]!.client.act("late-join", { mode: "closed" });
    assert.equal(party.lateJoin, "closed");
    await until(() => players[1]!.client.view["room"].lateJoin === "closed");
    const end = new Promise((r) => g.once("end", r));
    await players[0]!.client.act("start");
    await assert.rejects(players[0]!.client.act("late-join", { mode: "open" }), (e: { code: string }) => e.code === "running");
    await assert.rejects(bots(1, { url, WebSocket: server.relay.WebSocket, room: party.code, key: party.joinKey, names: ["Late"] }), (e: { code: string }) => e.code === "in-progress");
    await end;
    const late = await bots(1, { url, WebSocket: server.relay.WebSocket, room: party.code, key: party.joinKey, names: ["Late"] });
    players.push(...late);
    await until(() => late[0]!.client.view["me"]?.task === "final");
    assert.equal(party.players.length, 4);
  } finally {
    for (const b of players) b.close();
    await server.close();
  }
});

test("a silent player and one who drops mid-game: deadlines move the game on", async () => {
  const { server, g, players } = await game(4, { rounds: 1 });
  try {
    const silent = players[3]!;
    silent.client.close(); // gone: never answers, never comes back
    const end = new Promise((r) => g.once("end", r));
    // A player who drops during the drawing phase and comes back.
    g.once("phase", (p) => {
      if (p === "draw") players[2]!.client.drop();
    });
    const t0 = Date.now();
    await players[0]!.client.act("start");
    const table = (await end) as { name: string }[];
    assert.equal(table.length, 4, "the away player keeps their place");
    assert.ok(Date.now() - t0 >= fast.draw, "the draw phase waited for the deadline");
    assert.equal(players[2]!.client.status, "open");
  } finally {
    for (const b of players) b.close();
    await server.close();
  }
});

test("VIP removes a player in the lobby", async () => {
  const { server, party, players } = await game(3);
  try {
    await players[0]!.client.act("kick", { id: players[2]!.client.you!.id });
    assert.equal(party.players.length, 2);
    await assert.rejects(players[1]!.client.act("kick", { id: players[0]!.client.you!.id }), (e: { code: string }) => e.code === "not-vip");
  } finally {
    for (const b of players) b.close();
    await server.close();
  }
});

test("50 players: spotlights, rotating liars, pipelined lies, sampled options, every phone busy", async () => {
  const { server, party, g, players } = await game(50, { rounds: 2, think: 150 });
  try {
    const phases: string[] = [];
    const problems: string[] = [];
    const byId = new Map(players.map((b) => [b.client.you!.id, b]));
    g.on("phase", (p) => {
      phases.push(p);
      if (p !== "vote") return;
      // Checked once the vote's views are out.
      setTimeout(() => {
        const s = party.screen.peek() as Record<string, any>;
        if (s["phase"] !== "vote") return problems.push("the vote was over too soon to check");
        const voters = new Set<string>(s["waiting"]);
        let liars = 0;
        for (const [id, b] of byId) {
          const me = b.client.view["me"] ?? {};
          if (me.task === "vote") {
            if (!voters.has(id)) problems.push(`${id} votes but is not waited for`);
            const opts = me.options as { id: string }[];
            if (opts.length > 6) problems.push(`${id} has ${opts.length} options`);
            const tv = (s["options"] as { id: string }[]).map((o) => o.id);
            const order = opts.map((o) => tv.indexOf(o.id));
            if (order.some((x, i) => x < 0 || (i > 0 && x < order[i - 1]!))) problems.push(`${id}: options not in the TV's order`);
          } else if (me.task === "lie") liars++;
          else if (me.task !== "artist") problems.push(`${id} has nothing to do: ${me.task}`);
        }
        if (s["spot"] < s["spots"] && liars === 0) problems.push(`no liars writing during vote ${s["spot"]}`);
      }, 60);
    });
    const end = new Promise((r) => g.once("end", r));
    await players[0]!.client.act("start");
    const table = (await end) as { name: string; score: number }[];
    assert.equal(table.length, 50);
    assert.deepEqual(problems.slice(0, 5), []);
    assert.equal(phases.filter((p) => p === "reveal").length, 12, "6 spotlights a round");
    assert.equal(phases.filter((p) => p === "gallery").length, 2);
    assert.ok(table[0]!.score < 40_000, `points scaled: ${table[0]!.score}`);
    const screen = party.screen.peek() as Record<string, any>;
    assert.equal(screen["total"], 50);
    assert.equal(screen["scores"].length, 8);
    assert.ok(players.every((b) => b.client.view["me"].of === 50));
    assert.deepEqual(new Set(players.flatMap((b) => b.errors)), new Set());
  } finally {
    for (const b of players) b.close();
    await server.close();
  }
});
