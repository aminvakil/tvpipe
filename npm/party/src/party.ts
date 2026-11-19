/**
 * One party (a room): its players and screens, their views, actions and
 * messages (docs/party-design.md §5). A {@link PartyServer} hosts parties
 * and hands each socket to the party its `hello` names.
 */

import { randomBytes, timingSafeEqual } from "node:crypto";
import { EventEmitter } from "node:events";

import { TokenBucket } from "./limits.ts";
import { cleanText, MAX_NAME, nameKey } from "./names.ts";
import { diff, isObject, StateBox, toJson, type Json, type JsonObject } from "./sync.ts";
import type { WsConnection } from "./ws.ts";

export const PROTOCOL = 1;

/** Who a socket is. */
export type Role = "player" | "screen" | "watch";

export type LogLevel = "debug" | "info" | "warn" | "error";
export type Logger = (level: LogLevel, message: string) => void;

/** Why a client was sent away (`bye`), and the close code that goes with it. */
export const BYE = {
  protocol: 4002,
  "bad-key": 4003,
  "no-room": 4004,
  "no-player": 4005,
  "name-taken": 4006,
  "name-invalid": 4007,
  "rate-limited": 4008,
  replaced: 4009,
  kicked: 4010,
  "room-closed": 4011,
  slow: 4013,
  full: 4029,
  locked: 4029,
} as const;
export type ByeReason = keyof typeof BYE;

/** An error for the phone: `act()` there rejects with this code and message. */
export class PartyError extends Error {
  readonly code: string;
  constructor(code: string, message: string = code) {
    super(message);
    this.name = "PartyError";
    this.code = code;
  }
}

/** A random token: 128 bits, base64url. */
export function token(): string {
  return randomBytes(16).toString("base64url");
}

/** Constant-time string comparison (for keys and tokens). */
export function sameToken(a: unknown, b: string): boolean {
  if (typeof a !== "string") return false;
  const x = Buffer.from(a);
  const y = Buffer.from(b);
  return x.length === y.length && timingSafeEqual(x, y);
}

/** Room codes: four consonants (no words, no 0/O confusion). */
const CODE_LETTERS = "BCDFGHJKLMNPQRSTVWXZ";
export function roomCode(): string {
  const b = randomBytes(4);
  return [...b].map((x) => CODE_LETTERS[x % CODE_LETTERS.length]).join("");
}

/** Bytes queued to one socket before its views wait (latest wins). */
const STATE_HIGH_WATER = 64 * 1024;
/** Bytes queued to one socket before it is closed as too slow. */
const SLOW_LIMIT = 256 * 1024;
/** Results of earlier actions kept per player, for replays. */
const ACK_CACHE = 32;
/** Largest message other than `act`. */
const MAX_CONTROL_MESSAGE = 16 * 1024;
/** Largest `in` message (continuous input). */
const MAX_INPUT = 4 * 1024;

type Ack = { t: "ack"; id: number; ok: true; result?: Json; dup?: true } | { t: "ack"; id: number; ok: false; error: { code: string; message: string } };

/** A player: a person with a phone, across reconnects. */
export class Player {
  readonly role = "player" as const;
  readonly id: string;
  name: string;
  vip = false;
  /** What only this player sees (`view.me` on the phone). */
  readonly state: StateBox;
  /** The game's own data about the player (not sent anywhere). */
  data: Record<string, unknown> = {};
  readonly joinedAt = Date.now();
  /** @internal */ resume: string;
  /** @internal */ lastAct = 0;
  /** @internal */ results = new Map<number, Ack | Promise<Ack>>();
  /** @internal */ client: Client | undefined;
  /** The latest continuous input (`in`) from the phone, if any. */
  input: Json | undefined;

  constructor(id: string, name: string, changed: () => void) {
    this.id = id;
    this.name = name;
    this.resume = token();
    this.state = new StateBox(changed);
  }

  get connected(): boolean {
    return this.client !== undefined;
  }

  toJSON(): JsonObject {
    return { id: this.id, name: this.name, vip: this.vip, connected: this.connected };
  }
}

/** A screen (the TV page, a watcher) as the sender of an action. */
export interface Screen {
  readonly role: "screen" | "watch";
  readonly id: string;
}

export type Sender = Player | Screen;

export type ActionHandler = (data: Json, from: Sender) => unknown;

