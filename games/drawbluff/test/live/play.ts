/**
 * A live check of drawbluff (not part of `npm test`): the game with bots
 * (two by default, up to 49) and one phone in a real (headless) Chrome,
 * the main screen through tvpipe to its browser player, screenshots of
 * the TV and the phone in every phase (`tv-*.png`, `phone-*.png`), and of
 * the phone alone when it gets a lie or a vote (`phone-*-task.png`).
 *
 *   node test/live/play.ts [--video] [--bots N] [--tunnel=cloudflare|opentunnel] [--out DIR]
 *   node test/live/play.ts --host rust [--video] [--bots N]
 *   … [--late-join wait|open|closed] [--late N]
 *
 * `--late-join`: the phone (the VIP) taps that setting in the lobby
 * (screenshots `lobby-MODE`); `--late N`: N more bots join during the
 * first spotlight (the TV's "+ Anna").
 *
 * Needs Chrome, and tvpipe built (target/release or TVPIPE_BINARY). With
 * `--host rust` the game is the Rust binary (`cargo build --release -p
 * drawbluff`; its video screen by default, `--web-screen` without
 * `--video`), phones and bots unchanged: the same pages and the Node
 * bots against the other host.
 */

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
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
    bots: { type: "string", default: "2" },
    rounds: { type: "string", default: "1" },
    "late-join": { type: "string" },
    late: { type: "string", default: "0" },
  },
});

