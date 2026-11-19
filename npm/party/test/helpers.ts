/** Test helpers: a party on a local server, fake phones and screens. */

import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { memoryStorage, PartyClient } from "../client/party-client.js";
import { PartyServer, type ServerOptions } from "../src/server.ts";
import type { Party, PartyOptions } from "../src/party.ts";

export interface Setup {
  server: PartyServer;
  party: Party;
  ws: string;
  phone(name?: string): PartyClient;
  screen(role?: "screen" | "watch"): PartyClient;
  close(): Promise<void>;
}

/** A directory with a phone and a screen page. */
export function pages(): { phone: string; screen: string } {
  const root = mkdtempSync(join(tmpdir(), "party-test-"));
  const phone = join(root, "phone");
  const screen = join(root, "screen");
  mkdirSync(join(phone, "sub"), { recursive: true });
  mkdirSync(screen);
  writeFileSync(join(phone, "index.html"), "<!doctype html><title>phone</title>");
  writeFileSync(join(phone, "app.js"), "export const x = 1;\n");
  writeFileSync(join(phone, "sub", "index.html"), "sub");
  writeFileSync(join(phone, ".secret"), "nope");
  writeFileSync(join(root, "outside.txt"), "outside");
  writeFileSync(join(screen, "index.html"), "<!doctype html><title>screen</title>");
  return { phone, screen };
}

export async function setup(opts: { server?: ServerOptions; party?: PartyOptions } = {}): Promise<Setup> {
  const server = new PartyServer({ ...pages(), ...opts.server });
  await server.listen();
  const party = server.createParty(opts.party);
  party.joinInfo = { code: party.code, url: `${server.origin}/?r=${party.code}#k=${party.joinKey}`, qr: { size: 1, rows: ["1"] } };
  const ws = `ws://127.0.0.1:${server.port}/party/ws`;
  const clients: PartyClient[] = [];
  return {
    server,
    party,
    ws,
    phone() {
      const c = new PartyClient({ url: ws, room: party.code, key: party.joinKey, storage: memoryStorage() });
      clients.push(c);
      return c;
    },
    screen(role = "screen") {
      const c = new PartyClient({
        url: ws,
        room: party.code,
        key: role === "screen" ? party.screenKey : party.watchKey,
        role,
        storage: memoryStorage(),
      });
      clients.push(c);
      return c;
    },
    async close() {
      for (const c of clients) c.close();
      await server.close();
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