export interface Hello {
  t: "hello";
  v?: unknown;
  room?: unknown;
  role?: unknown;
  key?: unknown;
  name?: unknown;
  resume?: unknown;
  agent?: unknown;
}

/** One socket after its `hello`. @internal */
export class Client {
  readonly ws: WsConnection;
  readonly role: Role;
  readonly id: string;
  readonly address: string;
  player: Player | undefined;
  /** The view this client has (after the last `state`). */
  sent: JsonObject | undefined;
  rev = 0;
  waiting = false;
  readonly bucket: TokenBucket;
  /** Screens: action ids seen on this connection. */
  lastAct = 0;
  /** The last `in` sequence number on this connection. */
  inSeq = 0;

  constructor(ws: WsConnection, role: Role, id: string, address: string, rate: number, burst: number) {
    this.ws = ws;
    this.role = role;
    this.id = id;
    this.address = address;
    this.bucket = new TokenBucket(rate, burst);
  }

  send(msg: JsonObject): boolean {
    if (this.ws.closed) return false;
    if (this.ws.bufferedAmount > SLOW_LIMIT) {
      this.bye("slow");
      return false;
    }
    this.ws.send(JSON.stringify(msg));
    return true;
  }

  bye(reason: ByeReason, message?: string): void {
    if (this.ws.closed) return;
    const msg: JsonObject = { t: "bye", reason };
    if (message) msg["message"] = message;
    this.ws.send(JSON.stringify(msg));
    this.ws.close(BYE[reason], reason);
  }
}

export interface JoinInfo {
  code: string;
  url: string;
  /** The QR code of `url`: `size` modules per side, `rows` of "0"/"1". */
  qr: { size: number; rows: string[] } | null;
  /** How phones reach the party: a tunnel, the LAN (Rust hosts), this computer. */
  mode?: "tunnel" | "lan" | "local";
}

export interface PartyLimits {
  /** Messages per second per socket (default 20). */
  rate?: number;
  /** Messages a socket may send at once (default 50). */
  burst?: number;
  /** Screens and watchers at once (default 16). */
  screens?: number;
}

export interface PartyOptions {
  /** The room code (default: four random consonants). */
  code?: string | undefined;
  /** Players at most (default 8). */
  maxPlayers?: number | undefined;
  /**
   * Checks names (and whatever the game passes to {@link Party.moderate}):
   * return a replacement, `false` to refuse, or nothing to accept.
   */
  moderate?: ((text: string, ctx: { kind: string; player?: Player | undefined }) => string | false | undefined | void) | undefined;
  limits?: PartyLimits | undefined;
  log?: Logger | undefined;
  /** Late submissions accepted after a `collect` deadline, ms (default 500). */
  grace?: number | undefined;
}

export interface PartyEvents {
  /** A new player joined. */
  join: [player: Player];
  /** A player's phone is back. */
  reconnect: [player: Player];
  /** A player's phone went away (the player stays). */
  away: [player: Player];
  /** A player left for good (`leave`, kicked, removed). */
  leave: [player: Player, reason: string];
  /** The VIP changed. */
  vip: [player: Player | undefined];
  /** A screen connected or left. */
  screen: [screen: Screen, connected: boolean];
  /** The screen view changed (after the tick's changes). */
  "screen-view": [view: JsonObject];
  /**
   * A player's continuous input (`in`: a stick, a tilt), newer than the
   * last one (also kept as `player.input`). Never answered; Node hosts
   * get it over the WebSocket (Rust hosts also over WebRTC).
   */
  input: [player: Player, data: Json];
}

export interface CollectOptions<T> {
  /** Who must answer (default: every player when `collect` starts). */
  from?: Player[] | undefined;
  /** Deadline, in `party.now()` time; without it, until everyone answered. */
  until?: number | undefined;
  /** Accepted after the deadline, ms (default: the party's, 500). */
  grace?: number | undefined;
  /** Checks and converts an answer; throw a {@link PartyError} to refuse it. */
  validate?: ((data: Json, player: Player) => T) | undefined;
  /** Whether a player may answer again, replacing the first answer (default false). */
  replace?: boolean | undefined;
  /** The phone's result for an accepted answer (default `null`). */
  reply?: ((value: T, player: Player) => unknown) | undefined;
  /** Called after each accepted answer. */
  onAnswer?: ((player: Player, value: T, answers: ReadonlyMap<Player, T>) => void) | undefined;
  /** With `until`: stop waiting for players who are away (default: they may come back until the deadline). */
  skipAway?: boolean | undefined;
  /** Ends early (resolving with the answers so far). */
  signal?: AbortSignal | undefined;
}

