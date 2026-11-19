/** Test helpers: a party on a relay in memory, fake phones and screens. */

import { memoryStorage, PartyClient } from "../src/client.ts";
import { Party, type PartyOptions } from "../src/party.ts";
import { MemoryRelay } from "../src/testing.ts";

export interface Setup {
  relay: MemoryRelay;
  party: Party;
  /** The socket URL clients use (any: the relay takes every one). */
  ws: string;
  phone(name?: string): PartyClient;
  screen(role?: "screen" | "watch"): PartyClient;
  /** A client of the party with its own options. */
  client(opts: { key?: string; room?: string; role?: "player" | "screen" | "watch"; storage?: ReturnType<typeof memoryStorage> }): PartyClient;
  close(): Promise<void>;
}

export async function setup(opts: { party?: PartyOptions } = {}): Promise<Setup> {
  const relay = new MemoryRelay();
  const party = new Party(opts.party).listen(relay);
  party.joinInfo = { code: party.code, url: `https://party.test/?r=${party.code}#k=${party.joinKey}`, qr: { size: 1, rows: ["1"] } };
  const ws = "ws://party.test/_tvpipe/ws";
  const clients: PartyClient[] = [];
  const client: Setup["client"] = (o) => {
    const c = new PartyClient({
      url: ws,
      room: o.room ?? party.code,
      key: o.key,
      role: o.role ?? "player",
      storage: o.storage ?? memoryStorage(),
      WebSocket: relay.WebSocket,
    });
    clients.push(c);
    return c;
  };
  return {
    relay,
    party,
    ws,
    client,
    phone: () => client({ key: party.joinKey }),
    screen: (role = "screen") => client({ key: role === "screen" ? party.screenKey : party.watchKey, role }),
    async close() {
      for (const c of clients) c.close();
      party.close();
      await tick(5);
    },
  };
}

/** Resolves once `cond()` holds (polling), or fails after `ms`. */
export async function until(cond: () => boolean, ms = 3000, what = "condition"): Promise<void> {
  const end = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 5));
  }
}

export const tick = (ms = 20) => new Promise((r) => setTimeout(r, ms));
