import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { Party } from "@tvpipe/party";
import { MemoryRelay } from "@tvpipe/party/testing";

import { Bot, bots } from "../game/bots.ts";
import { Garble, type Timings } from "../game/game.ts";
import { parseList, says, seeded } from "../game/rules.ts";

const data = (f: string) => parseList(readFileSync(new URL(`../data/${f}`, import.meta.url), "utf8"));
const PROMPTS = { en: data("prompts.txt"), de: data("prompts.de.txt") };
const WORDS = { en: data("words.txt"), de: data("words.de.txt") };

const fast: Timings = {
  intro: 20,
  firstDraw: 1500,
  draw: 1200,
  guess: 1200,
  showPrompt: 5,
  showDrawing: 5,
  showGuess: 5,
  overview: 5,
  judge: 1000,
  verdictBase: 5,
  verdictStep: 1,
  verdictSmuggle: 1,
  accuse: 1200,
  unmask: 5,
  scores: 5,
};

async function until(cond: () => boolean, ms = 3000): Promise<void> {
  const end = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > end) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 5));
  }
}

async function game(n: number, opts: { rounds?: number; silent?: number; length?: number; oracle?: boolean; lang?: "en" | "de" } = {}) {
  const relay = new MemoryRelay();
  const party = new Party({ grace: 100, maxPlayers: 12 }).listen(relay);
  const g = new Garble(party, { prompts: PROMPTS, words: WORDS, rounds: opts.rounds ?? 1, timings: fast, rng: seeded(11), length: opts.length, lang: opts.lang });
  g.open();
  const phases: string[] = [];
  g.on("phase", (p) => phases.push(p));
  const o = {
    url: "ws://party.test/_tvpipe/ws",
    WebSocket: relay.WebSocket,
    room: party.code,
    key: party.joinKey,
    rng: seeded(n),
    think: () => 3,
    oracle: opts.oracle === false ? undefined : (id: string) => g.meant(id),
  };
  const silent = opts.silent ?? 0;
  const players = await bots(n - silent, o);
  // Players who never answer: connected, but silent.
  for (let i = 0; i < silent; i++) {
    const b = new Bot({ ...o, name: `Mute${i + 1}`, silent: true });
    await b.join();
    players.push(b);
  }
  return { party, g, players, phases, close: () => party.close() };
}

test("a whole game with 4 players", async () => {
  const { party, g, players, phases, close } = await game(4, { rounds: 2 });
  try {
    const seen: { words: Map<string, string>; wrecker: string | undefined; steps: number }[] = [];
    g.on("phase", (p) => {
      if (p === "intro") seen.push({ words: new Map(g.round!.words), wrecker: g.round!.wrecker?.id, steps: g.round!.steps });
    });
    const end = new Promise<{ score: number }[]>((r) => g.once("end", r));
    await players[0]!.client.act("start");
    const table = await end;
    assert.equal(table.length, 4);
    assert.equal(phases.filter((p) => p === "intro").length, 2);
    assert.equal(phases.filter((p) => p === "play").length, 8, "4 steps per round with 4 players");
    assert.equal(phases.filter((p) => p === "reveal").length, 2);
    assert.equal(phases.at(-1), "final");
    assert.ok(table.reduce((s, l) => s + l.score, 0) > 0, "someone scored");
    assert.deepEqual(players.map((b) => b.errors), [[], [], [], []]);
    // Everyone had a word of their own, one Wrecker per round, and two different Wreckers.
    for (const r of seen) {
      assert.equal(new Set(r.words.values()).size, 4);
      assert.ok(r.wrecker);
    }
    assert.notEqual(seen[0]!.wrecker, seen[1]!.wrecker);
    await until(() => players.every((b) => b.client.view["me"].task === "final"));
    assert.equal((party.screenView()["screen"] as { phase: string }).phase, "final");
  } finally {
    for (const b of players) b.close();
    close();
  }
});

