/**
 * The local API of `tvpipe playshare` (docs/playshare-design.md §5): the
 * launcher and the host page talk to tvpipe with it. One WebSocket
 * (`/_tvpipe/api`, the session cookie or `?t=TOKEN`): calls, events, and
 * the relay of the phones' connections once the host attaches.
 *
 *   const tvpipe = await Tvpipe.connect();
 *   const party = await tvpipe.party({ lateJoin: "wait" });   // the host
 *   await party.cast();                                      // the TV page on the chosen TVs
 */

import { Emitter } from "./emitter.ts";
import { Party, type PartyOptions } from "./party.ts";
import { TvpipeRelay } from "./relay.ts";

export interface Pages {
  host: string;
  tv: string;
  phone: string;
  launcher?: string;
}

export interface TunnelInfo {
  /** `off`, `starting`, `up`, `failed`. */
  state: string;
  provider?: string;
  /** `https://HOST/` once up. */
  url?: string;
  error?: string;
}

/** `info`: the game and how phones reach it. */
export interface Info {
  name: string;
  title: string;
  players: { min: number; max: number };
  /** Absolute paths on the local origin. */
  pages: Pages;
  tv: { width: number; height: number } | null;
  phone: { url: string; mode: "tunnel" | "lan" | "local"; lan?: string };
  tunnel: TunnelInfo;
  /** `http://127.0.0.1:PORT`. */
  local: string;
  version: string;
}

/** A receiver the launcher can pick. */
export interface Device {
  name: string;
  /** `browser`: a tab on this computer (no TV needed). */
  kind: "airplay" | "cast" | "both" | "browser";
  model?: string;
  title?: string;
  /** AirPlay: shows a PIN and has no pairing here yet. */
  needsPairing?: boolean;
  paired?: boolean;
}

export interface PairStatus {
  device: string;
  state: "connecting" | "pin" | "done" | "failed";
  error?: string;
}

export interface ReceiverStatus {
  name: string;
  protocol?: string;
  /** `connected`, `failed`, `lost`, `reconnected`, `gave-up`, `ended`, `stopped`. */
  state: string;
  error?: string;
  code?: string;
}

export interface CastStatus {
  state: "starting" | "streaming" | "failed" | "stopped";
  receivers?: ReceiverStatus[];
  /** The page of the `browser` receiver. */
  player?: string;
  error?: string;
  to?: string[];
}

export interface TvpipeEvents {
  devices: [devices: Device[]];
  pair: [status: PairStatus];
  cast: [status: CastStatus];
  tunnel: [tunnel: TunnelInfo];
  /** tvpipe is ending: a second to say goodbye. */
  closing: [];
  /** Another page took the relay over. */
  detached: [];
  /** The API socket closed. */
  close: [];
}

/** A refused call. */
export class TvpipeError extends Error {
  readonly code: string;
  constructor(code: string, message: string) {
    super(message);
    this.name = "TvpipeError";
    this.code = code;
  }
}

export interface ConnectOptions {
  /** Default: `/_tvpipe/api` on the page's origin. */
  url?: string;
  /** The API token (pages have the session cookie instead). */
  token?: string;
  /** Default the global one. */
  WebSocket?: typeof WebSocket;
}

type Waiter = { resolve: (v: any) => void; reject: (e: Error) => void };

export class Tvpipe extends Emitter<TvpipeEvents> {
  /** Connects to tvpipe's API. */
  static connect(opts: ConnectOptions = {}): Promise<Tvpipe> {
    let url = opts.url;
    if (!url) {
      const loc = globalThis.location;
      url = `${loc.protocol === "https:" ? "wss:" : "ws:"}//${loc.host}/_tvpipe/api`;
    }
    if (opts.token) url += `${url.includes("?") ? "&" : "?"}t=${encodeURIComponent(opts.token)}`;
    const WS = opts.WebSocket ?? globalThis.WebSocket;
    const ws = new WS(url);
    ws.binaryType = "arraybuffer";
    return new Promise((resolve, reject) => {
      ws.onopen = () => resolve(new Tvpipe(ws));
      ws.onerror = () => reject(new TvpipeError("connect", `cannot reach tvpipe at ${url!.replace(/t=[^&]*/, "t=…")}`));
    });
  }

  readonly #ws: WebSocket;
  #n = 0;
  readonly #calls = new Map<number, Waiter>();
  #relay: TvpipeRelay | undefined;
  #closed = false;

  private constructor(ws: WebSocket) {
    super();
    this.#ws = ws;
    ws.onerror = null;
    ws.onmessage = (ev) => {
      if (typeof ev.data === "string") this.#onText(ev.data);
      else if (ev.data instanceof ArrayBuffer) this.#relay?.handleBinary(new Uint8Array(ev.data));
    };
    ws.onclose = () => {
      this.#closed = true;
      const err = new TvpipeError("closed", "the connection to tvpipe closed");
      for (const w of this.#calls.values()) w.reject(err);
      this.#calls.clear();
      this.#relay?.handleDetach();
      this.emit("close");
    };
  }

  get closed(): boolean {
    return this.#closed;
  }

  #onText(text: string): void {
    let msg: Record<string, any>;
    try {
      msg = JSON.parse(text);
    } catch {
      return;
    }
    if (typeof msg !== "object" || msg === null) return;
    if (msg["t"] === "reply") {
      const w = this.#calls.get(msg["n"]);
      if (!w) return;
      this.#calls.delete(msg["n"]);
      if (msg["ok"]) w.resolve(msg["value"]);
      else w.reject(new TvpipeError(msg["error"]?.code ?? "failed", msg["error"]?.message ?? "failed"));
      return;
    }
    if (msg["t"] === "event") {
      const data = msg["data"];
      switch (msg["type"]) {
        case "devices":
          this.emit("devices", Array.isArray(data) ? data : []);
          return;
        case "pair":
          this.emit("pair", data);
          return;
        case "cast":
          this.emit("cast", data);
          return;
        case "tunnel":
          this.emit("tunnel", data);
          return;
        case "closing":
          this.#relay?.onclosing?.();
          this.emit("closing");
          return;
        case "detached":
          this.emit("detached");
          return;
      }
      return;
    }
    this.#relay?.handleControl(msg);
  }

