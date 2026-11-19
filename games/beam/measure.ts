/**
 * Measures beam's two paths (not part of any test suite; Node for the
 * harness only): `tvpipe playshare games/beam` to the `browser` receiver,
 * the player page in a second headless Chrome reading the clock row of
 * every picture it shows (glass-to-glass latency including the player's
 * playout delay, and the frames that really arrive), CPU time of tvpipe
 * with its own browser and of the window's browser, and tvpipe's own
 * numbers for surfaces (fps, lag from the page to tvpipe).
 *
 *   node games/beam/measure.ts [--secs 15] [--only NAME] [--headed] [--json FILE]
 *
 * Runs: canvas-raf, canvas-timer, canvas-worker, tab, canvas-1080p60,
 * two-surfaces; with --headed also the hidden-window runs (a real
 * window, minimized; with and without Chrome's anti-throttling switches),
 * firefox and safari (they open real windows).
 */

import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { existsSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const { values: args } = parseArgs({
  options: {
    secs: { type: "string", default: "15" },
    only: { type: "string" },
    headed: { type: "boolean", default: false },
    json: { type: "string" },
  },
});
const SECS = Number(args.secs);
const repo = fileURLToPath(new URL("../../", import.meta.url));
const tvpipeBin = process.env["TVPIPE_BINARY"] ?? [`${repo}target/release/tvpipe`, `${repo}target/debug/tvpipe`].find(existsSync)!;
const CHROME = process.env["CHROME"] ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const FIREFOX = "/Applications/Firefox.app/Contents/MacOS/firefox";
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const log = (m: string) => console.error(m);

// ---------------------------------------------------------------------------
// A minimal DevTools client

class Tab {
  #id = 0;
  #pending = new Map<number, (m: any) => void>();
  readonly ws: WebSocket;
  constructor(ws: WebSocket) {
    this.ws = ws;
    ws.onmessage = (e) => {
      const m = JSON.parse(String(e.data));
      this.#pending.get(m.id)?.(m);
    };
  }
  send(method: string, params: Record<string, unknown> = {}): Promise<any> {
    const id = ++this.#id;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((r) => this.#pending.set(id, (m) => r(m.result ?? m)));
  }
  async eval<T>(expression: string): Promise<T> {
    const r = await this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, userGesture: true });
    if (r.exceptionDetails) throw new Error(`${expression.slice(0, 80)}: ${JSON.stringify(r.exceptionDetails).slice(0, 300)}`);
    return r.result?.value as T;
  }
  async until(expression: string, ms = 30_000): Promise<void> {
    const end = Date.now() + ms;
    while (!(await this.eval(expression).catch(() => false))) {
      if (Date.now() > end) throw new Error(`timed out: ${expression}`);
      await sleep(200);
    }
  }
}

class Chrome {
  readonly proc: ChildProcess;
  readonly port: number;
  constructor(proc: ChildProcess, port: number) {
    this.proc = proc;
    this.port = port;
  }
  static async launch(extra: string[] = [], headed = false): Promise<Chrome> {
    const port = 9400 + Math.floor(Math.random() * 400);
    const dir = mkdtempSync(join(tmpdir(), "beam-chrome-"));
    const proc = spawn(
      CHROME,
      [
        ...(headed ? ["--window-position=40,40", "--window-size=1100,760"] : ["--headless=new"]),
        `--remote-debugging-port=${port}`,
        `--user-data-dir=${dir}`,
        "--no-first-run",
        "--no-default-browser-check",
        "--autoplay-policy=no-user-gesture-required",
        ...extra,
        "about:blank",
      ],
      { stdio: "ignore" },
    );
    for (let i = 0; i < 150; i++) {
      try {
        await fetch(`http://127.0.0.1:${port}/json/version`);
        return new Chrome(proc, port);
      } catch {
        await sleep(100);
      }
    }
    throw new Error("Chrome did not start");
  }
  async open(url: string, size = { width: 1280, height: 720 }): Promise<Tab> {
    const t = (await (await fetch(`http://127.0.0.1:${this.port}/json/new?about:blank`, { method: "PUT" })).json()) as { webSocketDebuggerUrl: string; id: string };
    const ws = new WebSocket(t.webSocketDebuggerUrl);
    await new Promise((r) => (ws.onopen = r));
    const tab = new Tab(ws);
    await tab.send("Page.enable");
    await tab.send("Emulation.setDeviceMetricsOverride", { ...size, deviceScaleFactor: 1, mobile: false });
    await tab.send("Page.navigate", { url });
    (tab as any).targetId = t.id;
    return tab;
  }
  async minimize(tab: Tab): Promise<void> {
    const v = (await (await fetch(`http://127.0.0.1:${this.port}/json/version`)).json()) as { webSocketDebuggerUrl: string };
    const ws = new WebSocket(v.webSocketDebuggerUrl);
    await new Promise((r) => (ws.onopen = r));
    const b = new Tab(ws);
    const { windowId } = await b.send("Browser.getWindowForTarget", { targetId: (tab as any).targetId });
    await b.send("Browser.setWindowBounds", { windowId, bounds: { windowState: "minimized" } });
    ws.close();
  }
  close(): void {
    this.proc.kill();
  }
}

