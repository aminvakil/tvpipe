/**
 * The party server: one HTTP server on 127.0.0.1 for the game's pages
 * (the phone directory at `/`, the screen directory at `/screen/`), the
 * browser client (`/party/client.js`), the game's own routes, and the
 * WebSocket (`/party/ws`) whose `hello` picks a party.
 *
 * It faces the internet through a tunnel (docs/party-design.md §6): only
 * loopback `Host`s and the tunnel's are served (421 otherwise: DNS
 * rebinding), WebSockets need a matching `Origin` (403: cross-site
 * hijacking), and every piece of data is behind a key.
 */

import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { extname, join, resolve, sep } from "node:path";
import type { Duplex } from "node:stream";
import { fileURLToPath } from "node:url";

import { WindowCounter } from "./limits.ts";
import { Party, PROTOCOL, sendBye, type Hello, type Logger, type PartyOptions } from "./party.ts";
import { isObject } from "./sync.ts";
import { accept, refuseUpgrade, type WsConnection } from "./ws.ts";

const MIME: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".htm": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".map": "application/json",
  ".webmanifest": "application/manifest+json",
  ".txt": "text/plain; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".gif": "image/gif",
  ".webp": "image/webp",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
  ".woff": "font/woff",
  ".ttf": "font/ttf",
  ".otf": "font/otf",
  ".wav": "audio/wav",
  ".mp3": "audio/mpeg",
  ".ogg": "audio/ogg",
  ".m4a": "audio/mp4",
};

/** The default Content-Security-Policy of HTML pages. */
export const DEFAULT_CSP =
  "default-src 'self'; img-src 'self' data: blob:; media-src 'self' data: blob:; " +
  "style-src 'self' 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

const CLIENT_JS = fileURLToPath(new URL("../client/party-client.js", import.meta.url));
const HELLO_TIMEOUT = 5000;
const PING_EVERY = 15_000;
const DEAD_AFTER = 45_000;

export type RouteHandler = (req: IncomingMessage, res: ServerResponse) => void | Promise<void>;

export interface ServerOptions {
  /** Port on 127.0.0.1 (default 0: any free one). */
  port?: number | undefined;
  /** The phone pages (served at `/`). */
  phone?: string | URL | undefined;
  /** The screen pages (served at `/screen/`). */
  screen?: string | URL | undefined;
  /** Content-Security-Policy for HTML (`false`: none). */
  csp?: string | false | undefined;
  /** WebSockets at once (default 64). */
  maxSockets?: number | undefined;
  /** Largest message from a client, bytes (default 256 KiB). */
  maxMessage?: number | undefined;
  /** Failed `hello`s (wrong key, unknown room, garbage) per address and minute before it is refused (default 20). */
  joinsPerMinute?: number | undefined;
  log?: Logger | undefined;
}

function dir(p: string | URL | undefined): string | undefined {
  if (p === undefined) return undefined;
  return resolve(p instanceof URL ? fileURLToPath(p) : p);
}

export class PartyServer {
  readonly #server: Server;
  readonly #phone: string | undefined;
  readonly #screen: string | undefined;
  readonly #csp: string | false;
  readonly #maxSockets: number;
  readonly #maxMessage: number;
  readonly #joins: WindowCounter;
  readonly #log: Logger;
  readonly #parties = new Map<string, Party>();
  readonly #routes = new Map<string, RouteHandler>();
  readonly #publicHosts = new Set<string>();
  readonly #sockets = new Set<WsConnection>();
  readonly #seen = new WeakMap<WsConnection, number>();
  #port: number;
  #ping: ReturnType<typeof setInterval> | undefined;

  constructor(options: ServerOptions = {}) {
    this.#phone = dir(options.phone);
    this.#screen = dir(options.screen);
    this.#csp = options.csp ?? DEFAULT_CSP;
    this.#maxSockets = options.maxSockets ?? 64;
    this.#maxMessage = options.maxMessage ?? 256 * 1024;
    this.#joins = new WindowCounter(options.joinsPerMinute ?? 20, 60_000);
    this.#log = options.log ?? (() => {});
    this.#port = options.port ?? 0;
    this.#server = createServer((req, res) => void this.#request(req, res));
    this.#server.on("upgrade", (req, socket, head) => this.#upgrade(req, socket, head));
    this.#server.on("clientError", (_err, socket) => socket.destroy());
    this.#server.headersTimeout = 10_000;
    this.#server.requestTimeout = 30_000;
  }

