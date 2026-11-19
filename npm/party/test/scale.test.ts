/**
 * 50 phones and a screen on one party: what a state change costs the host
 * (CPU per flush, bytes per change), the numbers docs/drawbluff-scale.md
 * §11 quotes. `PARTY_SCALE_REPORT=1` prints them.
 */

import assert from "node:assert/strict";
import { test } from "node:test";

import { setup, until } from "./helpers.ts";

/** A drawing-sized room: ~15 KB of strokes, as a phone sends them. */
function drawing(seed: number): { strokes: number[][] } {
  let x = seed;
  const r = () => ((x = (x * 1103515245 + 12345) % 2147483648) / 2147483648);
  return { strokes: Array.from({ length: 40 }, () => [Math.floor(r() * 8), 20, ...Array.from({ length: 60 }, () => Math.floor(r() * 1000))]) };
}

test("50 phones: one answer, one phase change, one roster change", async () => {
  const s = await setup({ party: { maxPlayers: 50 } });
  try {
    const phones = Array.from({ length: 50 }, () => s.phone());
    for (const [i, p] of phones.entries()) await p.join(`P${i}`);
    const screen = s.screen();
    await screen.connect();
    const party = s.party;
    const settle = async (check: () => boolean) => until(check, 5000);
    const measure = async (what: string, change: () => void, check: () => boolean, n = 1) => {
      const before = party.stats;
      for (let i = 0; i < n; i++) {
        change();
        await new Promise((r) => setImmediate(r));
      }
      await settle(check);
      const after = party.stats;
      const line = {
        what,
        changes: n,
        flushMsPerChange: (after.flushMs - before.flushMs) / n,
        statesPerChange: (after.states - before.states) / n,
        bytesPerChange: Math.round((after.stateBytes - before.stateBytes) / n),
      };
      if (process.env["PARTY_SCALE_REPORT"]) console.log(JSON.stringify(line));
      return line;
    };
    // A phase with a drawing everybody sees.
    const d = drawing(1);
    const phase = await measure(
      "room: a new phase with a 15 KB drawing (all 51 clients)",
      () => {
        party.room.replace({ phase: "vote", drawing: d, deadline: 1 });
        party.screen.replace({ phase: "vote", drawing: d, waiting: party.players.map((p) => p.id), done: [] });
        for (const p of party.players) p.state.replace({ task: "vote", options: [{ id: "o1", text: "a sad potato" }], done: false });
      },
      () => phones.every((p) => p.view["room"]?.["phase"] === "vote") && screen.view["screen"]?.["phase"] === "vote",
    );
    assert.equal(phase.statesPerChange, 51);
    // 50 answers, one at a time: the voter's state and the screen's `done`.
    let i = 0;
    const done: string[] = [];
    const answers = await measure(
      "an answer: one phone's state and the screen's done list",
      () => {
        const p = party.players[i++]!;
        p.state.set({ done: true, choice: "o1" });
        done.push(p.id);
        party.screen.set({ done: [...done] });
      },
      () => (screen.view["screen"]?.["done"] as string[] | undefined)?.length === 50,
      50,
    );
    assert.equal(answers.statesPerChange, 2);
    // A phone goes away: the roster changes for everybody.
    const away = await measure(
      "roster: a phone goes away (all clients)",
      () => phones[49]!.drop(),
      () => phones[0]!.view["roster"]?.[49]?.["connected"] === false,
    );
    assert.ok(away.statesPerChange >= 50);
  } finally {
    await s.close();
  }
});