// ---------------------------------------------------------------------------
// CPU time of process trees (ps: cumulative user+system time)

function cpuTimes(): Map<number, { ppid: number; secs: number; cmd: string }> {
  const out = execFileSync("ps", ["-A", "-o", "pid=,ppid=,time=,command="], { encoding: "utf8", maxBuffer: 64 << 20 });
  const m = new Map<number, { ppid: number; secs: number; cmd: string }>();
  for (const line of out.split("\n")) {
    const p = /^\s*(\d+)\s+(\d+)\s+(\S+)\s+(.*)$/.exec(line);
    if (!p) continue;
    const parts = p[3]!.split(/[:-]/).map(Number);
    let secs = 0;
    for (const x of parts) secs = secs * 60 + x;
    m.set(Number(p[1]), { ppid: Number(p[2]), secs, cmd: p[4]! });
  }
  return m;
}

/** CPU seconds of the trees under `roots` and of processes whose command line has `needle`. */
function treeCpu(t: ReturnType<typeof cpuTimes>, roots: number[], needle?: string): number {
  const inTree = (pid: number): boolean => {
    for (let p = pid, n = 0; p > 1 && n < 50; n++) {
      if (roots.includes(p)) return true;
      p = t.get(p)?.ppid ?? 0;
    }
    return false;
  };
  let sum = 0;
  for (const [pid, v] of t) if (inTree(pid) || (needle && v.cmd.includes(needle))) sum += v.secs;
  return sum;
}

// ---------------------------------------------------------------------------
// The player page's reader of the clock row

const SAMPLER = `(() => {
  const c = document.getElementById("video");
  const g = c.getContext("2d");
  const S = { x: 16, y: 8, size: 20, gap: 2, bits: 48 };
  const st = (globalThis.__beam = { lat: [], frames: 0, bad: 0, last: 0, t0: performance.now() });
  const read = () => {
    if (c.width >= 640) {
      // The row is drawn in the stream's pixels (canvas or page size).
      const scale = 1;
      const row = g.getImageData(0, Math.round((S.y + S.size / 2) * scale), c.width, 1).data;
      let ms = 0, ok = true;
      for (let i = 0; i < S.bits && ok; i++) {
        const o = Math.round((S.x + i * (S.size + S.gap) + S.size / 2) * scale) * 4;
        const lum = (row[o] + row[o + 1] + row[o + 2]) / 3;
        if (lum > 80 && lum < 176) { ok = false; break; }
        const on = lum > 128;
        if (i < 2) ok = on === (i === 0);
        else if (i >= S.bits - 2) ok = on === (i === S.bits - 1);
        else ms = ms * 2 + (on ? 1 : 0);
      }
      if (!ok) st.bad++;
      else if (ms !== st.last) {
        st.last = ms;
        st.frames++;
        st.lat.push(Date.now() - ms);
      }
    }
    requestAnimationFrame(read);
  };
  requestAnimationFrame(read);
  return true;
})()`;

// ---------------------------------------------------------------------------

interface Run {
  name: string;
  /** host page parameters */
  query: string;
  /** the window: headless Chrome (default), a headed one minimized, Firefox, Safari */
  window?: "headless" | "hidden" | "hidden-plain" | "firefox" | "safari";
  /** which player(s) to read */
  players?: string[];
}