  /** Starts listening on 127.0.0.1. */
  async listen(): Promise<void> {
    await new Promise<void>((ok, fail) => {
      this.#server.once("error", fail);
      this.#server.listen(this.#port, "127.0.0.1", () => {
        this.#server.off("error", fail);
        ok();
      });
    });
    this.#port = (this.#server.address() as AddressInfo).port;
    this.#ping = setInterval(() => this.#keepalive(), PING_EVERY);
    this.#ping.unref();
  }

  get port(): number {
    return this.#port;
  }

  /** `http://127.0.0.1:PORT` */
  get origin(): string {
    return `http://127.0.0.1:${this.#port}`;
  }

  /** Also serve requests for this public host name (a tunnel's). */
  allowHost(host: string): void {
    this.#publicHosts.add(host.toLowerCase());
  }

  /** A new party on this server. */
  createParty(options: PartyOptions = {}): Party {
    const party = new Party({ log: this.#log, ...options });
    if (this.#parties.has(party.code)) throw new Error(`room ${party.code} exists`);
    this.#parties.set(party.code, party);
    party.onClose = () => {
      if (this.#parties.get(party.code) === party) this.#parties.delete(party.code);
    };
    return party;
  }

  party(code: string): Party | undefined {
    return this.#parties.get(code.toUpperCase());
  }

  /** Serves `handler` for `METHOD /path` (exact path). */
  route(method: string, path: string, handler: RouteHandler): void {
    this.#routes.set(`${method.toUpperCase()} ${path}`, handler);
  }

  /** Closes every party and the server. */
  async close(): Promise<void> {
    if (this.#ping) clearInterval(this.#ping);
    for (const p of [...this.#parties.values()]) p.close();
    for (const s of this.#sockets) s.terminate();
    this.#server.closeAllConnections();
    await new Promise<void>((ok) => this.#server.close(() => ok()));
  }

  // -------------------------------------------------------------------------

  /** `local`, `public`, or undefined for a Host that is neither. */
  #via(req: IncomingMessage): "local" | "public" | undefined {
    const host = (req.headers.host ?? "").toLowerCase();
    const port = this.#port;
    if (host === `127.0.0.1:${port}` || host === `localhost:${port}` || host === `[::1]:${port}`) return "local";
    const bare = host.endsWith(":443") ? host.slice(0, -4) : host;
    if (this.#publicHosts.has(bare)) return "public";
    return undefined;
  }

  #originOk(req: IncomingMessage): boolean {
    const origin = req.headers.origin;
    if (origin === undefined) return true; // not a browser: it still needs a key
    const port = this.#port;
    const o = origin.toLowerCase();
    if (o === `http://127.0.0.1:${port}` || o === `http://localhost:${port}` || o === `http://[::1]:${port}`) return true;
    return o.startsWith("https://") && this.#publicHosts.has(o.slice(8));
  }

  #address(req: IncomingMessage, via: "local" | "public"): string {
    const cf = req.headers["cf-connecting-ip"];
    if (via === "public" && typeof cf === "string" && cf.length < 64) return cf;
    return req.socket.remoteAddress ?? "?";
  }

  async #request(req: IncomingMessage, res: ServerResponse): Promise<void> {
    try {
      const via = this.#via(req);
      if (!via) return plain(res, 421, "Misdirected Request");
      const url = new URL(req.url ?? "/", "http://x");
      const route = this.#routes.get(`${req.method} ${url.pathname}`);
      if (route) return await route(req, res);
      if (req.method !== "GET" && req.method !== "HEAD") return plain(res, 405, "Method Not Allowed", { Allow: "GET, HEAD" });
      if (url.pathname === "/party/client.js") return await this.#file(req, res, CLIENT_JS);
      if (url.pathname.startsWith("/party/")) return plain(res, 404, "Not Found");
      if (url.pathname === "/screen") {
        res.writeHead(301, { Location: `/screen/${url.search}`, "Content-Length": 0 });
        return void res.end();
      }
      if (url.pathname.startsWith("/screen/")) {
        return await this.#static(req, res, this.#screen, url.pathname.slice("/screen".length));
      }
      return await this.#static(req, res, this.#phone, url.pathname);
    } catch (err) {
      this.#log("error", `party server: ${req.method} ${req.url}: ${err instanceof Error ? err.message : String(err)}`);
      if (!res.headersSent) plain(res, 500, "Internal Server Error");
      else res.destroy();
    }
  }

  async #static(req: IncomingMessage, res: ServerResponse, root: string | undefined, path: string): Promise<void> {
    if (!root) return plain(res, 404, "Not Found");
    let rel: string;
    try {
      rel = decodeURIComponent(path);
    } catch {
      return plain(res, 400, "Bad Request");
    }
    const parts = rel.split("/");
    if (rel.includes("\0") || rel.includes("\\") || parts.some((p) => p === ".." || p === "." || p.startsWith("."))) {
      return plain(res, 404, "Not Found");
    }
    let file = join(root, ...parts);
    if (file !== root && !file.startsWith(root + sep)) return plain(res, 404, "Not Found");
    if (rel.endsWith("/")) file = join(file, "index.html");
    return this.#file(req, res, file);
  }

  async #file(req: IncomingMessage, res: ServerResponse, file: string): Promise<void> {
    let st;
    try {
      st = await stat(file);
    } catch {
      return plain(res, 404, "Not Found");
    }
    if (st.isDirectory()) {
      // `/dir` → `/dir/`
      const url = new URL(req.url ?? "/", "http://x");
      res.writeHead(301, { Location: `${url.pathname}/${url.search}`, "Content-Length": 0 });
      return void res.end();
    }
    if (!st.isFile()) return plain(res, 404, "Not Found");
    const type = MIME[extname(file).toLowerCase()] ?? "application/octet-stream";
    const etag = `W/"${st.size.toString(16)}-${Math.floor(st.mtimeMs).toString(16)}"`;
    const headers: Record<string, string | number> = {
      "Content-Type": type,
      "Cache-Control": "no-cache",
      ETag: etag,
      "X-Content-Type-Options": "nosniff",
      "Referrer-Policy": "no-referrer",
    };
    if (type.startsWith("text/html") && this.#csp) headers["Content-Security-Policy"] = this.#csp;
    if (req.headers["if-none-match"] === etag) {
      res.writeHead(304, headers);
      return void res.end();
    }
    headers["Content-Length"] = st.size;
    res.writeHead(200, headers);
    if (req.method === "HEAD") return void res.end();
    await new Promise<void>((ok) => {
      const s = createReadStream(file);
      s.on("error", () => {
        res.destroy();
        ok();
      });
      s.on("end", () => ok());
      s.pipe(res);
    });
  }

  #upgrade(req: IncomingMessage, socket: Duplex, head: Buffer): void {
    socket.on("error", () => {});
    const via = this.#via(req);
    if (!via) return refuseUpgrade(socket, 421, "Misdirected Request");
    const path = (req.url ?? "").split("?")[0];
    if (path !== "/party/ws") return refuseUpgrade(socket, 404, "Not Found");
    if (!this.#originOk(req)) {
      this.#log("warn", `party server: WebSocket from origin ${JSON.stringify(req.headers.origin)} refused`);
      return refuseUpgrade(socket, 403, "Forbidden");
    }
    if (this.#sockets.size >= this.#maxSockets) return refuseUpgrade(socket, 503, "Service Unavailable");
    const address = this.#address(req, via);
    if (this.#joins.over(address)) return refuseUpgrade(socket, 429, "Too Many Requests");
    const ws = accept(req, socket, head, { maxMessage: this.#maxMessage });
    if (!ws) return;
    this.#sockets.add(ws);
    this.#seen.set(ws, performance.now());
    ws.on("close", () => this.#sockets.delete(ws));
    ws.on("pong", () => this.#seen.set(ws, performance.now()));
    ws.on("message", () => this.#seen.set(ws, performance.now()));
    const timer = setTimeout(() => fail("protocol", "no hello"), HELLO_TIMEOUT);
    timer.unref();
    const fail = (reason: "protocol" | "no-room", message?: string) => {
      this.#joins.hit(address);
      sendBye(ws, reason, message);
    };
    ws.once("message", (data, binary) => {
      clearTimeout(timer);
      let hello: unknown;
      try {
        hello = binary ? undefined : JSON.parse(data as string);
      } catch {
        hello = undefined;
      }
      if (!isObject(hello) || hello["t"] !== "hello" || hello["v"] !== PROTOCOL) {
        return fail("protocol", "expected hello, version 1");
      }
      const code = typeof hello["room"] === "string" ? hello["room"].toUpperCase() : "";
      const party = this.#parties.get(code);
      if (!party) {
        this.#log("warn", `party server: hello for room ${JSON.stringify(code.slice(0, 8))} from ${address}`);
        return fail("no-room");
      }
      party.attach(ws, hello as unknown as Hello, address, () => this.#joins.hit(address));
    });
  }

  #keepalive(): void {
    const now = performance.now();
    for (const ws of this.#sockets) {
      if (now - (this.#seen.get(ws) ?? now) > DEAD_AFTER) ws.terminate();
      else ws.ping();
    }
  }
}

function plain(res: ServerResponse, status: number, reason: string, extra: Record<string, string> = {}): void {
  const body = `${status} ${reason}\n`;
  res.writeHead(status, {
    "Content-Type": "text/plain; charset=utf-8",
    "Content-Length": Buffer.byteLength(body),
    "X-Content-Type-Options": "nosniff",
    ...extra,
  });
  res.end(body);
}
