/**
 * Test helpers: the binary from `cargo build`, a viewer for the `browser`
 * receiver's page, and a stub binary for flows that need a TV.
 */

import { chmodSync, existsSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const repo = resolve(here, "..", "..", "..");
export const fixtures = join(here, "fixtures");

/** `$TVPIPE_BINARY`, else `target/debug/tvpipe` of this repository. */
export function realBinary(): string {
  const env = process.env["TVPIPE_BINARY"];
  if (env) return env;
  const exe = process.platform === "win32" ? "tvpipe.exe" : "tvpipe";
  const p = join(repo, "target", "debug", exe);
  if (!existsSync(p)) throw new Error(`${p} is missing: run cargo build first (or set TVPIPE_BINARY)`);
  return p;
}

/** A config directory of its own per test (pairings, the player token). */
export function configDir(): string {
  return mkdtempSync(join(tmpdir(), "tvpipe-sdk-test-"));
}

/** Options every test passes: the real binary, a scratch config directory. */
export function opts(): { binary: string; configDir: string } {
  return { binary: realBinary(), configDir: configDir() };
}

export interface ViewerRecord {
  kind: number;
  ptsUs: bigint;
  payload: Uint8Array;
}

/** A tab of the player page, without a browser (docs/player-protocol.md). */
export class Viewer {
  readonly texts: Record<string, unknown>[] = [];
  readonly records: ViewerRecord[] = [];
  #ws: WebSocket;
  readonly opened: Promise<void>;

  constructor(playerUrl: string) {
    const u = new URL(playerUrl);
    const ws = new WebSocket(`ws://${u.host}/ws?t=${u.searchParams.get("t")}`);
    ws.binaryType = "arraybuffer";
    this.#ws = ws;
    this.opened = new Promise((resolve, reject) => {
      ws.addEventListener("open", () => resolve(), { once: true });
      ws.addEventListener("error", () => reject(new Error("viewer socket failed")), { once: true });
    });
    ws.addEventListener("message", (ev: MessageEvent) => {
      if (typeof ev.data === "string") {
        this.texts.push(JSON.parse(ev.data) as Record<string, unknown>);
      } else {
        const b = new Uint8Array(ev.data as ArrayBuffer);
        const v = new DataView(b.buffer, b.byteOffset, b.byteLength);
        this.records.push({ kind: b[0]!, ptsUs: v.getBigUint64(1, true), payload: b.subarray(13) });
      }
    });
  }

  config(): Record<string, unknown> | undefined {
    return this.texts.find((t) => t["type"] === "config");
  }

  count(kind: number): number {
    return this.records.filter((r) => r.kind === kind).length;
  }

  close(): void {
    this.#ws.close();
  }
}

/** Polls `f` until it holds. */
export async function until(what: string, f: () => boolean, timeoutMs = 20000): Promise<void> {
  const end = Date.now() + timeoutMs;
  while (!f()) {
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(20);
  }
}

export function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

/**
 * A stub binary (a Node script) that speaks the machine interface for
 * flows a test cannot run against a TV: pairing, receiver discovery.
 * `script` is the body of `main(args, state, emit, commands)`. Unix only
 * (Windows cannot spawn a script without a shell).
 */
export function stubBinary(script: string): { binary: string; dir: string } {
  const dir = mkdtempSync(join(tmpdir(), "tvpipe-stub-"));
  const binary = join(dir, "tvpipe");
  writeFileSync(
    binary,
    `#!${process.execPath}
const fs = require("node:fs");
const path = require("node:path");
const args = process.argv.slice(2).filter((a) => a !== "--machine" && a !== "-v");
const stateFile = path.join(${JSON.stringify(dir)}, "state.json");
const state = fs.existsSync(stateFile) ? JSON.parse(fs.readFileSync(stateFile, "utf8")) : {};
const save = () => fs.writeFileSync(stateFile, JSON.stringify(state));
const emit = (ev) => process.stdout.write(JSON.stringify(ev) + "\\n");
// Commands from stdin: SLRW magic, then records; kind 6 is JSON.
const commands = [];
const waiters = [];
let buf = Buffer.alloc(0);
let closed = false;
process.stdin.on("data", (d) => {
  buf = Buffer.concat([buf, d]);
  if (buf.length >= 4 && buf.subarray(0, 4).toString() === "SLRW") buf = buf.subarray(4);
  while (buf.length >= 13) {
    const len = buf.readUInt32LE(9);
    if (buf.length < 13 + len) break;
    if (buf[0] === 6) commands.push(JSON.parse(buf.subarray(13, 13 + len).toString()));
    buf = buf.subarray(13 + len);
  }
  while (commands.length && waiters.length) waiters.shift()(commands.shift());
});
process.stdin.on("end", () => { closed = true; while (waiters.length) waiters.shift()(undefined); });
const nextCommand = () => new Promise((r) => {
  if (commands.length) r(commands.shift()); else if (closed) r(undefined); else waiters.push(r);
});
const finish = (code) => { process.stdout.write("", () => process.exit(code)); };
emit({ type: "hello", protocol: 1, version: "0.0.0-stub", command: args[0] });
(async () => {
${script}
})().catch((e) => { process.stderr.write(String(e && e.stack || e) + "\\n"); process.exit(3); });
`,
  );
  chmodSync(binary, 0o755);
  return { binary, dir };
}
