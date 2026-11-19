/**
 * Live check of the Rust party host (not part of any test suite): builds
 * and runs `tvpipe-party`'s example `buzzer` (default `local`), opens two phones
 * in a headless Chrome, joins both, arms three rounds from the VIP and
 * buzzes, and checks the terminal reports a winner each time.
 *
 *   node scripts/live/party-buzzer.ts [local|lan|auto|cloudflare|opentunnel] [--shots DIR]
 *
 * Needs Chrome (CHROME=… to pick one).
 */

import { execFileSync, spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { Browser, type Tab } from "../../games/drawbluff/test/live/browser.ts";

const join = process.argv[2] && !process.argv[2].startsWith("--") ? process.argv[2] : "local";
const shots = process.argv.includes("--shots") ? process.argv[process.argv.indexOf("--shots") + 1] : undefined;

execFileSync("cargo", ["build", "-q", "-p", "tvpipe-party", "--example", "buzzer"], { stdio: "inherit" });
const target = JSON.parse(execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], { encoding: "utf8" }))
  .target_directory as string;
const host = spawn(`${target}/debug/examples/buzzer`, [join], { stdio: ["ignore", "pipe", "inherit"] });
const lines: string[] = [];
const waiters: { re: RegExp; resolve: (m: RegExpMatchArray) => void }[] = [];
createInterface({ input: host.stdout! }).on("line", (line) => {
  if (!/^[█▀▄ ]+$/.test(line)) console.log(`host| ${line}`);
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

const phone = { width: 390, height: 844, mobile: true };
// One browser per phone: tabs of one browser share localStorage, where
// the client keeps its identity.
const browsers: Browser[] = [];
try {
  const url = (await hostSays(/^join: (\S+)/))[1]!;
  const tabs: Tab[] = [];
  for (const name of ["Ann", "Bob"]) {
    const browser = await Browser.launch();
    browsers.push(browser);
    const tab = await browser.open(url, phone);
    await tab.until(`document.readyState === "complete" && !!document.getElementById("name")`);
    await tab.eval(`document.getElementById("name").value = ${JSON.stringify(name)}`);
    await tab.eval(`document.querySelector("#join button").click()`);
    await tab.until(`!document.getElementById("game").hidden`);
    await hostSays(new RegExp(`^\\+ ${name}$`), 10_000);
    tabs.push(tab);
  }
  const [ann, bob] = tabs as [Tab, Tab];
  // LAN links are plain HTTP: the page must work without a secure context.
  const secure = await ann.eval<boolean>("window.isSecureContext");
  if (secure !== url.startsWith("https:") && !url.startsWith("http://127.0.0.1:")) throw new Error(`isSecureContext ${secure} on ${url}`);
  console.log(`secure context: ${secure}`);
  await ann.until(`!document.getElementById("start").hidden`);
  if (!(await bob.eval(`document.getElementById("start").hidden`))) throw new Error("Bob sees the VIP's start button");
  if (shots) await ann.screenshot(`${shots}/buzzer-lobby.png`);

  const press = (tab: Tab) =>
    tab.eval(`document.getElementById("buzz").dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }))`);
  for (let round = 1; round <= 3; round++) {
    await ann.eval(`document.getElementById("start").click()`);
    await bob.until(`document.getElementById("status").textContent === "Get ready…"`);
    if (round === 1) {
      // A false start: refused by the host.
      await press(bob);
      await bob.until(`document.getElementById("status").textContent === "False start!"`);
    }
    await ann.until(`document.getElementById("buzz").className === "go"`, 10_000);
    const winner = round === 2 ? bob : ann;
    await press(winner);
    const m = await hostSays(new RegExp(`^round ${round}: (\\w+) in (\\d+) ms`), 10_000);
    const expected = winner === bob ? "Bob" : "Ann";
    if (m[1] !== expected) throw new Error(`round ${round}: ${m[1]} won, expected ${expected}`);
    await bob.until(`/won in \\d+ ms/.test(document.getElementById("status").textContent)`);
  }
  const wins = await bob.eval<string>(`document.getElementById("players").textContent`);
  if (!wins.includes("Ann ★ — 2") || !wins.includes("Bob — 1")) throw new Error(`wins: ${wins}`);
  if (shots) await bob.screenshot(`${shots}/buzzer-result.png`);
  console.log("ok: two phones, three rounds, a false start, the wins");
} finally {
  for (const b of browsers) b.close();
  host.kill();
}