const RUNS: Run[] = [
  { name: "canvas-raf", query: "path=canvas&clock=raf&fps=30&audio=1" },
  { name: "canvas-raf-mute", query: "path=canvas&clock=raf&fps=30&audio=0" },
  { name: "canvas-timer", query: "path=canvas&clock=timer&fps=30&audio=1" },
  { name: "canvas-worker", query: "path=canvas&clock=worker&fps=30&audio=0" },
  { name: "tab", query: "path=tab&fps=30" },
  { name: "canvas-1080p60", query: "path=canvas&clock=raf&fps=60&size=1920x1080&audio=1" },
  { name: "two-surfaces", query: "path=canvas&clock=raf&fps=30&audio=1&second=same", players: ["screen1Player", "screen2Player"] },
];
const HEADED: Run[] = [
  { name: "hidden-raf", query: "path=canvas&clock=raf&fps=30&audio=1", window: "hidden" },
  { name: "hidden-timer", query: "path=canvas&clock=timer&fps=30&audio=1", window: "hidden" },
  { name: "hidden-worker", query: "path=canvas&clock=worker&fps=30&audio=0", window: "hidden" },
  { name: "hidden-plain-raf", query: "path=canvas&clock=raf&fps=30&audio=1", window: "hidden-plain" },
  { name: "hidden-plain-timer", query: "path=canvas&clock=timer&fps=30&audio=1", window: "hidden-plain" },
  { name: "hidden-plain-worker", query: "path=canvas&clock=worker&fps=30&audio=0", window: "hidden-plain" },
  { name: "firefox-raf", query: "path=canvas&clock=raf&fps=30&audio=1", window: "firefox" },
  { name: "safari-raf", query: "path=canvas&clock=raf&fps=30&audio=1", window: "safari" },
];

/** Chrome's switches that playshare's own window has. */
const UNTHROTTLED = ["--disable-background-timer-throttling", "--disable-renderer-backgrounding", "--disable-backgrounding-occluded-windows"];

function pct(xs: number[], p: number): number {
  if (xs.length === 0) return NaN;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))]!;
}

