/**
 * Test helpers: a relay in memory (`memoryRelay()`), so a host and fake
 * phones run in one process without tvpipe: `relay.WebSocket` is a
 * WebSocket class for `PartyClient` whose sockets end in the relay.
 * Delivery is asynchronous and in order, like a socket's.
 */

import type { WebSocketFactory, WebSocketLike } from "./client.ts";
import type { Relay, RelayConn, Via } from "./relay.ts";

const later = (f: () => void) => setTimeout(f, 0);

class HostSide implements RelayConn {
  readonly id: number;
  readonly via: Via;
  readonly address: string;
  readonly query: string;
  readonly agent: string;
  busy = false;
  closed = false;
  onmessage: ((data: string | Uint8Array) => void) | undefined;
  onclose: ((code: number, reason: string) => void) | undefined;
  ondrain: (() => void) | undefined;
  readonly #client: FakeSocket;
  readonly #done: () => void;
  #ended = false;

  constructor(id: number, client: FakeSocket, via: Via, address: string, query: string, done: () => void) {
    this.id = id;
    this.#done = done;
    this.#client = client;
    this.via = via;
    this.address = address;
    this.query = query;
    this.agent = "memory";
  }

  send(text: string): void {
    if (this.closed) return;
    later(() => this.#client.deliver(text));
  }

  sendBinary(data: Uint8Array): void {
    if (this.closed) return;
    const copy = data.slice();
    later(() => this.#client.deliver(copy));
  }

  close(code: number, reason = ""): void {
    if (this.closed) return;
    this.closed = true;
    later(() => {
      this.#client.closedBy(code, reason);
      this.ended(code, reason);
    });
  }

  /** The connection is over (either side): the host hears of it once. */
  ended(code: number, reason: string): void {
    if (this.#ended) return;
    this.#ended = true;
    this.closed = true;
    this.#done();
    this.onclose?.(code, reason);
  }
}

class FakeSocket implements WebSocketLike {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSED = 3;
  readyState = 0;
  onopen: ((ev: any) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: { code: number; reason: string }) => void) | null = null;
  onerror: ((ev: any) => void) | null = null;
  readonly url: string;
  #host: HostSide | undefined;

  constructor(relay: MemoryRelay, url: string, via: Via, address: string) {
    this.url = url;
    later(() => {
      if (this.readyState !== 0) return;
      const conn = relay.open(this, via, address, url.includes("?") ? url.slice(url.indexOf("?") + 1) : "");
      if (!conn) {
        // tvpipe answers 503 without a host.
        this.readyState = 3;
        this.onerror?.({});
        this.onclose?.({ code: 1006, reason: "" });
        return;
      }
      this.#host = conn;
      this.readyState = 1;
      this.onopen?.({});
    });
  }

  send(data: string | Uint8Array): void {
    if (this.readyState !== 1) return;
    const host = this.#host;
    const copy = typeof data === "string" ? data : data.slice();
    later(() => {
      if (host && !host.closed) host.onmessage?.(copy);
    });
  }

  close(code = 1000, reason = ""): void {
    if (this.readyState === 3) return;
    const wasOpen = this.readyState === 1;
    this.readyState = 3;
    const host = this.#host;
    later(() => {
      if (wasOpen) host?.ended(code, reason);
      this.onclose?.({ code, reason });
    });
  }

  /** From the host. */
  deliver(data: string | Uint8Array): void {
    if (this.readyState !== 1) return;
    this.onmessage?.({ data });
  }

  /** The host closed it. */
  closedBy(code: number, reason: string): void {
    if (this.readyState === 3) return;
    this.readyState = 3;
    this.onclose?.({ code, reason });
  }
}

/** A relay in memory; see the module docs. */
export class MemoryRelay implements Relay {
  onopen: ((conn: RelayConn) => void) | undefined;
  onclosing: (() => void) | undefined;
  readonly publicUrl: string;
  /** A WebSocket class for `PartyClient` (`via: "public"`, address `127.0.0.1`). */
  readonly WebSocket: WebSocketFactory;
  #next = 0;
  readonly #conns = new Set<HostSide>();

  constructor(publicUrl = "https://party.test/") {
    this.publicUrl = publicUrl;
    const relay = this;
    this.WebSocket = class extends FakeSocket {
      constructor(url: string) {
        super(relay, url, "public", "127.0.0.1");
      }
    };
  }

  /** A WebSocket class whose sockets come from `via` and `address`. */
  socketFrom(via: Via, address: string): WebSocketFactory {
    const relay = this;
    return class extends FakeSocket {
      constructor(url: string) {
        super(relay, url, via, address);
      }
    };
  }

  /** @internal */
  open(client: FakeSocket, via: Via, address: string, query: string): HostSide | undefined {
    if (!this.onopen) return undefined;
    const conn: HostSide = new HostSide(++this.#next, client, via, address, query, () => this.#conns.delete(conn));
    this.#conns.add(conn);
    this.onopen(conn);
    return conn;
  }

  /** Connections open now. */
  get size(): number {
    return this.#conns.size;
  }

  /** The host's side of every open connection, oldest first. */
  connections(): Iterable<RelayConn> {
    return this.#conns;
  }

  /** What tvpipe reports when a client's backlog crosses 64 KiB. */
  setBusy(conn: RelayConn, busy: boolean): void {
    const c = conn as HostSide;
    if (!this.#conns.has(c) || c.busy === busy) return;
    c.busy = busy;
    if (!busy) later(() => c.ondrain?.());
  }

  /** What tvpipe does at the end: `closing`, then every client closed with 4011. */
  async end(graceMs = 50): Promise<void> {
    this.onclosing?.();
    await new Promise((r) => setTimeout(r, graceMs));
    for (const c of [...this.#conns]) c.close(4011, "closed");
  }
}

export function memoryRelay(publicUrl?: string): MemoryRelay {
  return new MemoryRelay(publicUrl);
}
