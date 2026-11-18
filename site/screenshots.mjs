// Makes the screenshots in site/shots/ from a running tvpipe.
//
//     node site/screenshots.mjs [path/to/tvpipe]
//
// Needs Node 22+ (fetch and WebSocket built in) and Google Chrome. Starts
// `tvpipe preview` (the test pattern in the browser player) and `tvpipe share
// --to browser`, opens their pages in headless Chrome over the DevTools
// protocol and saves:
//   test-pattern.png   the player with the test pattern
//   player-stats.png   the same with the statistics overlay (S)
//   share-page.png     the share page before sharing, light
//   share-page.dark.png  the same, dark (the site shows the one matching its theme)
import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const tvpipe = process.argv[2] || join(here, "..", "target", "release", "tvpipe");
const chromePath = process.env.CHROME || [
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/usr/bin/google-chrome", "/usr/bin/chromium",
].find(existsSync);
if (!existsSync(tvpipe)) throw new Error(`no tvpipe at ${tvpipe}; run cargo build --release`);
if (!chromePath) throw new Error("no Chrome found; set CHROME");

const sleep = ms => new Promise(r => setTimeout(r, ms));
const children = [];
process.on("exit", () => children.forEach(c => c.kill()));

// Starts a tvpipe command and resolves with the http://127.0.0.1 URL it logs.
function start(args) {
  return new Promise((resolve, reject) => {
    const child = spawn(tvpipe, args, { stdio: ["ignore", "ignore", "pipe"] });
    children.push(child);
    let log = "";
    child.stderr.on("data", d => {
      log += d;
      const m = log.match(/http:\/\/127\.0\.0\.1:\d+\/\?t=[0-9a-f]+/);
      if (m) resolve(m[0]);
    });
    child.on("exit", code => reject(new Error(`tvpipe ${args.join(" ")} exited (${code}):\n${log}`)));
  });
}

async function chrome() {
  const port = 9300 + Math.floor(Math.random() * 500);
  const proc = spawn(chromePath, [
    "--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${mkdtempSync(join(tmpdir(), "tvpipe-shots-"))}`,
    "--hide-scrollbars", "--autoplay-policy=no-user-gesture-required", "--no-first-run", "about:blank",
  ], { stdio: "ignore" });
  children.push(proc);
  let targets = [];
  for (let i = 0; i < 100 && !targets.length; i++) {
    try { targets = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).filter(t => t.type === "page"); }
    catch { await sleep(100); }
  }
  const ws = new WebSocket(targets[0].webSocketDebuggerUrl);
  await new Promise(r => ws.addEventListener("open", r));
  let id = 0;
  const pending = new Map();
  ws.addEventListener("message", e => {
    const m = JSON.parse(e.data);
    if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id); }
  });
  const send = (method, params = {}) => new Promise(r => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
  return {
    async shot(url, file, { wait = 4000, js = null, scheme = "light", width = 1280, height = 720, scale = 1 } = {}) {
      await send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: scale, mobile: false });
      await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: scheme }] });
      await send("Page.navigate", { url });
      await sleep(wait);
      if (js) { await send("Runtime.evaluate", { expression: js }); await sleep(1200); }
      const { data } = await send("Page.captureScreenshot", { format: "png" });
      writeFileSync(join(here, "shots", file), Buffer.from(data, "base64"));
      console.log("wrote", join("site", "shots", file));
    },
  };
}

const browser = await chrome();
const player = await start(["preview", "--port", "0"]);
await browser.shot(player, "test-pattern.png");
await browser.shot(player, "player-stats.png", { wait: 6000, js: "document.dispatchEvent(new KeyboardEvent('keydown', { key: 's' }))" });
const share = await start(["share", "--to", "browser", "--port", "0"]);
// The page without the empty space below it, sharp on high-density screens.
const page = { wait: 2500, width: 960, height: 470, scale: 2 };
await browser.shot(share, "share-page.png", page);
await browser.shot(share, "share-page.dark.png", { ...page, scheme: "dark" });
process.exit(0);