async function measure(run: Run): Promise<Record<string, unknown>> {
  const tv = spawn(tvpipeBin, [process.env["BEAM_DEBUG"] ? "--debug" : "-v", "playshare", `${repo}games/beam`, "--window", "none", "--no-tunnel"], { stdio: ["ignore", "pipe", "pipe"] });
  const lines: string[] = [];
  createInterface({ input: tv.stderr! }).on("line", (l) => lines.push(l));
  const start = await new Promise<string>((ok) => createInterface({ input: tv.stdout! }).once("line", ok));
  const next = encodeURIComponent(`/index.html?${run.query}&to=browser&autostart=1`);
  const url = `${start}&next=${next}`;
  // One viewer browser per player page: a background tab gets no frames.
  const viewers: Chrome[] = [];
  let win: Chrome | undefined;
  let other: ChildProcess | undefined;
  const result: Record<string, unknown> = { name: run.name };
  try {
    let host: Tab | undefined;
    const w = run.window ?? "headless";
    if (w === "headless" || w === "hidden" || w === "hidden-plain") {
      win = await Chrome.launch(w === "hidden-plain" ? [] : UNTHROTTLED, w !== "headless");
      host = await win.open(url, { width: 1100, height: 760 });
    } else if (w === "firefox") {
      const dir = mkdtempSync(join(tmpdir(), "beam-ff-"));
      writeFileSync(join(dir, "user.js"), 'user_pref("media.autoplay.default", 0);\nuser_pref("media.autoplay.blocking_policy", 0);\n');
      other = spawn(FIREFOX, ["--headless", "--no-remote", "--profile", dir, "--window-size=1100,760", url], { stdio: "ignore" });
    } else if (w === "safari") {
      execFileSync("open", ["-g", "-a", "Safari", url]);
    }
    // The player pages, from the host page or tvpipe's log.
    const players: string[] = [];
    const deadline = Date.now() + 60_000;
    const keys = run.players ?? ["screen1Player"];
    while (players.length < keys.length && Date.now() < deadline) {
      if (host) {
        const st = await host.eval<any>("globalThis.beam ? { s: beam.status, c: beam.cast } : null").catch(() => null);
        const found = run.query.includes("path=tab") ? (st?.c?.player ? [st.c.player] : []) : keys.map((k) => st?.s?.[k]).filter(Boolean);
        if (found.length === keys.length) players.push(...found);
      } else {
        const urls = [...new Set(lines.flatMap((l) => /browser player at (\S+)/.exec(l)?.[1] ?? /(http:\/\/127\.0\.0\.1:\d+\/\?t=\w+)/.exec(l)?.[1] ?? []))];
        if (urls.length >= keys.length) players.push(...urls.slice(0, keys.length));
      }
      await sleep(300);
    }
    if (players.length < keys.length) throw new Error(`no player page (${run.name})`);
    const tabs: Tab[] = [];
    for (const p of players) {
      const viewer = await Chrome.launch();
      viewers.push(viewer);
      const t = await viewer.open(p);
      await t.until(`!!document.getElementById("video")`);
      await t.eval(SAMPLER);
      tabs.push(t);
    }
    await sleep(3000);
    if (win && host && (run.window === "hidden" || run.window === "hidden-plain")) {
      await win.minimize(host);
      await sleep(2000);
      result["hidden"] = await host.eval("document.visibilityState");
    }
    for (const t of tabs) await t.eval("(() => { const s = __beam; s.lat = []; s.frames = 0; s.bad = 0; s.t0 = performance.now(); return 1; })()");
    const winPid = win?.proc.pid ?? other?.pid ?? -1;
    const before = cpuTimes();
    const t0 = Date.now();
    await sleep(SECS * 1000);
    const after = cpuTimes();
    const secs = (Date.now() - t0) / 1000;
    const tvRoots = [tv.pid!];
    const winRoots = [winPid];
    result["cpu"] = {
      tvpipePct: Math.round(((treeCpu(after, tvRoots) - treeCpu(before, tvRoots)) / secs) * 100),
      windowPct:
        run.window === "safari"
          ? Math.round(((treeCpu(after, [], "Safari") + treeCpu(after, [], "WebKit") - treeCpu(before, [], "Safari") - treeCpu(before, [], "WebKit")) / secs) * 100)
          : Math.round(((treeCpu(after, winRoots) - treeCpu(before, winRoots)) / secs) * 100),
    };
    const reads = [];
    for (const t of tabs) {
      const s = await t.eval<{ lat: number[]; frames: number; bad: number }>("__beam");
      if (s.frames === 0) (result["playerText"] ??= []) && (result["playerText"] as string[]).push(await t.eval<string>("document.body.innerText.slice(0, 200) + ' ' + document.getElementById('video').width"));
      reads.push({ fps: Math.round((s.frames / secs) * 10) / 10, latMs: { p50: pct(s.lat, 50), p95: pct(s.lat, 95), max: pct(s.lat, 100) }, unreadable: s.bad });
    }
    result["player"] = reads.length === 1 ? reads[0] : reads;
    if (host) {
      const st = await host.eval<any>("({ stats: beam.stats, codec: beam.codec, errors: beam.errors, hidden: document.visibilityState })");
      result["surface"] = st;
    } else {
      result["tvpipeLog"] = [
        ...lines.filter((l) => / in Mozilla/.test(l)).slice(0, 1),
        ...lines.filter((l) => /surface \d+: \{/.test(l)).slice(-3),
      ].map((l) => l.replace(/^\s*\S+ \S+ \S+ /, ""));
    }
    if (process.env["BEAM_LOG"]) result["log"] = lines.filter((l) => new RegExp(process.env["BEAM_LOG"]!).test(l)).slice(0, 40);
  } catch (e) {
    result["error"] = String(e);
    result["log"] = lines.slice(-15);
  } finally {
    tv.kill("SIGINT");
    await new Promise((r) => tv.on("exit", r));
    win?.close();
    other?.kill();
    for (const v of viewers) v.close();
    if (run.window === "safari") {
      try {
        execFileSync("osascript", ["-e", 'tell application "Safari" to close (every tab of every window whose URL contains "autostart=1")']);
      } catch {}
    }
  }
  return result;
}

const runs = [...RUNS, ...(args.headed ? HEADED : [])].filter((r) => !args.only || args.only.split(",").includes(r.name));
const results = [];
for (const r of runs) {
  log(`== ${r.name}`);
  const res = await measure(r);
  log(JSON.stringify(res));
  results.push(res);
}
if (args.json) writeFileSync(args.json, JSON.stringify(results, null, 1));
process.exit(0);
