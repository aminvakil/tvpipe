/**
 * A live check of drawbluff on `tvpipe playshare` (not part of `npm
 * test`): tvpipe serves the bundle (`dist/`, or `drawbluff.zip` with
 * `--zip`); a headless Chrome plays the window (the launcher, then the
 * host page), a phone (joining, drawing with touch events, lying,
 * voting) and the TV (the `browser` receiver's player page, so the
 * picture went through tvpipe's own headless Chrome, the capture and the
 * stream); bots join through the public listener. Screenshots of the
 * launcher, the host window, the TV and the phone per phase.
 *
 *   npm run build && node test/live/playshare.ts [--zip] [--bots N] [--rounds N] [--tunnel=PROVIDER] [--out DIR]
 *
 * Needs Chrome and tvpipe built (target/release, else target/debug, or
 * TVPIPE_BINARY).
 */

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import { bots } from "../../game/bots.ts";
import { Browser, type Tab } from "./browser.ts";

const { values: args } = parseArgs({
  options: {
    zip: { type: "boolean", default: false },
    bots: { type: "string", default: "2" },
    rounds: { type: "string", default: "1" },
    tunnel: { type: "string" },
    out: { type: "string", default: "/tmp/drawbluff-playshare" },
  },
});

const here = new URL("../../", import.meta.url);
const repo = new URL("../../", here);
const exe = process.platform === "win32" ? "tvpipe.exe" : "tvpipe";
const binary =
  process.env["TVPIPE_BINARY"] ??
  ["release", "debug"].map((p) => fileURLToPath(new URL(`target/${p}/${exe}`, repo))).find((p) => existsSync(p));
if (!binary) throw new Error("build tvpipe first (cargo build --release -p tvpipe-cli)");
const bundle = fileURLToPath(args.zip ? new URL("drawbluff.zip", here) : here);
const out = `${args.out}${args.zip ? "-zip" : ""}`;
mkdirSync(out, { recursive: true });
const t0 = Date.now();
const log = (m: string) => console.log(`${((Date.now() - t0) / 1000).toFixed(1).padStart(6)} ${m}`);

// tvpipe playshare, the window left to us (--window none prints its URL).
const tunnel = args.tunnel ? [`--tunnel=${args.tunnel}`] : ["--no-tunnel"];
const proc = spawn(binary, ["-v", "playshare", bundle, "--window", "none", ...tunnel], { stdio: ["ignore", "pipe", "pipe"] });
const exited = new Promise<number | null>((r) => proc.on("exit", (c) => r(c)));
const tvpipeLog: string[] = [];
createInterface({ input: proc.stderr! }).on("line", (l) => {
  tvpipeLog.push(l);
  if (/ (WARN|ERROR) |playshare: /.test(l)) log(`tvpipe: ${l.replace(/^\s*\S+ /, "")}`);
});
const start = await new Promise<string>((resolve, reject) => {
  createInterface({ input: proc.stdout! }).once("line", resolve);
  exited.then(() => reject(new Error(`tvpipe exited:\n${tvpipeLog.join("\n")}`)));
});
log(`tvpipe playshare ${args.zip ? "drawbluff.zip" : "games/drawbluff"}: ${start.replace(/t=.*/, "t=…")}`);

