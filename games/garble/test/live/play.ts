/**
 * A live check of garble on `tvpipe game play` (not part of `npm test`):
 * tvpipe serves the bundle (`dist/`); a headless Chrome plays the window
 * (the launcher, then the host page), the TV (the watch link at
 * 1280×720) and a phone (joining, drawing with touch, guessing, judging,
 * accusing, reacting); bots join in the window (with the game's oracle,
 * so chains survive and die). Screenshots of the TV and the phone at
 * each moment, and a layout check of every phone screen.
 *
 *   npm run build && node test/live/play.ts [--bots N] [--rounds N] [--length 4|6|8] [--lang en|de] [--undo] [--out DIR] [--shots DIR]
 *
 * `--shots DIR`: the pictures for the games list (docs/game-catalog.md)
 * into DIR: `tv-verdict` (the chain the phone judged, on the TV),
 * `phone-draw` and `phone-judge`; the game ends there. The bots then have
 * people's names:
 *
 *   node test/live/play.ts --bots 7 --rounds 1 --shots ../../site/static/games/garble
 *
 * Needs Chrome and tvpipe built (target/release, else target/debug, or TVPIPE_BINARY).
 */

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import { Browser, type Tab } from "./browser.ts";
import { keep } from "./shots.ts";

const { values: args } = parseArgs({
  options: {
    bots: { type: "string", default: "5" },
    rounds: { type: "string", default: "2" },
    length: { type: "string", default: "6" },
    lang: { type: "string", default: "en" },
    undo: { type: "boolean", default: false },
    out: { type: "string" },
    shots: { type: "string" },
  },
});

const here = new URL("../../", import.meta.url);
const repo = new URL("../../", here);
const exe = process.platform === "win32" ? "tvpipe.exe" : "tvpipe";
const binary = process.env["TVPIPE_BINARY"] ?? ["release", "debug"].map((p) => fileURLToPath(new URL(`target/${p}/${exe}`, repo))).find((p) => existsSync(p));
if (!binary) throw new Error("build tvpipe first (cargo build --release -p tvpipe-cli)");
const lang = args.lang === "de" ? "de" : "en";
const out = args.out ?? `/tmp/garble-play${lang === "de" ? "-de" : ""}`;
mkdirSync(out, { recursive: true });
if (args.shots) mkdirSync(args.shots, { recursive: true });
const t0 = Date.now();
const log = (m: string) => console.log(`${((Date.now() - t0) / 1000).toFixed(1).padStart(6)} ${m}`);

const proc = spawn(binary, ["game", "play", fileURLToPath(here), "--window", "none", "--no-tunnel"], { stdio: ["ignore", "pipe", "pipe"] });
const exited = new Promise<number | null>((r) => proc.on("exit", (c) => r(c)));
const tvpipeLog: string[] = [];
createInterface({ input: proc.stderr! }).on("line", (l) => {
  tvpipeLog.push(l);
  if (/ (WARN|ERROR) /.test(l)) log(`tvpipe: ${l}`);
});
const start = await new Promise<string>((resolve, reject) => {
  createInterface({ input: proc.stdout! }).once("line", resolve);
  exited.then(() => reject(new Error(`tvpipe exited:\n${tvpipeLog.join("\n")}`)));
});