let partySeq = 0;

export class Party extends EventEmitter<PartyEvents> {
  readonly code: string;
  readonly joinKey = token();
  readonly screenKey = token();
  readonly watchKey = token();
  /** What every player sees (`view.room`). */
  readonly room: StateBox;
  /** What screens see (`view.screen`). */
  readonly screen: StateBox;
  readonly maxPlayers: number;
  readonly grace: number;
  /** @internal */ readonly uid = ++partySeq;
  #join: JoinInfo;
  #players: Player[] = [];
  #clients = new Set<Client>();
  #handlers = new Map<string, ActionHandler>();
  #locked = false;
  #closed = false;
  #seq = 0;
  #screenSeq = 0;
  #flushQueued = false;
  #screenView: JsonObject | undefined;
  readonly #moderate: PartyOptions["moderate"];
  readonly #limits: Required<PartyLimits>;
  readonly #log: Logger;
  /** @internal */ onClose: (() => void) | undefined;

  constructor(options: PartyOptions = {}) {
    super();
    this.code = options.code ?? roomCode();
    this.maxPlayers = options.maxPlayers ?? 8;
    this.grace = options.grace ?? 500;
    this.#moderate = options.moderate;
    this.#limits = { rate: 20, burst: 50, screens: 16, ...options.limits };
    this.#log = options.log ?? (() => {});
    this.room = new StateBox(() => this.#changed());
    this.screen = new StateBox(() => this.#changed());
    this.#join = { code: this.code, url: "", qr: null };
  }

  /** The server's clock (ms since the epoch): deadlines in views use it. */
  now(): number {
    return Date.now();
  }

  /** Players in join order (away ones included). */
  get players(): readonly Player[] {
    return this.#players;
  }

  /** Players whose phone is connected. */
  get connectedPlayers(): Player[] {
    return this.#players.filter((p) => p.connected);
  }

  get vip(): Player | undefined {
    return this.#players.find((p) => p.vip);
  }

  get locked(): boolean {
    return this.#locked;
  }

  get closed(): boolean {
    return this.#closed;
  }

  /** The join link and its QR code, shown by screens (`view.join`). */
  get joinInfo(): JoinInfo {
    return this.#join;
  }

  set joinInfo(info: JoinInfo) {
    this.#join = info;
    this.#changed();
  }

  player(id: string): Player | undefined {
    return this.#players.find((p) => p.id === id);
  }

  /** What screens see: `{ roster, screen, join }` (also what a video renderer draws). */
  screenView(): JsonObject {
    this.#screenView ??= {
      roster: this.#roster(),
      screen: this.screen.peek(),
      join: toJson(this.#join),
    };
    return this.#screenView;
  }

  /** What a player sees: `{ roster, room, me }`. */
  playerView(p: Player): JsonObject {
    return { roster: this.#roster(), room: this.room.peek(), me: p.state.peek() };
  }

  #roster(): Json {
    return this.#players.map((p) => p.toJSON());
  }

  /** No new players from now on (resumes still work). */
  lock(): void {
    this.#locked = true;
  }

  unlock(): void {
    this.#locked = false;
  }

  /** Handles actions of `type`; the return value is the phone's result. */
  handle(type: string, handler: ActionHandler): () => void {
    this.#handlers.set(type, handler);
    return () => {
      if (this.#handlers.get(type) === handler) this.#handlers.delete(type);
    };
  }

  /** Runs the `moderate` hook on a text the game shows (`false`: refused). */
  moderate(text: string, kind: string, player?: Player): string | false {
    const r = this.#moderate?.(text, { kind, player });
    if (r === false) return false;
    return typeof r === "string" ? r : text;
  }

  /** A transient message to one player (lost when the phone is away). */
  send(to: Player, type: string, data: unknown = null): void {
    to.client?.send({ t: "msg", type, data: toJson(data) });
  }

  /** A transient message to every connected client of a role (default: players). */
  broadcast(type: string, data: unknown = null, role: Role | "all" = "player"): void {
    const msg = { t: "msg", type, data: toJson(data) };
    for (const c of this.#clients) if (role === "all" || c.role === role) c.send(msg);
  }

  /** Removes a player for good, telling their phone. */
  kick(player: Player, reason: string = "kicked"): void {
    player.client?.bye("kicked", reason);
    this.#remove(player, "kicked");
  }

  /** Ends the party: every client gets `bye room-closed`. */
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    for (const c of this.#clients) c.bye("room-closed");
    this.#clients.clear();
    this.onClose?.();
  }

  /**
   * Waits for one action of `type` from each player (or the deadline),
   * resolving with the answers. Answers from others, or a second one,
   * are refused (`not-asked`, `already-answered`).
   */
  collect<T = Json>(type: string, options: CollectOptions<T> = {}): Promise<Map<Player, T>> {
    const from = new Set(options.from ?? this.#players);
    const answers = new Map<Player, T>();
    const grace = options.grace ?? this.grace;
    return new Promise((resolve) => {
      let done = false;
      let timer: ReturnType<typeof setTimeout> | undefined;
      const finish = () => {
        if (done) return;
        done = true;
        if (timer) clearTimeout(timer);
        off();
        this.off("away", check);
        this.off("leave", onLeave);
        options.signal?.removeEventListener("abort", finish);
        resolve(answers);
      };
      // With a deadline, players who are away may still come back and
      // answer; without one (or with `skipAway`), only connected players
      // are waited for, and at least one must be there.
      const check = () => {
        if (from.size === 0) return finish();
        const all = [...from];
        if (options.until !== undefined && !options.skipAway) {
          if (all.every((p) => answers.has(p))) finish();
          return;
        }
        if (!all.some((p) => p.connected)) return;
        if (all.every((p) => answers.has(p) || !p.connected)) finish();
      };
      const onLeave = (p: Player) => {
        from.delete(p);
        check();
      };
      const off = this.handle(type, (data, sender) => {
        if (sender.role !== "player" || !from.has(sender)) throw new PartyError("not-asked", "Not your turn");
        if (answers.has(sender) && !options.replace) throw new PartyError("already-answered", "Already sent");
        const value = options.validate ? options.validate(data, sender) : (data as T);
        answers.set(sender, value);
        options.onAnswer?.(sender, value, answers);
        queueMicrotask(check);
        return options.reply ? options.reply(value, sender) : null;
      });
      this.on("away", check);
      this.on("leave", onLeave);
      options.signal?.addEventListener("abort", finish, { once: true });
      if (options.signal?.aborted) return finish();
      if (options.until !== undefined) {
        timer = setTimeout(finish, Math.max(0, options.until + grace - this.now()));
      }
      check();
    });
  }

  // -------------------------------------------------------------------------
  // Sockets (called by the server)

  /** A socket's `hello` for this party. @internal */
  attach(ws: WsConnection, hello: Hello, address: string, failed: () => void = () => {}): void {
    if (this.#closed) {
      bye(ws, "room-closed");
      return;
    }
    const role = hello.role;
    if (role === "screen" || role === "watch") {
      const key = role === "screen" ? this.screenKey : this.watchKey;
      if (!sameToken(hello.key, key)) {
        this.#log("warn", `party ${this.code}: ${role} with a wrong key from ${address}`);
        failed();
        return bye(ws, "bad-key");
      }
      const screens = [...this.#clients].filter((c) => c.role !== "player").length;
      if (screens >= this.#limits.screens) return bye(ws, "full");
      const c = new Client(ws, role, `s${++this.#screenSeq}`, address, this.#limits.rate, this.#limits.burst);
      this.#adopt(c);
      this.#welcome(c);
      this.emit("screen", { role, id: c.id }, true);
      return;
    }
    if (role !== "player") return bye(ws, "protocol");
    // A known player coming back.
    if (typeof hello.resume === "string") {
      const p = this.#players.find((x) => sameToken(hello.resume, x.resume));
      if (p) {
        const old = p.client;
        const c = new Client(ws, "player", p.id, address, this.#limits.rate, this.#limits.burst);
        c.player = p;
        p.client = c;
        if (old) {
          old.player = undefined;
          old.bye("replaced");
          this.#clients.delete(old);
        }
        this.#adopt(c);
        this.#welcome(c);
        this.#log("info", `party ${this.code}: ${p.name} is back`);
        this.emit("reconnect", p);
        this.#changed();
        return;
      }
    }
    if (!sameToken(hello.key, this.joinKey)) {
      this.#log("warn", `party ${this.code}: a join with a wrong key from ${address}`);
      failed();
      return bye(ws, "bad-key");
    }
    if (hello.name === undefined || hello.name === null) return bye(ws, "no-player");
    if (this.#locked) return bye(ws, "locked");
    if (this.#players.length >= this.maxPlayers) return bye(ws, "full");
    let name = cleanText(hello.name, MAX_NAME);
    if (!name) return bye(ws, "name-invalid", "Pick a name");
    const moderated = this.moderate(name, "name");
    if (moderated === false) return bye(ws, "name-invalid", "Pick another name");
    name = cleanText(moderated, MAX_NAME);
    if (!name) return bye(ws, "name-invalid", "Pick a name");
    const key = nameKey(name);
    if (this.#players.some((p) => nameKey(p.name) === key)) return bye(ws, "name-taken", "Someone has that name");
    const p = new Player(`p${++this.#seq}`, name, () => this.#changed());
    const c = new Client(ws, "player", p.id, address, this.#limits.rate, this.#limits.burst);
    c.player = p;
    p.client = c;
    this.#players.push(p);
    if (!this.vip) {
      p.vip = true;
    }
    this.#adopt(c);
    this.#welcome(c);
    this.#log("info", `party ${this.code}: ${name} joined (${address})`);
    this.emit("join", p);
    if (p.vip) this.emit("vip", p);
    this.#changed();
  }

  #adopt(c: Client): void {
    this.#clients.add(c);
    c.ws.on("message", (data, binary) => this.#onMessage(c, data, binary));
    c.ws.on("close", () => this.#onClose(c));
    c.ws.socket.on("drain", () => {
      if (c.waiting) this.#flushClient(c);
    });
  }

  #viewFor(c: Client): JsonObject {
    return c.player ? this.playerView(c.player) : this.screenView();
  }

  #welcome(c: Client): void {
    const view = toJson(this.#viewFor(c)) as JsonObject;
    c.sent = view;
    c.rev = 0;
    const you: JsonObject = { id: c.id, role: c.role };
    if (c.player) {
      you["name"] = c.player.name;
      you["vip"] = c.player.vip;
      you["resume"] = c.player.resume;
      you["lastAct"] = c.player.lastAct;
    }
    c.send({ t: "welcome", v: PROTOCOL, room: { code: this.code }, you, now: this.now(), view, rev: 0 });
  }

  #onClose(c: Client): void {
    this.#clients.delete(c);
    const p = c.player;
    if (p && p.client === c) {
      p.client = undefined;
      this.#log("info", `party ${this.code}: ${p.name} is away`);
      this.emit("away", p);
      this.#changed();
    } else if (c.role !== "player") {
      this.emit("screen", { role: c.role as "screen" | "watch", id: c.id }, false);
    }
  }

  #remove(p: Player, reason: string): void {
    const i = this.#players.indexOf(p);
    if (i < 0) return;
    this.#players.splice(i, 1);
    if (p.client) {
      p.client.player = undefined;
      p.client = undefined;
    }
    const wasVip = p.vip;
    p.vip = false;
    this.#log("info", `party ${this.code}: ${p.name} left (${reason})`);
    this.emit("leave", p, reason);
    if (wasVip) {
      const next = this.#players.find((x) => x.connected) ?? this.#players[0];
      if (next) next.vip = true;
      this.emit("vip", next);
    }
    this.#changed();
  }

  #onMessage(c: Client, data: string | Buffer, binary: boolean): void {
    if (!c.bucket.take()) return c.bye("rate-limited");
    if (binary) return c.bye("protocol", "binary messages are not used");
    let msg: unknown;
    try {
      msg = JSON.parse(data as string);
    } catch {
      return c.bye("protocol", "not JSON");
    }
    if (!isObject(msg)) return c.bye("protocol", "not an object");
    if (msg["t"] === "in" && (data as string).length > MAX_INPUT) return;
    if (msg["t"] !== "act" && (data as string).length > MAX_CONTROL_MESSAGE) return c.bye("protocol", "too big");
    switch (msg["t"]) {
      case "act":
        return void this.#onAct(c, msg);
      case "ping":
        c.send({ t: "pong", c: typeof msg["c"] === "number" ? msg["c"] : 0, now: this.now() });
        return;
      case "resync": {
        const view = toJson(this.#viewFor(c)) as JsonObject;
        c.sent = view;
        c.send({ t: "state", rev: ++c.rev, full: view });
        return;
      }
      case "in": {
        // Latest wins by `s` (per connection).
        const seq = msg["s"];
        const p = c.player;
        if (!p || typeof seq !== "number" || !Number.isSafeInteger(seq) || seq <= c.inSeq) return;
        c.inSeq = seq;
        p.input = msg["data"] ?? null;
        this.emit("input", p, p.input);
        return;
      }
      case "leave":
        if (c.player) {
          const p = c.player;
          this.#remove(p, "left");
        }
        c.ws.close(1000, "left");
        return;
      default:
        return; // unknown types are ignored
    }
  }

  async #onAct(c: Client, msg: JsonObject): Promise<void> {
    const id = msg["id"];
    const type = msg["type"];
    if (typeof id !== "number" || !Number.isSafeInteger(id) || id <= 0 || typeof type !== "string") {
      return c.bye("protocol", "bad act");
    }
    const data = msg["data"] ?? null;
    if (c.role === "watch") {
      c.send({ t: "ack", id, ok: false, error: { code: "read-only", message: "Watchers cannot act" } });
      return;
    }
    const p = c.player;
    if (!p) {
      if (id <= c.lastAct) return;
      c.lastAct = id;
      const ack = await this.#run(id, type, data, { role: c.role as "screen", id: c.id });
      c.send(ack);
      return;
    }
    if (id <= p.lastAct) {
      const cached = p.results.get(id);
      const ack: Ack = cached ? await cached : { t: "ack", id, ok: true, dup: true };
      p.client?.send(ack);
      return;
    }
    p.lastAct = id;
    const pending = this.#run(id, type, data, p);
    p.results.set(id, pending);
    const ack = await pending;
    p.results.set(id, ack);
    while (p.results.size > ACK_CACHE) p.results.delete(p.results.keys().next().value!);
    // The player's current socket: it may have reconnected meanwhile.
    p.client?.send(ack);
  }

  async #run(id: number, type: string, data: Json, from: Sender): Promise<Ack> {
    const handler = this.#handlers.get(type);
    if (!handler) return { t: "ack", id, ok: false, error: { code: "unknown-action", message: `No ${type} now` } };
    try {
      const result = await handler(data, from);
      const ack: Ack = { t: "ack", id, ok: true };
      if (result !== undefined && result !== null) ack.result = toJson(result);
      return ack;
    } catch (err) {
      if (err instanceof PartyError) return { t: "ack", id, ok: false, error: { code: err.code, message: err.message } };
      this.#log("error", `party ${this.code}: action ${type}: ${err instanceof Error ? (err.stack ?? err.message) : String(err)}`);
      return { t: "ack", id, ok: false, error: { code: "internal", message: "Something went wrong" } };
    }
  }

  // -------------------------------------------------------------------------
  // Views

  #changed(): void {
    this.#screenView = undefined;
    if (this.#flushQueued) return;
    this.#flushQueued = true;
    queueMicrotask(() => {
      this.#flushQueued = false;
      for (const c of this.#clients) this.#flushClient(c);
      if (this.listenerCount("screen-view") > 0) this.emit("screen-view", this.screenView());
    });
  }

  #flushClient(c: Client): void {
    if (c.ws.closed || !c.sent || (c.role === "player" && !c.player)) return;
    if (c.ws.bufferedAmount > STATE_HIGH_WATER) {
      // Latest wins: one diff against what it has once the socket drains.
      c.waiting = true;
      return;
    }
    c.waiting = false;
    const view = toJson(this.#viewFor(c)) as JsonObject;
    const d = diff(c.sent, view);
    if (!d) return;
    c.sent = view;
    c.send({ t: "state", rev: ++c.rev, ...(d as JsonObject) });
  }
}

function bye(ws: WsConnection, reason: ByeReason, message?: string): void {
  const msg: JsonObject = { t: "bye", reason };
  if (message) msg["message"] = message;
  ws.send(JSON.stringify(msg));
  ws.close(BYE[reason], reason);
}

export { bye as sendBye };
