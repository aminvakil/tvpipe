import assert from "node:assert/strict";
import { test } from "node:test";

import { PartyServer } from "@tvpipe/party";

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

const fast = { draw: 1500, lie: 1200, vote: 1200, revealStep: 20, revealEnd: 20, scores: 20 };

async function game(n: number, opts: { silent?: number; drop?: number; rounds?: number } = {}) {
  const server = new PartyServer({ log: () => {} });
  await server.listen();
  const party = server.createParty({ grace: 100 });
  const g = new Drawbluff(party, { rounds: opts.rounds ?? 2, timings: fast, rng: seeded(7) });
  g.open();
  const url = `ws://127.0.0.1:${server.port}/party/ws`;
  const phases: string[] = [];
  g.on("phase", (p) => phases.push(p));
  const players = await bots(n, { url, room: party.code, key: party.joinKey, rng: seeded(n), think: () => 5 });
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

test("8 players, one round, everyone lies for me", async () => {
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

test("too few players, non-VIP start, join while running", async () => {
  const { server, party, g, players } = await game(2);
  try {
    await assert.rejects(players[0]!.client.act("start"), (e: { code: string }) => e.code === "too-few");
    const more = await bots(1, { url: `ws://127.0.0.1:${server.port}/party/ws`, room: party.code, key: party.joinKey, names: ["Zed"], think: () => 5 });
    players.push(...more);
    await assert.rejects(players[1]!.client.act("start"), (e: { code: string }) => e.code === "not-vip");
    const end = new Promise((r) => g.once("end", r));
    await players[0]!.client.act("start");
    await assert.rejects(
      bots(1, { url: `ws://127.0.0.1:${server.port}/party/ws`, room: party.code, key: party.joinKey, names: ["Late"] }),
      (e: { code: string }) => e.code === "locked",
    );
    await end;
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
      if (p === "draw") players[2]!.client._drop();
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