const browser = await Browser.launch();
let failed = false;
try {
  // The window: the launcher first.
  const win = await browser.open(start, { width: 1100, height: 760 });
  await win.until(`document.querySelectorAll("#devices .device").length > 0`);
  await new Promise((r) => setTimeout(r, 1500)); // a discovery round: real TVs show up too
  await win.screenshot(`${out}/launcher.png`);
  const listed = await win.eval<string[]>(`[...document.querySelectorAll("#devices .device")].map((d) => d.dataset.name)`);
  log(`launcher lists: ${listed.join(", ")}`);
  // Only the browser receiver (no real TV without asking).
  await win.eval(`document.querySelector('#devices .device[data-name="browser"]').click(); true`);
  await win.until(`!document.getElementById("start").disabled`);
  await win.eval(`document.getElementById("start").click(); true`);
  await win.until(`location.pathname.endsWith("/host/index.html") && !!globalThis.drawbluff`, 20_000);
  log("the host page runs the game");
  await win.eval(`(() => { const r = document.getElementById("rounds"); r.value = ${JSON.stringify(args.rounds)}; r.dispatchEvent(new Event("change")); const t = document.getElementById("timers"); t.value = "fast"; t.dispatchEvent(new Event("change")); return true; })()`);
  const links = await win.eval<{ join: string; phone: string; code: string }>(
    `({ join: drawbluff.party.links.join, phone: drawbluff.party.info.phone.url, code: drawbluff.party.code })`,
  );
  log(`room ${links.code}, phones join at ${links.join.replace(/#.*/, "#k=…")}`);
  // The TV: the browser receiver's player page, once the cast streams.
  await win.until(`!document.getElementById("player").hidden`, 60_000);
  const playerUrl = await win.eval<string>(`document.getElementById("player").href`);
  const tv = await browser.open(playerUrl, { width: 1280, height: 720 });
  log(`TV: ${playerUrl.replace(/t=.*/, "t=…")}`);
  // A phone (through the phones' listener, as the QR code says).
  const phoneUrl = links.join.startsWith("https://") ? links.join : links.join;
  const phone = await browser.open(phoneUrl, { width: 390, height: 844, mobile: true });
  await phone.until(`!!document.getElementById("name")`, 20_000);
  await phone.eval(`document.getElementById("name").value = "Phil"; document.querySelector("#join-form button").click(); true`);
  await phone.until(`document.querySelector("h1")?.textContent === "You're in!"`);
  const ws = `${links.phone.replace(/^http/, "ws")}_tvpipe/ws`;
  const u = new URL(links.join);
  const robots = await bots(Number(args.bots), {
    url: ws,
    room: u.searchParams.get("r")!,
    key: new URLSearchParams(u.hash.slice(1)).get("k")!,
    think: () => 1200 + Math.random() * 2000,
  });
  log(`${robots.length} bots joined through the phones' listener`);

  const shots: string[] = [];
  const shot = async (name: string) => {
    await new Promise((r) => setTimeout(r, 1500)); // the TV is a stream: a little behind
    await tv.screenshot(`${out}/tv-${name}.png`);
    await win.screenshot(`${out}/host-${name}.png`);
    await phone.screenshot(`${out}/phone-${name}.png`);
    shots.push(name);
    log(`screenshots: ${name}`);
  };
  await shot("lobby");
  // Phil (the VIP) starts from the phone.
  await phone.until(`!document.querySelector("button.primary")?.disabled`);
  await phone.eval(`document.querySelector("button.primary").click(); true`);

  const phase = () => win.eval<string>(`drawbluff.game.phase`);
  const key = () => win.eval<string>(`drawbluff.game.phase + ":" + JSON.stringify([drawbluff.party.room.peek().round, drawbluff.party.room.peek().spot])`);
  const draw = async (tab: Tab) => {
    await tab.until(`!!document.querySelector("canvas.pad")`, 30_000);
    const box = await tab.eval<{ x: number; y: number; w: number }>(
      `(() => { const r = document.querySelector("canvas.pad").getBoundingClientRect(); return { x: r.left, y: r.top, w: r.width }; })()`,
    );
    const pt = (a: number, b: number): [number, number] => [box.x + a * box.w, box.y + b * box.w];
    await tab.drag(Array.from({ length: 30 }, (_, i) => pt(0.5 + 0.3 * Math.cos((i / 29) * 2 * Math.PI), 0.5 + 0.3 * Math.sin((i / 29) * 2 * Math.PI))));
    await tab.eval(`document.querySelectorAll(".palette button")[2].click(); true`);
    await tab.drag([pt(0.38, 0.4), pt(0.4, 0.42)]);
    await tab.drag([pt(0.6, 0.4), pt(0.62, 0.42)]);
    await tab.drag(Array.from({ length: 12 }, (_, i) => pt(0.35 + (0.3 * i) / 11, 0.6 + 0.08 * Math.sin((i / 11) * Math.PI))));
  };
  const tasks = new Set<string>();
  const drivePhone = async () => {
    const t = await phone
      .eval<{ lie: boolean; vote: number; key: string }>(
        `(() => { const i = document.querySelector("form input"); const o = document.querySelectorAll(".options button:not(.chosen)"); const c = document.querySelector(".options .chosen"); return { lie: !!i, vote: c ? 0 : o.length, key: document.querySelector("main")?.innerText.slice(0, 200) ?? "" }; })()`,
      )
      .catch(() => ({ lie: false, vote: 0, key: "" }));
    if (t.lie && !tasks.has(`lie:${t.key}`)) {
      tasks.add(`lie:${t.key}`);
      await phone.eval(`(() => { const i = document.querySelector("form input"); i.value = "a suspicious muffin"; i.form.requestSubmit(); return true; })()`);
      log("phone: lied");
    } else if (t.vote > 0 && !tasks.has(`vote:${t.key}`)) {
      tasks.add(`vote:${t.key}`);
      await phone.eval(`(() => { const o = document.querySelectorAll(".options button"); o[0].click(); return true; })()`);
      log(`phone: voted (${t.vote} options)`);
    }
  };
  const seen = new Set<string>();
  let n = 0;
  const deadline = Date.now() + 10 * 60_000;
  while ((await phase()) !== "final") {
    if (Date.now() > deadline) throw new Error("the game took too long");
    await drivePhone();
    const k = await key();
    if (seen.has(k)) {
      await new Promise((r) => setTimeout(r, 200));
      continue;
    }
    seen.add(k);
    const p = k.split(":")[0]!;
    const at = String(++n).padStart(2, "0");
    if (p === "draw") {
      await draw(phone);
      await shot(`${at}-draw`);
      await phone.eval(`[...document.querySelectorAll("button")].find((b) => b.textContent === "Done!").click(); true`);
    } else if (p === "lie" || p === "vote" || p === "scores" || p === "gallery") {
      await new Promise((r) => setTimeout(r, 400));
      await drivePhone();
      await shot(`${at}-${p}`);
    } else if (p === "reveal") {
      await new Promise((r) => setTimeout(r, 3000));
      await shot(`${at}-reveal`);
    }
  }
  await shot("final");
  const errors = robots.flatMap((b) => b.errors);
  log(`game over: ${shots.length} screenshot sets in ${out}; bot errors: ${errors.length ? errors.join(", ") : "none"}`);
  // The End button: tvpipe ends, everyone is told.
  await win.eval(`document.getElementById("quit").click(); true`);
  const code = await Promise.race([exited, new Promise<string>((r) => setTimeout(() => r("still running"), 15_000))]);
  await phone.until(`document.body.innerText.toLowerCase().includes("closed") || document.body.innerText.toLowerCase().includes("over")`, 5000).catch(() => {});
  await phone.screenshot(`${out}/phone-after-end.png`);
  log(`tvpipe exited: ${code}; the phone says: ${(await phone.eval<string>(`document.querySelector("main")?.innerText.slice(0, 80) ?? ""`)).replace(/\n/g, " / ")}`);
  for (const b of robots) b.close();
  if (code !== 0) failed = true;
} catch (err) {
  failed = true;
  log(`FAILED: ${(err as Error).stack ?? String(err)}`);
  writeFileSync(`${out}/tvpipe.log`, tvpipeLog.join("\n"));
} finally {
  browser.close();
  proc.kill();
}
process.exit(failed ? 1 : 0);
