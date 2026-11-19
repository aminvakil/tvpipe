/**
 * A live check of drawbluff (not part of `npm test`): the game with two
 * bots and one phone in a real (headless) Chrome, the main screen through
 * tvpipe to its browser player, screenshots of the TV and the phone in
 * every phase.
 *
 *   node test/live/play.ts [--video] [--tunnel=cloudflare|opentunnel] [--out DIR]
 *
 * Needs Chrome, and tvpipe built (target/release or TVPIPE_BINARY).
 */

import { existsSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import { createParty, type TunnelChoice } from "@tvpipe/party";

import { bots } from "../../game/bots.ts";
import { Drawbluff } from "../../game/game.ts";
import { loadCanvas, renderScreen } from "../../screen-video.ts";
import { Browser } from "./browser.ts";

const { values: args } = parseArgs({
  options: {
    video: { type: "boolean", default: false },
    tunnel: { type: "string" },
    out: { type: "string", default: "/tmp/drawbluff-live" },
  },
});

if (!process.env["TVPIPE_BINARY"]) {
  for (const profile of ["release", "debug"]) {
    const p = fileURLToPath(new URL(`../../../../target/${profile}/tvpipe`, import.meta.url));
    if (existsSync(p)) {
      process.env["TVPIPE_BINARY"] = p;
      break;
    }
  }
}
const out = args.out;
mkdirSync(out, { recursive: true });
const t0 = Date.now();
const log = (m: string) => console.log(`${((Date.now() - t0) / 1000).toFixed(1).padStart(6)}s ${m}`);

const here = new URL("../../", import.meta.url);
const party = await createParty({
  phone: new URL("web/phone/", here),
  screen: new URL("web/screen/", here),
  tunnel: (args.tunnel as TunnelChoice | undefined) ?? false,
  logLevel: "info",
});
log(`join ${party.links.join}`);
const game = new Drawbluff(party, {
  rounds: 1,
  timings: { draw: 20_000, lie: 15_000, vote: 12_000, revealStep: 1800, revealEnd: 2500, scores: 4000 },
});
game.open();

let tv;
if (args.video) {
  const { createCanvas } = await loadCanvas();
  tv = await party.toTv({
    mode: "video",
    to: "browser",
    canvas: createCanvas(1280, 720),
    render: (ctx, view, t) => renderScreen(ctx, view, t, party.now()),
  });
} else {
  tv = await party.toTv({ mode: "web", to: "browser", size: "1280x720" });
}
const playerUrl = (tv.session as { playerUrl?: string }).playerUrl!;
log(`TV (${args.video ? "video" : "web"}) in the player: ${playerUrl}`);

const browser = await Browser.launch();
const tvTab = await browser.open(playerUrl, { width: 1280, height: 720 });
const phone = await browser.open(party.links.join, { width: 390, height: 844, mobile: true });
const shots: string[] = [];
const shot = async (name: string) => {
  // The TV is ~0.1-0.4 s behind: let it catch up.
  await new Promise((r) => setTimeout(r, 1200));
  await tvTab.screenshot(`${out}/tv-${name}.png`);
  await phone.screenshot(`${out}/phone-${name}.png`);
  shots.push(name);
  log(`screenshots: ${name}`);
};

try {
  // Join from the phone.
  await phone.until(`!!document.getElementById("name")`);
  await phone.eval(`document.getElementById("name").value = "Phil"; document.querySelector("#join-form button").click(); true`);
  await phone.until(`document.querySelector("h1")?.textContent === "You're in!"`);
  const url = party.links.localJoin.replace(/^http/, "ws").replace(/\/\?.*$/, "/party/ws");
  const bs = await bots(2, { url, room: party.code, key: party.joinKey, names: ["Ada", "Bea"], think: () => 1500 + Math.random() * 2500 });
  await shot("lobby");
  // Phil is the VIP (joined first): start.
  await phone.until(`!document.querySelector("button.primary")?.disabled`);
  await phone.eval(`document.querySelector("button.primary").click(); true`);

  // Draw.
  await phone.until(`!!document.querySelector("canvas.pad")`);
  const box = await phone.eval<{ x: number; y: number; w: number }>(
    `(() => { const r = document.querySelector("canvas.pad").getBoundingClientRect(); return { x: r.left, y: r.top, w: r.width }; })()`,
  );
  const pt = (u: number, v: number): [number, number] => [box.x + u * box.w, box.y + v * box.w];
  const circle = Array.from({ length: 30 }, (_, i) => pt(0.5 + 0.3 * Math.cos((i / 29) * 2 * Math.PI), 0.5 + 0.3 * Math.sin((i / 29) * 2 * Math.PI)));
  await phone.drag(circle);
  await phone.eval(`document.querySelectorAll(".palette button")[2].click(); true`);
  await phone.drag([pt(0.38, 0.4), pt(0.4, 0.42)]);
  await phone.drag([pt(0.6, 0.4), pt(0.62, 0.42)]);
  await phone.eval(`document.querySelectorAll(".palette button")[6].click(); true`);
  await phone.drag(Array.from({ length: 12 }, (_, i) => pt(0.35 + (0.3 * i) / 11, 0.6 + 0.08 * Math.sin((i / 11) * Math.PI))));
  await shot("draw");
  await phone.eval(`[...document.querySelectorAll("button")].find((b) => b.textContent === "Done!").click(); true`);

  // Per drawing: lie (or watch), vote, reveal.
  const seen = new Set<string>();
  while (game.phase !== "scores" && game.phase !== "final") {
    const phase = game.phase;
    const key = `${phase}:${JSON.stringify(party.screen.peek()["artist"] ?? "")}`;
    if (seen.has(key)) {
      await new Promise((r) => setTimeout(r, 200));
      continue;
    }
    seen.add(key);
    const n = seen.size;
    if (phase === "lie") {
      const lying = await phone.eval<boolean>(`!!document.querySelector("form input")`).catch(() => false);
      if (lying) {
        await phone.eval(
          `(() => { const i = document.querySelector("form input"); i.value = "a suspicious muffin"; i.form.requestSubmit(); return true; })()`,
        );
      }
      await shot(`${n}-lie`);
    } else if (phase === "vote") {
      await new Promise((r) => setTimeout(r, 500));
      const voting = await phone.eval<boolean>(`!!document.querySelector(".options button")`);
      if (voting) await phone.eval(`document.querySelector(".options button").click(); true`);
      await shot(`${n}-vote`);
    } else if (phase === "reveal") {
      await new Promise((r) => setTimeout(r, 4000));
      await shot(`${n}-reveal`);
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  await new Promise((r) => setTimeout(r, 800));
  await shot("scores");
  await new Promise<void>((r) => (game.phase === "final" ? r() : game.once("end", () => r())));
  await shot("final");
  for (const b of bs) b.close();
  const errors = bs.flatMap((b) => b.errors);
  log(`done: ${shots.length} screenshot pairs in ${out}; bot errors: ${errors.length ? errors.join(", ") : "none"}`);
} finally {
  browser.close();
  game.close();
  await party.shutdown();
}
process.exit(0);
