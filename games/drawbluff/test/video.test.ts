import assert from "node:assert/strict";
import { test } from "node:test";

import { doodle } from "../game/bots.ts";
import { seeded } from "../game/rules.ts";
import { readFileSync, writeFileSync } from "node:fs";
import { join as joinPath } from "node:path";

import { loadCanvas, renderScreen } from "../screen-video.ts";
import { SCREENS } from "./vectors.ts";

let canvasMod: Awaited<ReturnType<typeof loadCanvas>> | undefined;
try {
  canvasMod = await loadCanvas();
} catch {
  canvasMod = undefined;
}

const roster = ["Ada", "Bea", "Cy", "Dot with a really very long name"].map((name, i) => ({ id: `p${i + 1}`, name, vip: i === 0, connected: i !== 2 }));
// A 21x21 code: dark finder corners are enough to check placement.
const rows = Array.from({ length: 21 }, (_, y) => Array.from({ length: 21 }, (_, x) => ((x < 7 && y < 7) || (x > 13 && y < 7) || (x < 7 && y > 13) ? "1" : "0")).join(""));
const join = { code: "BCDF", url: "https://words-words-words-words.trycloudflare.com/?r=BCDF#k=abcdefghijklmnopqrstuv", qr: { size: 21, rows } };
const drawing = doodle(seeded(3));
const now = 1_000_000;
const views: Record<string, unknown>[] = [
  { phase: "lobby", minPlayers: 3, maxPlayers: 8 },
  { phase: "draw", round: 1, rounds: 2, deadline: now + 42_000, waiting: ["p1", "p2", "p3", "p4"], done: ["p2"] },
  { phase: "lie", round: 1, deadline: now + 5000, drawing, artist: { id: "p1", name: "Ada" }, waiting: ["p2", "p3"], done: [] },
  {
    phase: "vote",
    round: 1,
    deadline: now + 9000,
    drawing,
    artist: { id: "p1", name: "Ada" },
    options: [{ id: "o1", text: "a haunted toaster" }, { id: "o2", text: "a sad potato" }, { id: "o3", text: "a convincing lie that is rather long indeed, yes" }],
    waiting: ["p2", "p3"],
    done: ["p3"],
  },
  {
    phase: "reveal",
    round: 2,
    drawing,
    artist: { id: "p1", name: "Ada" },
    prompt: "a haunted toaster",
    step: 2,
    items: [
      { id: "o2", text: "a sad potato", truth: false, decoy: false, authors: ["Bea"], voters: ["Cy"], points: 500, artistPoints: 0 },
      { id: "o3", text: "a lie", truth: false, decoy: true, authors: [], voters: ["Dot"], points: 0, artistPoints: 0 },
      { id: "o1", text: "a haunted toaster", truth: true, decoy: false, authors: [], voters: ["Bea"], points: 1000, artistPoints: 1000 },
    ],
  },
  { phase: "scores", round: 1, rounds: 2, scores: roster.map((r, i) => ({ id: r.id, name: r.name, score: 3000 - i * 700, delta: 500 })), last: false },
  { phase: "final", scores: roster.map((r, i) => ({ id: r.id, name: r.name, score: 3000 - i * 700, delta: 0 })) },
  {},
];

test("the video screen draws every phase", { skip: !canvasMod && "@napi-rs/canvas is not installed" }, () => {
  const canvas = canvasMod!.createCanvas(1280, 720);
  const ctx = canvas.getContext("2d");
  for (const screen of views) {
    renderScreen(ctx, { roster, screen, join }, 1234, now);
    const px = ctx.getImageData(0, 0, 1280, 720).data;
    // Something besides the background was drawn.
    let bright = 0;
    for (let i = 0; i < px.length; i += 4 * 97) if (px[i]! > 200 && px[i + 1]! > 200) bright++;
    assert.ok(bright > 50, `phase ${String(screen["phase"])}: ${bright}`);
  }
});

test("the lobby's QR code: dark finder, light quiet zone", { skip: !canvasMod && "@napi-rs/canvas is not installed" }, () => {
  const canvas = canvasMod!.createCanvas(1280, 720);
  const ctx = canvas.getContext("2d");
  renderScreen(ctx, { roster, screen: views[0], join }, 0, now);
  // The code sits centered at x = 25vw, from y = 22vh, 46vh square, quiet zone 2 modules (25 in total).
  const size = 0.46 * 720;
  const x0 = 0.25 * 1280 - size / 2;
  const y0 = 0.22 * 720;
  const mod = size / 25;
  const at = (mx: number, my: number) => {
    const d = ctx.getImageData(Math.round(x0 + (mx + 0.5) * mod), Math.round(y0 + (my + 0.5) * mod), 1, 1).data;
    return d[0]! + d[1]! + d[2]!;
  };
  assert.ok(at(0, 0) > 700, "quiet zone is light");
  assert.ok(at(2, 2) < 100, "finder is dark");
  assert.ok(at(12, 12) > 700, "centre is light");
  assert.ok(at(22, 22) > 700, "no finder bottom right");
});

test("the video screen with 50 players (test/vectors/screens-50.json)", { skip: !canvasMod && "@napi-rs/canvas is not installed" }, async () => {
  const big = JSON.parse(readFileSync(SCREENS, "utf8")) as { roster: unknown[]; join: unknown; now: number; views: [string, Record<string, unknown>][] };
  const canvas = canvasMod!.createCanvas(1280, 720);
  const ctx = canvas.getContext("2d");
  const out = process.env["DRAWBLUFF_SCREEN_OUT"];
  for (const [name, screen] of big.views) {
    renderScreen(ctx, { roster: big.roster, screen, join: big.join }, 1234, big.now);
    const px = ctx.getImageData(0, 0, 1280, 720).data;
    let bright = 0;
    for (let i = 0; i < px.length; i += 4 * 97) if (px[i]! > 200 && px[i + 1]! > 200) bright++;
    assert.ok(bright > 50, `${name}: ${bright}`);
    if (out) writeFileSync(joinPath(out, `node-${name}.png`), await canvas.encode("png"));
  }
});
