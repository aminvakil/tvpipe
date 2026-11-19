/**
 * A live check of drawbluff on `tvpipe game play` (not part of `npm
 * test`): tvpipe serves the bundle (`dist/`, or `drawbluff.zip` with
 * `--zip`); a headless Chrome plays the window (the launcher, then the
 * host page), a phone (joining, drawing with touch events, lying,
 * voting) and the TV (the `browser` receiver's player page, so the
 * picture went through tvpipe's own headless Chrome, the capture and the
 * stream); bots join through the public listener. Screenshots of the
 * launcher, the host window, the TV and the phone per phase.
 *
 *   npm run build && node test/live/play.ts [--zip] [--bots N] [--rounds N] [--tunnel=PROVIDER] [--tv page|canvas] [--lang en|de] [--out DIR]
 *
 * `--lang de`: the host page sets the room to German (its Language
 * setting), the phone is a German one (`de-DE`); the screenshots go to
 * `/tmp/drawbluff-de` unless `--out`. Every shot also checks the layout
 * (the phone, also at 320 px wide, and the TV page in the host's
 * preview): no text running out of its box, nothing off the screen, no
 * English left in a German room.
 *
 * `--tv canvas`: the TV page drawn into a canvas in the window
 * (HTML-in-Canvas) and streamed as a surface instead of tvpipe's own
 * browser capturing it (docs/playshare-design.md §17).
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
    out: { type: "string" },
    tv: { type: "string", default: "page" },
    lang: { type: "string", default: "en" },
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
const lang = args.lang === "de" ? "de" : "en";
const out = `${args.out ?? (lang === "de" ? "/tmp/drawbluff-de" : "/tmp/drawbluff-play")}${args.zip ? "-zip" : ""}${args.tv === "canvas" ? "-canvas" : ""}`;
mkdirSync(out, { recursive: true });
const t0 = Date.now();
const log = (m: string) => console.log(`${((Date.now() - t0) / 1000).toFixed(1).padStart(6)} ${m}`);

// tvpipe game play, the window left to us (--window none prints its URL).
const tunnel = args.tunnel ? [`--tunnel=${args.tunnel}`] : ["--no-tunnel"];
const proc = spawn(binary, ["-v", "game", "play", bundle, "--window", "none", ...tunnel], { stdio: ["ignore", "pipe", "pipe"] });
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
log(`tvpipe game play ${args.zip ? "drawbluff.zip" : "games/drawbluff"}: ${start.replace(/t=.*/, "t=…")}`);

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
  // The room's language, as the host page sets it.
  await win.eval(`(() => { const l = document.getElementById("lang"); l.value = ${JSON.stringify(lang)}; l.dispatchEvent(new Event("change")); return true; })()`);
  await win.until(`drawbluff.game.lang === ${JSON.stringify(lang)} && document.documentElement.lang === ${JSON.stringify(lang)}`);
  log(`the room speaks ${lang}`);
  const links = await win.eval<{ join: string; phone: string; code: string }>(
    `({ join: drawbluff.party.links.join, phone: drawbluff.party.info.phone.url, code: drawbluff.party.code })`,
  );
  log(`room ${links.code}, phones join at ${links.join.replace(/#.*/, "#k=…")}`);
  // The TV: the browser receiver's player page, once the cast streams.
  await win.until(`!document.getElementById("player").hidden`, 60_000);
  if (args.tv === "canvas") {
    const before = await win.eval<string>(`document.getElementById("player").href`);
    await win.eval(`(() => { const m = document.getElementById("tvmode"); m.value = "canvas"; m.dispatchEvent(new Event("change")); return true; })()`);
    await win.until(`document.getElementById("player").href !== ${JSON.stringify(before)} && !document.getElementById("player").hidden`, 60_000);
    log("the TV picture is drawn in the window (HTML-in-Canvas) and streamed as a surface");
  }
  const playerUrl = await win.eval<string>(`document.getElementById("player").href`);
  const tv = await browser.open(playerUrl, { width: 1280, height: 720 });
  log(`TV: ${playerUrl.replace(/t=.*/, "t=…")}`);
  // A phone (through the phones' listener, as the QR code says).
  const phoneUrl = links.join.startsWith("https://") ? links.join : links.join;
  const PHONE = { width: 390, height: 844, mobile: true };
  const phone = await browser.open(phoneUrl, { ...PHONE, locale: lang === "de" ? "de-DE" : "en-US" });
  await phone.until(`!!document.getElementById("name")`, 20_000);
  await phone.screenshot(`${out}/phone-join.png`);
  log(`phone: navigator.language ${await phone.eval<string>("navigator.language")}, the join form says "${await phone.eval<string>(`document.querySelector("h1").textContent`)}"`);
  await phone.eval(`document.getElementById("name").value = "Phil"; document.querySelector("#join-form button").click(); true`);
  await phone.until(`!!document.querySelector(".card.lobby") && document.documentElement.lang === ${JSON.stringify(lang)}`);
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
  const problems: string[] = [];
  /**
   * Layout problems in a document (`doc`: an expression): text running
   * out of its box (but where it ends with … on purpose), boxes off the
   * screen, and in a German room English words left.
   */
  const layout = (doc: string) => `(() => {
    const doc = ${doc};
    if (!doc) return ["no document"];
    const win = doc.defaultView;
    const out = [];
    const W = win.innerWidth, H = win.innerHeight;
    if (doc.documentElement.scrollWidth > W + 1) out.push("the page is wider than the screen: " + doc.documentElement.scrollWidth);
    for (const el of doc.querySelectorAll("body *")) {
      if (!el.checkVisibility || !el.checkVisibility()) continue;
      if (["svg", "path", "rect", "canvas", "i", "input", "select", "option", "template"].includes(el.localName)) continue;
      const cs = win.getComputedStyle(el);
      const text = (el.innerText || "").trim().slice(0, 60);
      const clipped = cs.textOverflow === "ellipsis" || cs.webkitLineClamp !== "none" || cs.overflow === "hidden" && el.closest(".qr, #corner-qr");
      if (!clipped && el.scrollWidth > el.clientWidth + 2 && el.clientWidth > 0 && cs.overflowX !== "auto" && cs.overflowX !== "scroll") out.push("too wide: " + el.localName + "." + el.className + " " + JSON.stringify(text));
      const r = el.getBoundingClientRect();
      if (r.width > 0 && (r.right > W + 2 || r.left < -2)) out.push("off the side: " + el.localName + "." + el.className + " " + JSON.stringify(text));
    }
    ${
      lang === "de"
        ? `const words = (doc.body.innerText.match(/\\b(the|you|your|you're|waiting|players?|round|drawing|liars?|vote[sd]?|scores?|points?|found|fooled|meanwhile|which|title)\\b/gi) || []);
    if (words.length) out.push("English left: " + [...new Set(words)].join(", "));`
        : ""
    }
    return out;
  })()`;
  const check = async (where: string, tab: Tab, doc = "document") => {
    const found = await tab.eval<string[]>(layout(doc)).catch((e: Error) => [`cannot check: ${e.message.slice(0, 120)}`]);
    for (const f of found) {
      const p = `${where}: ${f}`;
      if (!problems.includes(p)) {
        problems.push(p);
        log(`LAYOUT ${p}`);
      }
    }
  };
  const shot = async (name: string) => {
    await new Promise((r) => setTimeout(r, 1500)); // the TV is a stream: a little behind
    await tv.screenshot(`${out}/tv-${name}.png`);
    await win.screenshot(`${out}/host-${name}.png`);
    await phone.screenshot(`${out}/phone-${name}.png`);
    await check(`phone ${name}`, phone);
    // The TV page itself, in the host's preview (the TV's 1280×720).
    await check(`tv ${name}`, win, `document.getElementById("preview").contentDocument`);
    // A small phone (320 px wide, an iPhone SE): long German words.
    await phone.resize({ width: 320, height: 640, mobile: true });
    await new Promise((r) => setTimeout(r, 300));
    await phone.screenshot(`${out}/phone-small-${name}.png`);
    await check(`small phone ${name}`, phone);
    await phone.resize(PHONE);
    shots.push(name);
    log(`screenshots: ${name}`);
  };
  await shot("lobby");
  // The phone's own language switch (in the lobby), there and back.
  const other = lang === "de" ? "en" : "de";
  await phone.eval(`document.getElementById("lang").click(); true`);
  await phone.until(`document.documentElement.lang === ${JSON.stringify(other)} && !!document.querySelector(".card.lobby")`);
  await phone.screenshot(`${out}/phone-lobby-switched.png`);
  await phone.eval(`document.getElementById("lang").click(); true`);
  await phone.until(`document.documentElement.lang === ${JSON.stringify(lang)} && localStorage.getItem("drawbluff:lang") === null`);
  log(`phone: switched to ${other} and back to the room's ${lang}`);
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
      await phone.eval(`(() => { const i = document.querySelector("form input"); i.value = ${JSON.stringify(lang === "de" ? "ein verdächtiger Streuselkuchen" : "a suspicious muffin")}; i.form.requestSubmit(); return true; })()`);
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
    const k = await key();
    if (seen.has(k)) {
      await drivePhone();
      await new Promise((r) => setTimeout(r, 200));
      continue;
    }
    seen.add(k);
    const p = k.split(":")[0]!;
    const at = String(++n).padStart(2, "0");
    if (p === "draw") {
      await draw(phone);
      await shot(`${at}-draw`);
      await phone.eval(`document.getElementById("done").click(); true`);
    } else if (p === "lie" || p === "vote" || p === "scores" || p === "gallery") {
      // The phone's input or options first, then its answer.
      await new Promise((r) => setTimeout(r, 400));
      await shot(`${at}-${p}`);
      await drivePhone();
    } else if (p === "reveal") {
      await new Promise((r) => setTimeout(r, 3000));
      await shot(`${at}-reveal`);
    }
  }
  await shot("final");
  const errors = robots.flatMap((b) => b.errors);
  log(`game over: ${shots.length} screenshot sets in ${out}; bot errors: ${errors.length ? errors.join(", ") : "none"}; layout problems: ${problems.length || "none"}`);
  if (problems.length > 0) failed = true;
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