  /** Calls an API method (docs/playshare-design.md §5). */
  call<T = any>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    if (this.#closed) return Promise.reject(new TvpipeError("closed", "the connection to tvpipe closed"));
    const n = ++this.#n;
    return new Promise<T>((resolve, reject) => {
      this.#calls.set(n, { resolve, reject });
      this.#ws.send(JSON.stringify({ t: "call", n, method, params }));
    });
  }

  info(): Promise<Info> {
    return this.call("info");
  }

  /** Starts (or stops) discovery; `devices` events follow. Resolves with the list so far. */
  watchDevices(on = true): Promise<Device[]> {
    return this.call("devices.watch", { on });
  }

  /** The launcher's choice, kept by tvpipe across the navigation to the host page. */
  async select(devices: string[]): Promise<void> {
    await this.call("select", { devices });
  }

  async selection(): Promise<string[]> {
    return (await this.call<{ devices: string[] }>("selection")).devices;
  }

  /** AirPlay PIN pairing: `pair` events say when the PIN is wanted. */
  async pair(device: string): Promise<void> {
    await this.call("pair.start", { device });
  }

  async pairPin(device: string, pin: string): Promise<void> {
    await this.call("pair.pin", { device, pin });
  }

  async pairCancel(device: string): Promise<void> {
    await this.call("pair.cancel", { device });
  }

  /** Casts a page of this game (a path with its fragment) to `to` (default: the selection). */
  cast(opts: { page: string; to?: string[] | undefined }): Promise<CastStatus> {
    const params: Record<string, unknown> = { page: opts.page };
    if (opts.to) params["to"] = opts.to;
    return this.call("cast.start", params);
  }

  async stopCast(): Promise<void> {
    await this.call("cast.stop");
  }

  /** A QR code made by tvpipe: `size` modules, `rows` of "0"/"1". */
  qr(text: string): Promise<{ size: number; rows: string[] }> {
    return this.call("qr", { text });
  }

  /** Ends playshare (the TVs stop, phones are told). */
  async quit(): Promise<void> {
    await this.call("quit").catch(() => {});
  }

  /** Becomes the relay's host: phones' connections come here. */
  async relay(): Promise<TvpipeRelay> {
    const r = await this.call<{ public: string; local: string }>("relay.attach");
    const ws = this.#ws;
    this.#relay = new TvpipeRelay(
      {
        sendText: (t) => ws.send(t),
        sendBinary: (d) => ws.send(d),
      },
      r.public,
    );
    return this.#relay;
  }

  /** The host: a party on the relay, its links and QR code, casting. */
  async party(options: PartyOptions = {}): Promise<HostedParty> {
    const info = await this.info();
    const relay = await this.relay();
    const party = new HostedParty(this, info, relay, options);
    await party.refreshJoin();
    return party;
  }

  /** Closes the API socket (tvpipe ends after a while without a page). */
  close(): void {
    this.#ws.close();
  }
}

/** A party's links. */
export interface PartyLinks {
  /** What phones open (the QR code): public with a tunnel. */
  join: string;
  /** The TV page with the screen key: a path on the local origin, for `cast`. */
  tv: string;
  /** A read-only screen on this computer (an `<iframe>` preview in the host page). */
  watch: string;
}

/** A {@link Party} in a host page, with tvpipe behind it. */
export class HostedParty extends Party {
  readonly tvpipe: Tvpipe;
  readonly info: Info;
  readonly relay: TvpipeRelay;
  links: PartyLinks;

  constructor(tvpipe: Tvpipe, info: Info, relay: TvpipeRelay, options: PartyOptions) {
    super({ maxPlayers: info.players.max, ...options });
    this.tvpipe = tvpipe;
    this.info = info;
    this.relay = relay;
    this.listen(relay);
    const tv = `${info.pages.tv}#r=${this.code}&k=${this.screenKey}&role=screen`;
    this.links = {
      join: this.#joinUrl(info.phone.url),
      tv,
      watch: `${info.local}${info.pages.tv}#r=${this.code}&k=${this.watchKey}&role=watch`,
    };
    tvpipe.on("tunnel", (t) => {
      if (t.state === "up" && t.url) {
        this.info.phone = { ...this.info.phone, url: t.url, mode: "tunnel" };
        void this.refreshJoin();
      }
    });
  }

  #joinUrl(base: string): string {
    return `${base.endsWith("/") ? base : `${base}/`}?r=${this.code}#k=${this.joinKey}`;
  }

  /** Makes the join link and its QR code (again: after the tunnel came up). */
  async refreshJoin(): Promise<void> {
    this.links = { ...this.links, join: this.#joinUrl(this.info.phone.url) };
    let qr: { size: number; rows: string[] } | null = null;
    try {
      qr = await this.tvpipe.qr(this.links.join);
    } catch {}
    this.joinInfo = { code: this.code, url: this.links.join, qr, mode: this.info.phone.mode };
  }

  /** Casts the TV page to `to` (default: what the launcher chose). */
  cast(to?: string[]): Promise<CastStatus> {
    return this.tvpipe.cast({ page: this.links.tv, to });
  }

  stopCast(): Promise<void> {
    return this.tvpipe.stopCast();
  }
}
