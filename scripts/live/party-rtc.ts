/**
 * Live check of the WebRTC fast path (not part of any test suite): builds
 * and runs `tvpipe-party`'s example `stick` with the `webrtc` feature
 * (default `lan`), opens two phones in headless Chrome, one upgrading to
 * data channels and one kept on the WebSocket (`?rtc=0`), and measures
 * on each: echo round trips (acts: `r` against the WebSocket), then 60 Hz
 * stick input (`u` against the WebSocket's 100 ms coalescing), as the
 * host saw it (one-way latency on the shared clock, missing updates).
 *
 *   node scripts/live/party-rtc.ts [lan|local|auto|cloudflare|opentunnel] [--acts N] [--inputs N]
 *
 * Needs Chrome (CHROME=… to pick one). Phones on the same computer, so
 * the numbers are the stack's, not Wi-Fi's.
 */

import { execFileSync, spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { Browser, type Tab } from "../../games/drawbluff/test/live/browser.ts";

const args = process.argv.slice(2);
const join = args[0] && !args[0].startsWith("--") ? args[0] : "lan";
const opt = (name: string, dflt: number) => (args.includes(name) ? Number(args[args.indexOf(name) + 1]) : dflt);
const acts = opt("--acts", 200);
const inputs = opt("--inputs", 600);

execFileSync("cargo", ["build", "-q", "--release", "-p", "tvpipe-party", "--features", "webrtc", "--example", "stick"], {
  stdio: "inherit",
});
const target = JSON.parse(execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], { encoding: "utf8" }))
  .target_directory as string;
const host = spawn(`${target}/release/examples/stick`, [join], { stdio: ["ignore", "pipe", "inherit"] });
const lines: string[] = [];
const waiters: { re: RegExp; resolve: (m: RegExpMatchArray) => void }[] = [];
createInterface({ input: host.stdout! }).on("line", (line) => {
  if (!/^\S+( ⚡)? [+-]\d/.test(line)) console.log(`host| ${line}`);
  lines.push(line);
  for (const w of [...waiters]) {
    const m = line.match(w.re);
    if (m) {
      waiters.splice(waiters.indexOf(w), 1);
      w.resolve(m);
    }
  }
});

function hostSays(re: RegExp, ms = 120_000): Promise<RegExpMatchArray> {
  for (const line of lines) {
    const m = line.match(re);
    if (m) return Promise.resolve(m);
  }
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error(`host never said ${re}`)), ms);
    waiters.push({
      re,
      resolve: (m) => {
        clearTimeout(t);
        resolve(m);
      },
    });
  });
}

const pct = (xs: number[], q: number) => {
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.round((s.length - 1) * q)]!;
};
const ms = (v: number) => `${v.toFixed(2)} ms`;

interface Measured {
  transport: string;
  rtts: number[];
  host: { count: number; missing: number; p50: number; p95: number; max: number };
  sent: number;
}

const browsers: Browser[] = [];
try {
  const url = (await hostSays(/^join: (\S+)/))[1]!;
  const phones: [string, Tab][] = [];
  for (const [name, rtc] of [
    ["Rtc", true],
    ["Ws", false],
  ] as const) {
    const browser = await Browser.launch();
    browsers.push(browser);
    const u = new URL(url);
    if (!rtc) u.searchParams.set("rtc", "0");
    const tab = await browser.open(u.toString(), { width: 390, height: 844, mobile: true });
    await tab.until(`document.readyState === "complete" && !!window.party`);
    await tab.eval(`window.party.join(${JSON.stringify(name)}).then(() => true)`);
    await hostSays(new RegExp(`^\\+ ${name}$`), 10_000);
    phones.push([name, tab]);
  }
  const [rtcTab, wsTab] = [phones[0]![1], phones[1]![1]];
  const upgraded = await rtcTab
    .until(`window.party.transport === "rtc"`, 10_000)
    .then(() => true)
    .catch(() => false);
  console.log(`phone 1 transport: ${upgraded ? "rtc" : "ws (no upgrade)"}`);
  if (!upgraded && join !== "local") throw new Error("the fast path did not open");
  if ((await wsTab.eval<string>("window.party.transport")) !== "ws") throw new Error("?rtc=0 upgraded");

  const results: [string, Measured][] = [];
  for (const [name, tab] of phones) {
    const m = await tab.eval<Measured>(`window.measure(${acts}, ${inputs}, 60)`);
    results.push([name, m]);
  }
  console.log(`\njoin ${join}, ${acts} acts, ${inputs} inputs at 60 Hz, phones on this computer\n`);
  console.log("phone  transport  act RTT p50 / p95 / max            input one-way p50 / p95 / max      inputs seen / sent (missing)");
  for (const [name, m] of results) {
    const h = m.host;
    console.log(
      `${name.padEnd(6)} ${m.transport.padEnd(10)} ${ms(pct(m.rtts, 0.5))} / ${ms(pct(m.rtts, 0.95))} / ${ms(Math.max(...m.rtts))}`.padEnd(64) +
        `${ms(h.p50)} / ${ms(h.p95)} / ${ms(h.max)}`.padEnd(36) +
        `${h.count} / ${m.sent} (${h.missing})`,
    );
  }
  const rtc = results[0]![1];
  if (upgraded && rtc.host.count < m95(rtc.sent)) throw new Error(`only ${rtc.host.count} of ${rtc.sent} inputs over rtc`);
  console.log("\nok");
} finally {
  for (const b of browsers) b.close();
  host.kill("SIGTERM");
}

/** 95 % of `n`: what the unreliable channel must deliver on loopback. */
function m95(n: number): number {
  return Math.floor(n * 0.95);
}