const browser = await Browser.launch();
let failed = false;
try {
  const win = await browser.open(start, { width: 1200, height: 800 });
  await win.until(`document.querySelectorAll("#devices .device").length > 0`, 20_000);
  await win.eval(`document.querySelector('#devices .device[data-name="browser"]').click(); true`);
  await win.until(`!document.getElementById("start").disabled`);
  await win.eval(`document.getElementById("start").click(); true`);
  await win.until(`location.pathname.endsWith("/host/index.html") && !!globalThis.garble`, 20_000);
  log("the host page runs the game");
  const set = (id: string, value: string) => win.eval(`(() => { const s = document.getElementById(${JSON.stringify(id)}); s.value = ${JSON.stringify(value)}; s.dispatchEvent(new Event("change")); return true; })()`);
  await set("rounds", args.rounds);
  await set("length", args.length);
  await set("timers", "fast");
  await set("lang", lang);
  if (args.undo) await set("undo", "on");
  const links = await win.eval<{ join: string; watch: string }>(`({ join: garble.party.links.join, watch: garble.party.links.watch })`);
  const tv = await browser.open(`${links.watch}&mute=1`, { width: 1280, height: 720 });
  const PHONE = { width: 390, height: 844, mobile: true };
  const phone = await browser.open(links.join, { ...PHONE, locale: lang === "de" ? "de-DE" : "en-US" });
  await phone.until(`!!document.getElementById("name")`, 20_000);
  await tv.screenshot(`${out}/tv-00-empty.png`);
  await phone.screenshot(`${out}/phone-00-join.png`);
  await phone.eval(`document.getElementById("name").value = "Phil"; document.querySelector("form button").click(); true`);
  await phone.until(`!!document.querySelector(".lobby")`);
  if (args.shots) {
    // As the host page adds them, but with people's names.
    await win.eval(`(async () => {
      const { bots } = await import(new URL("../game/bots.js", location.href).href);
      globalThis.__people = await bots(${Number(args.bots)}, {
        url: (location.protocol === "https:" ? "wss:" : "ws:") + "//" + location.host + "/_tvpipe/ws",
        room: garble.party.code,
        key: garble.party.joinKey,
        names: ["Mia", "Jonas", "Lena", "Sam", "Noah", "Zoe", "Ben", "Ella", "Theo", "Ivy", "Max"],
        think: () => 1500 + Math.random() * 5000,
        oracle: (id) => garble.game.meant(id),
      });
      return true;
    })()`);
  } else await win.eval(`garble.addBots(${Number(args.bots)}).then(() => true)`);
  await phone.until(`document.querySelectorAll(".chips .chip").length === ${Number(args.bots) + 1}`, 20_000);
  log(`${args.bots} bots joined in the window`);

  const problems: string[] = [];
  const layout = `(() => {
    const out = [];
    const W = innerWidth;
    if (document.documentElement.scrollWidth > W + 1) out.push("the page is wider than the screen: " + document.documentElement.scrollWidth);
    for (const el of document.querySelectorAll("body *")) {
      if (!el.checkVisibility || !el.checkVisibility()) continue;
      if (["svg", "path", "canvas", "input", "i"].includes(el.localName)) continue;
      const cs = getComputedStyle(el);
      const clipped = cs.textOverflow === "ellipsis" || cs.webkitLineClamp !== "none";
      if (!clipped && el.scrollWidth > el.clientWidth + 2 && el.clientWidth > 0 && cs.overflowX === "visible") out.push("too wide: " + el.localName + "." + el.className + " " + JSON.stringify((el.innerText || "").slice(0, 40)));
      const r = el.getBoundingClientRect();
      if (r.width > 0 && (r.right > W + 2 || r.left < -2) && !el.closest(".slip")) out.push("off the side: " + el.localName + "." + el.className);
    }
    return out;
  })()`;
  const check = async (where: string, tab: Tab) => {
    for (const f of await tab.eval<string[]>(layout).catch((e: Error) => [`cannot check: ${e.message.slice(0, 100)}`])) {
      const p = `${where}: ${f}`;
      if (!problems.includes(p)) {
        problems.push(p);
        log(`LAYOUT ${p}`);
      }
    }
  };
  const kept: string[] = [];
  const pic = async (tab: Tab, name: string, kind: "tv" | "phone", settle?: number) => {
    if (!args.shots || kept.some((f) => f.startsWith(`${name}.`))) return;
    kept.push(await keep(tab, args.shots, name, kind, settle));
    log(`picture: ${kept.at(-1)}`);
  };
  let judged = false;
  let n = 0;
  const shot = async (name: string, tvToo = true) => {
    const at = String(++n).padStart(2, "0");
    if (tvToo) await tv.screenshot(`${out}/tv-${at}-${name}.png`);
    await phone.screenshot(`${out}/phone-${at}-${name}.png`);
    await check(`phone ${name}`, phone);
    await phone.resize({ width: 320, height: 640, mobile: true });
    await new Promise((r) => setTimeout(r, 250));
    await check(`small phone ${name}`, phone);
    await phone.resize(PHONE);
  };
  await shot("lobby");
  await phone.until(`!document.getElementById("start")?.disabled`);
  await phone.eval(`document.getElementById("start").click(); true`);

  const state = () =>
    win.eval<{ phase: string; key: string }>(
      `(() => { const r = garble.party.room.peek(); const s = garble.party.screenView().screen ?? {}; return { phase: garble.game.phase, key: [garble.game.phase, r.round, r.step, s.index, s.stage].join(":") }; })()`,
    );
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
  const drawOnPhone = async () => {
    await phone.until(`!!document.querySelector("canvas.pad")`, 10_000);
    const choice = await phone.eval<boolean>(`!!document.querySelector(".choices button")`);
    if (choice) {
      await phone.screenshot(`${out}/phone-${String(++n).padStart(2, "0")}-choices.png`);
      await phone.eval(`document.querySelector(".choices button").click(); true`);
    }
    const box = await phone.eval<{ x: number; y: number; w: number }>(`(() => { const r = document.querySelector("canvas.pad").getBoundingClientRect(); return { x: r.left, y: r.top, w: r.width }; })()`);
    const pt = (a: number, b: number): [number, number] => [box.x + a * box.w, box.y + b * box.w];
    await phone.drag(Array.from({ length: 40 }, (_, i) => pt(0.5 + 0.28 * Math.cos((i / 39) * 2 * Math.PI), 0.52 + 0.3 * Math.sin((i / 39) * 2 * Math.PI))));
    await phone.drag(Array.from({ length: 12 }, (_, i) => pt(0.3 + i * 0.01, 0.3 - i * 0.012)));
    await phone.drag(Array.from({ length: 12 }, (_, i) => pt(0.7 - i * 0.01, 0.3 - i * 0.012)));
    await phone.drag([pt(0.42, 0.45), pt(0.425, 0.455)]);
    await phone.drag([pt(0.58, 0.45), pt(0.585, 0.455)]);
    await phone.drag(Array.from({ length: 14 }, (_, i) => pt(0.38 + (0.24 * i) / 13, 0.62 + 0.06 * Math.sin((i / 13) * Math.PI))));
  };
  const seen = new Set<string>();
  const deadline = Date.now() + 25 * 60_000;
  for (;;) {
    if (Date.now() > deadline) throw new Error("the game took too long");
    if (args.shots && kept.length >= 3) break;
    const s = await state();
    if (s.phase === "final") break;
    if (seen.has(s.key)) {
      await sleep(150);
      continue;
    }
    seen.add(s.key);
    const [phase, round, step, index, stg] = s.key.split(":");
    const tag = `r${round}-${phase}${step && phase === "play" ? `-s${step}` : ""}${index && phase === "reveal" ? `-c${index}-${stg}` : ""}`;
    log(tag);
    if (phase === "intro") {
      await sleep(1500);
      await shot(tag);
      // Hold the card: the secret.
      await phone.eval(`document.querySelector(".envelope").dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, pointerId: 1 })); true`);
      await sleep(250);
      await phone.screenshot(`${out}/phone-${String(++n).padStart(2, "0")}-${tag}-held.png`);
      await phone.eval(`document.querySelector(".envelope").dispatchEvent(new PointerEvent("pointerup", { bubbles: true, pointerId: 1 })); true`);
    } else if (phase === "play") {
      const task = await phone.eval<string>(`document.querySelector("canvas.pad") ? "draw" : document.querySelector(".guess-form") ? "guess" : ""`).catch(() => "");
      await sleep(1600);
      if (task === "draw") {
        await drawOnPhone();
        await shot(tag);
        await pic(phone, "phone-draw", "phone");
        await phone.eval(`document.getElementById("done").click(); true`);
        await sleep(700);
        await phone.screenshot(`${out}/phone-${String(++n).padStart(2, "0")}-${tag}-passed.png`);
      } else if (task === "guess") {
        await sleep(2400);
        await shot(tag);
        await phone.eval(`(() => { const i = document.querySelector(".guess-form input"); i.value = ${JSON.stringify(lang === "de" ? "eine Katze mit Hut" : "a cat in a hat")}; i.form.requestSubmit(); return true; })()`);
      } else await shot(tag);
    } else if (phase === "reveal") {
      if (stg === "show") {
        await sleep(1700);
        await shot(tag, true);
        await phone.eval(`document.querySelectorAll(".reacts button")[${Math.floor(Math.random() * 4)}]?.click(); true`).catch(() => {});
      } else if (stg === "judge") {
        await sleep(1300);
        const judging = await phone.eval<boolean>(`!!document.querySelector(".judge .link")`).catch(() => false);
        await shot(tag);
        if (judging) {
          await phone.eval(`(() => { const l = document.querySelectorAll(".judge .link"); l[Math.min(1, l.length - 1)].click(); return true; })()`);
          await sleep(300);
          await phone.screenshot(`${out}/phone-${String(++n).padStart(2, "0")}-${tag}-picked.png`);
          await pic(phone, "phone-judge", "phone", 300);
          judged = true;
          await phone.eval(`document.querySelector(".verdict-bar .btn").click(); true`);
        }
      } else if (stg === "verdict") {
        await sleep(2200);
        await tv.screenshot(`${out}/tv-${String(++n).padStart(2, "0")}-${tag}-a.png`);
        if (judged) await pic(tv, "tv-verdict", "tv", 1500); // the smuggled words drop in last
        await sleep(2600);
        await tv.screenshot(`${out}/tv-${String(++n).padStart(2, "0")}-${tag}-b.png`);
      }
    } else if (phase === "accuse") {
      await sleep(1500);
      await shot(tag);
      await phone.eval(`document.querySelector(".suspects .suspect").click(); true`);
      await sleep(200);
      await phone.screenshot(`${out}/phone-${String(++n).padStart(2, "0")}-${tag}-picked.png`);
      await phone.eval(`document.querySelector(".verdict-bar .btn").click(); true`);
    } else if (phase === "unmask") {
      await sleep(2500);
      await tv.screenshot(`${out}/tv-${String(++n).padStart(2, "0")}-${tag}-a.png`);
      await sleep(3200);
      await shot(tag);
    } else if (phase === "scores") {
      await sleep(2500);
      await shot(tag);
    }
  }
  if (args.shots && kept.length < 3) {
    log(`only these pictures: ${kept.join(", ")}`);
    failed = true;
  }
  if (!args.shots) {
    await sleep(2000);
    await shot("final");
  }
  log(`game over; layout problems: ${problems.length || "none"}; screenshots in ${out}`);
  const errors = await tv.eval<string>(`document.body.innerText.slice(0, 0)`);
  void errors;
  if (problems.length > 0) failed = true;
  await win.eval(`document.getElementById("quit").click(); true`);
  await Promise.race([exited, sleep(10_000)]);
} catch (err) {
  failed = true;
  log(`FAILED: ${(err as Error).stack ?? String(err)}`);
  writeFileSync(`${out}/tvpipe.log`, tvpipeLog.join("\n"));
} finally {
  browser.close();
  proc.kill();
}
process.exit(failed ? 1 : 0);
