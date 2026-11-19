/**
 * A live check of drawbluff (not part of `npm test`): the game with two
 * bots and one phone in a real (headless) Chrome, the main screen through
 * tvpipe to its browser player, screenshots of the TV and the phone in
 * every phase.
 *
 *   node test/live/play.ts [--video] [--tunnel=cloudflare|opentunnel] [--out DIR]
 *   node test/live/play.ts --host rust [--video]
 *
 * Needs Chrome, and tvpipe built (target/release or TVPIPE_BINARY). With
 * `--host rust` the game is the Rust binary (`cargo build --release -p
 * drawbluff`; its video screen by default, `--web-screen` without
 * `--video`), phones and bots unchanged: the same pages and the Node
 * bots against the other host.
 */

import { spawn } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { createInterface } from "node:readline";
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
    host: { type: "string", default: "node" },
  },
});

/** The game as the script sees it: links, the phase, the end. */
interface Host {
  join: string;
  room: string;
  joinKey: string;
  ws: string;
  playerUrl: string;
  phase(): string;
  /** Changes with every new phase (and drawing). */
  key(): string;
  ended(): Promise<void>;
  stop(): Promise<void>;
}

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

async function nodeHost(): Promise<Host> {
  const here = new URL("../../", import.meta.url);
  const party = await createParty({
    phone: new URL("web/phone/", here),
    screen: new URL("web/screen/", here),
    tunnel: (args.tunnel as TunnelChoice | undefined) ?? false,
    logLevel: "info",
  });
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
  return {
    join: party.links.join,
    room: party.code,
    joinKey: party.joinKey,
    ws: party.links.localJoin.replace(/^http/, "ws").replace(/\/\?.*$/, "/party/ws"),
    playerUrl: (tv.session as { playerUrl?: string }).playerUrl!,
    phase: () => game.phase,
    key: () => `${game.phase}:${JSON.stringify(party.screen.peek()["artist"] ?? "")}`,
    ended: () => new Promise<void>((r) => (game.phase === "final" ? r() : game.once("end", () => r()))),
    stop: async () => {
      game.close();
      await party.shutdown();
    },
  };
}

/** The Rust binary, read from its output (`-v` prints the phases). */
async function rustHost(): Promise<Host> {
  const exe = fileURLToPath(new URL(`../../../../target/release/drawbluff${process.platform === "win32" ? ".exe" : ""}`, import.meta.url));
  if (!existsSync(exe)) throw new Error(`build it first: cargo build --release -p drawbluff (${exe})`);
  const join = args.tunnel ? [`--tunnel=${args.tunnel}`] : ["--local"];
  const screen = args.video ? [] : ["--web-screen"];
  const proc = spawn(exe, ["--to", "browser", ...join, ...screen, "--rounds", "1", "--fast", "-v"], { stdio: ["ignore", "pipe", "pipe"] });
  let phase = "lobby";
  let n = 0;
  let end: () => void = () => {};
  const ended = new Promise<void>((r) => (end = r));
  const links: Record<string, string> = {};
  let ready: () => void = () => {};
  const linked = new Promise<void>((r) => (ready = r));
  createInterface({ input: proc.stdout! }).on("line", (line) => {
    const m = /^ {2}(phones join at|the TV's page|the screen) +(\S+)/.exec(line);
    if (m) links[m[1]!] = m[2]!;
    if (links["phones join at"] && links["the TV's page"] && links["the screen"]) ready();
    if (line.includes("game over:")) end();
  });
  createInterface({ input: proc.stderr! }).on("line", (line) => {
    const m = /^drawbluff: (lobby|draw|lie|vote|reveal|scores|final)$/.exec(line);
    if (m) {
      phase = m[1]!;
      n++;
    } else if (line.trim()) log(`rust: ${line}`);
  });
  proc.on("exit", (code) => {
    log(`rust host exited (${code})`);
    end();
  });
  await Promise.race([linked, new Promise((_, no) => setTimeout(() => no(new Error("no links from the rust host")), 60_000))]);
  const join0 = links["phones join at"]!;
  const u = new URL(join0);
  const local = `ws://127.0.0.1:${new URL(links["the screen"]!).port}/party/ws`;
  return {
    join: join0,
    room: u.searchParams.get("r")!,
    joinKey: new URLSearchParams(u.hash.slice(1)).get("k")!,
    ws: local,
    playerUrl: links["the TV's page"]!,
    phase: () => phase,
    key: () => `${phase}:${n}`,
    ended: () => ended,
    stop: async () => {
      proc.kill();
    },
  };
}

const host = args.host === "rust" ? await rustHost() : await nodeHost();
log(`join ${host.join}`);
log(`TV (${args.video ? "video" : "web"}, ${args.host} host) in the player: ${host.playerUrl}`);

const browser = await Browser.launch();
const tvTab = await browser.open(host.playerUrl, { width: 1280, height: 720 });
const phone = await browser.open(host.join, { width: 390, height: 844, mobile: true });
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
  const bs = await bots(2, { url: host.ws, room: host.room, key: host.joinKey, names: ["Ada", "Bea"], think: () => 1500 + Math.random() * 2500 });
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
  while (host.phase() !== "scores" && host.phase() !== "final") {
    const phase = host.phase();
    const key = host.key();
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
  await host.ended();
  await shot("final");
  for (const b of bs) b.close();
  const errors = bs.flatMap((b) => b.errors);
  log(`done: ${shots.length} screenshot pairs in ${out}; bot errors: ${errors.length ? errors.join(", ") : "none"}`);
} finally {
  browser.close();
  await host.stop();
}
process.exit(0);