/** The game as the script sees it: links, the phase, the end. */
interface Host {
  join: string;
  room: string;
  joinKey: string;
  ws: string;
  playerUrl: string;
  /** The screen's page itself (the TV gets it through tvpipe). */
  screenUrl: string;
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
    maxPlayers: 50,
    logLevel: "info",
  });
  const game = new Drawbluff(party, {
    rounds: Number(args.rounds),
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
    screenUrl: party.links.screen,
    phase: () => game.phase,
    key: () => {
      const s = party.screen.peek();
      return `${game.phase}:${JSON.stringify([s["round"], s["spot"], s["page"]])}`;
    },
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
  const proc = spawn(exe, ["--to", "browser", ...join, ...screen, "--rounds", args.rounds, "--fast", "-v"], { stdio: ["ignore", "pipe", "pipe"] });
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
    const m = /^drawbluff: (lobby|draw|gallery|lie|vote|reveal|scores|final)$/.exec(line);
    if (m) {
      phase = m[1]!;
      n++;
    } else if (line.trim() && !line.startsWith("drawbluff: + ")) log(`rust: ${line}`);
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
    screenUrl: links["the screen"]!,
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
// The TV gets pixels; the web screen's page itself, to read its text.
const screenTab = args.video ? undefined : await browser.open(host.screenUrl, { width: 1280, height: 720 });
// The phone last: the tab in front (background tabs get no animation frames).
const phone = await browser.open(host.join, { width: 390, height: 844, mobile: true });
const shots: string[] = [];
const leaks: string[] = [];
const shot = async (name: string) => {
  // The TV is ~0.1-0.4 s behind: let it catch up.
  await new Promise((r) => setTimeout(r, 1200));
  await tvTab.screenshot(`${out}/tv-${name}.png`);
  // The web screen's page: no leaked placeholders.
  // And nothing below the bottom (hidden reveal lines are laid out too).
  const leak = await screenTab
    ?.eval<string>(
      `(() => { const m = /\\b(null|undefined|NaN)\\b/.exec(document.body.innerText); if (m) return m[0]; const below = [...document.querySelectorAll(".option, .cell, .player, .chips")].find((e) => e.getBoundingClientRect().bottom > innerHeight + 1); return below ? "overflow: " + below.textContent.slice(0, 40) : ""; })()`,
    )
    .catch(() => "");
  if (leak) {
    leaks.push(`${name}: ${leak}`);
    log(`the screen page shows "${leak}" (${name})`);
  }
  await phone.screenshot(`${out}/phone-${name}.png`);
  shots.push(name);
  log(`screenshots: ${name}`);
};

try {
  // Join from the phone.
  await phone.until(`!!document.getElementById("name")`);
  await phone.eval(`document.getElementById("name").value = "Phil"; document.querySelector("#join-form button").click(); true`);
  await phone.until(`document.querySelector("h1")?.textContent === "You're in!"`);
  const nBots = Number(args.bots);
  const bs = await bots(nBots, {
    url: host.ws,
    room: host.room,
    key: host.joinKey,
    names: nBots <= 2 ? ["Ada", "Bea"] : undefined,
    think: () => 1500 + Math.random() * 2500,
  });
  log(`${bs.length} bots joined`);
  await shot("lobby");
  const mode = args["late-join"];
  if (mode) {
    // The VIP's latecomers setting, at the bottom of the lobby card.
    await phone.until(`!!document.querySelector('.late button[data-mode="${mode}"]')`);
    await phone.eval(`(() => { const b = document.querySelector('.late button[data-mode="${mode}"]'); b.click(); b.scrollIntoView({ block: "center" }); return true; })()`);
    await phone.until(`document.querySelector('.late button[data-mode="${mode}"]').classList.contains("chosen")`);
    await shot(`lobby-${mode}`);
    await phone.eval(`scrollTo(0, 0); true`);
  }
  const late: Awaited<ReturnType<typeof bots>> = [];
  let lateTried = false;
  const lateJoin = async () => {
    const n = Number(args.late);
    if (n <= 0 || lateTried) return;
    lateTried = true;
    const names = ["Anna", "Ben", "Cleo", "Dev", "Eve"].slice(0, n);
    try {
      late.push(...(await bots(names.length, { url: host.ws, room: host.room, key: host.joinKey, names, think: () => 1500 + Math.random() * 2500 })));
      const end = Date.now() + 3000;
      while (late.some((b) => !b.client.view["me"]?.task) && Date.now() < end) await new Promise((r) => setTimeout(r, 50));
      const notes = late.map((b) => `${b.name}: ${b.client.view["me"]?.task ?? "?"}/${b.client.view["me"]?.note ?? ""}`);
      log(`late: ${notes.join(", ")}`);
    } catch (e) {
      log(`late: refused (${(e as { code?: string }).code ?? String(e)})`);
    }
  };
  // Phil is the VIP (joined first): start.
  await phone.until(`!document.querySelector("button.primary")?.disabled`);
  await phone.eval(`document.querySelector("button.primary").click(); true`);

  // Draw (every round), then Done.
  const drawOnPhone = async (name: string) => {
    await phone.until(`!!document.querySelector("canvas.pad")`, 30_000);
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
    await shot(name);
    await phone.eval(`[...document.querySelectorAll("button")].find((b) => b.textContent === "Done!").click(); true`);
  };

  // Per phase on the TV (gallery pages, lie, vote, reveal per drawing): a
  // pair of screenshots. The phone follows its own task (the next drawing's
  // liars write during the current vote): it lies or votes when it is asked.
  const seen = new Set<string>();
  const tasks = new Set<string>();
  let n = 0;
  const lies = ["a suspicious muffin", "grandma's secret weapon", "the last banana", "a moody cloud"];
  const drivePhone = async () => {
    const t = await phone
      .eval<{ lie: boolean; vote: number; key: string }>(
        `(() => { const i = document.querySelector("form input"); const o = document.querySelectorAll(".options button:not(.chosen)"); const c = document.querySelector(".options .chosen"); return { lie: !!i, vote: c ? 0 : o.length, key: document.querySelector("main")?.innerText.slice(0, 200) ?? "" }; })()`,
      )
      .catch(() => ({ lie: false, vote: 0, key: "" }));
    if (t.lie && !tasks.has(`lie:${t.key}`)) {
      tasks.add(`lie:${t.key}`);
      await phone.screenshot(`${out}/phone-${String(++n).padStart(2, "0")}-lie-task.png`);
      const lie = lies[tasks.size % lies.length]!;
      await phone.eval(`(() => { const i = document.querySelector("form input"); i.value = ${JSON.stringify(lie)}; i.form.requestSubmit(); return true; })()`);
      log(`phone: lied "${lie}"`);
    } else if (t.vote > 0 && !tasks.has(`vote:${t.key}`)) {
      tasks.add(`vote:${t.key}`);
      await phone.screenshot(`${out}/phone-${String(++n).padStart(2, "0")}-vote-task.png`);
      await phone.eval(`(() => { const o = document.querySelectorAll(".options button"); o[Math.floor(Math.random() * o.length)].click(); return true; })()`);
      log(`phone: voted (${t.vote} options)`);
    }
  };
  while (host.phase() !== "final") {
    await drivePhone();
    const phase = host.phase();
    const key = host.key();
    if (seen.has(key)) {
      await new Promise((r) => setTimeout(r, 200));
      continue;
    }
    seen.add(key);
    const at = String(++n).padStart(2, "0");
    if (phase === "draw") await drawOnPhone(`${at}-draw`);
    else if (phase === "gallery") await shot(`${at}-gallery`);
    else if (phase === "lie") {
      await lateJoin();
      await shot(`${at}-lie`);
    } else if (phase === "vote") {
      await lateJoin();
      await new Promise((r) => setTimeout(r, 300));
      await drivePhone();
      await shot(`${at}-vote`);
    } else if (phase === "reveal") {
      await new Promise((r) => setTimeout(r, 4000));
      await shot(`${at}-reveal`);
      // And the TV just before the next phase: the truth.
      let last: Buffer | undefined;
      while (host.key() === key) {
        last = await tvTab.capture();
        await drivePhone();
        await new Promise((r) => setTimeout(r, 500));
      }
      if (last) writeFileSync(`${out}/tv-${at}-reveal-end.png`, last);
    } else if (phase === "scores") {
      await new Promise((r) => setTimeout(r, 800));
      await shot(`${at}-scores`);
    }
  }
  await host.ended();
  await shot("final");
  for (const b of [...bs, ...late]) b.close();
  const errors = [...bs, ...late].flatMap((b) => b.errors);
  log(`done: ${shots.length} screenshot pairs in ${out}; bot errors: ${errors.length ? errors.join(", ") : "none"}${screenTab ? `; on the screen page: ${leaks.length ? leaks.join(", ") : "no null/undefined/NaN, no overflow"}` : ""}`);
} finally {
  browser.close();
  await host.stop();
}
process.exit(0);
