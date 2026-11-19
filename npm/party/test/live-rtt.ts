/**
 * Measures action round trips through a real tunnel (not part of `npm
 * test`): a party behind `tvpipe tunnel`, a client over the public wss://
 * link, 100 acks; prints median and p95.
 *
 *   TVPIPE_BINARY=… node test/live-rtt.ts [cloudflare|opentunnel]
 */

import { memoryStorage, PartyClient } from "../client/party-client.js";
import { createParty } from "../src/index.ts";

const provider = (process.argv[2] ?? "cloudflare") as "cloudflare" | "opentunnel";
const t0 = performance.now();
const party = await createParty({ tunnel: provider, qr: false, logLevel: "warn", limits: { rate: 1000, burst: 1000 } });
const setup = performance.now() - t0;
party.handle("ping", (d) => d);
const url = `wss://${party.tunnel!.host}/party/ws`;
const c = new PartyClient({ url, room: party.code, key: party.joinKey, storage: memoryStorage() });
await c.join("rtt");
const times: number[] = [];
for (let i = 0; i < 100; i++) {
  const s = performance.now();
  await c.act("ping", i);
  times.push(performance.now() - s);
}
times.sort((a, b) => a - b);
const q = (p: number) => times[Math.min(times.length - 1, Math.floor(p * times.length))]!.toFixed(1);
console.log(`${provider}: setup ${(setup / 1000).toFixed(1)} s; act round trip median ${q(0.5)} ms, p95 ${q(0.95)} ms, max ${q(1)} ms`);
c.close();
await party.shutdown();
process.exit(0);