test("secrets stay on their own phone until the unmasking", async () => {
  const { party, g, players, close } = await game(5);
  try {
    const leaks: string[] = [];
    let words = new Map<string, string>();
    let wrecker = "";
    g.on("phase", (p) => {
      if (p === "intro") {
        words = new Map(g.round!.words);
        wrecker = g.round!.wrecker!.id;
      }
    });
    // Every view of every phone and of the TV, until the unmasking.
    const check = (who: string, view: Record<string, any>, me: string | undefined) => {
      const phase = view["screen"]?.phase ?? view["room"]?.phase;
      if (phase === "unmask" || phase === "scores" || phase === "final" || words.size === 0) return;
      // Smuggled words are public from their verdict on.
      const text = JSON.stringify({ ...view, roster: undefined }, (k, v) => (k === "smuggles" ? undefined : v));
      for (const [id, w] of words) {
        if (id === me) continue;
        // A word may show up in a guess (smuggled or by chance): only its own field counts as a leak.
        if (new RegExp(`"word":"${w}"`).test(text)) leaks.push(`${who} sees ${id}'s word`);
      }
      if (me !== wrecker && /"wrecker":true/.test(text)) leaks.push(`${who} sees the Wrecker`);
      if (/"wrecked"/.test(text)) leaks.push(`${who} sees a wreck`);
    };
    for (const b of players) b.client.on("view", (v) => check(b.name, v, b.client.you?.id));
    const tick = setInterval(() => check("TV", party.screenView(), undefined), 2);
    const end = new Promise((r) => g.once("end", r));
    await players[0]!.client.act("start");
    await end;
    clearInterval(tick);
    assert.deepEqual([...new Set(leaks)], []);
  } finally {
    for (const b of players) b.close();
    close();
  }
});

test("chains: lengths, prompts, the drawer reads the guess before", async () => {
  const { g, players, close } = await game(7, { length: 8 });
  try {
    let chains: { prompt: string; links: { kind: string; by: string; text?: string }[]; author: string }[] = [];
    g.on("phase", (p) => {
      if (p === "accuse") chains = structuredClone(g.round!.chains);
    });
    const end = new Promise((r) => g.once("end", r));
    await players[0]!.client.act("start");
    await end;
    assert.equal(chains.length, 7);
    for (const c of chains) {
      assert.equal(c.links.length, 6, "7 players: 6 steps (even, at most the players)");
      assert.ok(c.prompt.length > 0);
      assert.equal(c.links[0]!.by, c.author);
      assert.deepEqual(c.links.map((l) => l.kind), ["draw", "guess", "draw", "guess", "draw", "guess"]);
      assert.equal(new Set(c.links.map((l) => l.by)).size, 6, "nobody twice in a chain");
    }
  } finally {
    for (const b of players) b.close();
    close();
  }
});

test("silent players: empty drawings, skipped guesses, the machine judges", async () => {
  const { g, players, close } = await game(4, { silent: 2 });
  try {
    let judged = 0;
    const end = new Promise<{ score: number }[]>((r) => g.once("end", r));
    g.on("phase", (p) => {
      if (p === "accuse") judged = g.round!.died.size;
    });
    await players[0]!.client.act("start");
    await end;
    assert.equal(judged, 4, "every chain has a verdict");
  } finally {
    for (const b of players) b.close();
    close();
  }
});

test("secret words never appear in the round's prompts (German too)", async () => {
  const { g, players, close } = await game(6, { lang: "de" });
  try {
    let ok = true;
    g.on("phase", (p) => {
      if (p !== "intro") return;
      const r = g.round!;
      for (const w of r.words.values()) for (const [a, b] of r.choices) if (says(a, w) || says(b, w)) ok = false;
    });
    const end = new Promise((r) => g.once("end", r));
    await players[0]!.client.act("start");
    await end;
    assert.ok(ok);
    assert.equal(g.lang, "de");
  } finally {
    for (const b of players) b.close();
    close();
  }
});

test("the host page removes players and moves the VIP during a game: it plays on", async () => {
  const { party, g, players, close } = await game(9, { rounds: 2 });
  try {
    const end = new Promise((r) => g.once("end", r));
    const removed: string[] = [];
    // One player removed in each kind of phase, the first time it comes.
    g.on("phase", (p) => {
      if (removed.includes(p) || !["intro", "play", "reveal", "accuse", "unmask", "scores"].includes(p)) return;
      removed.push(p);
      setTimeout(() => {
        const victim = party.players.find((x) => !x.vip);
        if (victim) party.kick(victim);
        const next = party.players.find((x) => !x.vip);
        if (next) party.makeVip(next);
      }, 10);
    });
    await players[0]!.client.act("start");
    await end;
    assert.equal(removed.length, 6, "every kind of phase came");
    assert.equal(g.phase, "final");
    assert.equal(party.players.length, 3);
    assert.equal(party.players.filter((p) => p.vip).length, 1);
  } finally {
    for (const b of players) b.close();
    close();
  }
});

test("settings: the VIP only, before a game", async () => {
  const { g, players, close } = await game(4);
  try {
    await players[0]!.client.act("settings", { undo: true, twists: false, length: 4 });
    assert.deepEqual(g.settings, { undo: true, twists: false, length: 4 });
    await assert.rejects(players[1]!.client.act("settings", { undo: false }), /VIP/);
    await assert.rejects(players[0]!.client.act("settings", { length: 5 }), /length/);
    await until(() => players[2]!.client.view["room"].undo === true);
  } finally {
    for (const b of players) b.close();
    close();
  }
});
