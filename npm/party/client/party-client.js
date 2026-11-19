// @ts-check
/**
 * The browser side of @tvpipe/party (docs/party-design.md §5): served by
 * the party server at `/party/client.js`, imported by the game's phone
 * and screen pages. A plain ES module, no build step; it also runs in
 * Node ≥ 22 (tests use it as fake phones).
 *
 *   import { PartyClient } from "/party/client.js";
 *   const party = PartyClient.fromLocation();
 *   party.on("view", (view) => render(view));
 *   await party.join("Ann");                 // or party.connect() with a saved identity
 *   await party.act("answer", { text: "…" }); // acked, resent after reconnects
 */

export const PROTOCOL = 1;

/** Closes after which the client does not reconnect (`bye` reasons). */
const FINAL = new Set([
  "bad-key",
  "no-room",
  "no-player",
  "name-taken",
  "name-invalid",
  "replaced",
  "kicked",
  "room-closed",
  "full",
  "locked",
  "protocol",
  "left",
  "rate-limited",
]);

/** Unacknowledged actions kept at most. */
const MAX_PENDING = 32;

/** An action the host refused, or one that could not be sent. */
export class PartyActionError extends Error {
  /**
   * @param {string} code
   * @param {string} message
   */
  constructor(code, message) {
    super(message);
    this.name = "PartyActionError";
    this.code = code;
  }
}

/**
 * @param {unknown} v
 * @returns {v is Record<string, any>}
 */
function isObject(v) {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/**
 * Applies a `state` diff to a view in place (`set` merges objects, replaces
 * everything else; then `del` paths are removed). The server's
 * `applyDiff` follows the same rules.
 * @param {Record<string, any>} view
 * @param {{ set?: Record<string, any>, del?: string[][] }} d
 */
export function applyDiff(view, d) {
  if (d.set) merge(view, d.set);
  for (const path of d.del ?? []) {
    /** @type {any} */
    let o = view;
    for (let i = 0; i < path.length - 1 && isObject(o); i++) o = o[path[i]];
    if (isObject(o)) delete o[path[path.length - 1]];
  }
  return view;
}

/**
 * @param {Record<string, any>} target
 * @param {Record<string, any>} patch
 */
function merge(target, patch) {
  for (const k of Object.keys(patch)) {
    const pv = patch[k];
    if (isObject(pv) && isObject(target[k])) merge(target[k], pv);
    else target[k] = pv;
  }
}

/**
 * An SVG document of a QR code (`view.join.qr` on screens): dark modules
 * on a light square with a quiet zone. Insert it as markup (it holds no
 * text from players).
 * @param {{ size: number, rows: string[] } | null | undefined} qr
 * @param {{ dark?: string, light?: string, quiet?: number }} [opts]
 */
export function qrSvg(qr, opts = {}) {
  if (!qr) return "";
  const quiet = opts.quiet ?? 4;
  const n = qr.size + 2 * quiet;
  const color = (/** @type {string} */ c) => (/^[#a-zA-Z0-9(),.% ]+$/.test(c) ? c : "#000");
  let d = "";
  qr.rows.forEach((row, y) => {
    for (let x = 0; x < row.length; x++) {
      if (row[x] !== "1") continue;
      let w = 1;
      while (row[x + w] === "1") w++;
      d += `M${x + quiet} ${y + quiet}h${w}v1h-${w}z`;
      x += w - 1;
    }
  });
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${n} ${n}" shape-rendering="crispEdges">` +
    `<rect width="${n}" height="${n}" fill="${color(opts.light ?? "#fff")}"/>` +
    `<path d="${d}" fill="${color(opts.dark ?? "#000")}"/></svg>`
  );
}

/**
 * Draws a QR code into a 2D canvas context: `size` px square including the
 * quiet zone.
 * @param {{ fillStyle: any, fillRect(x: number, y: number, w: number, h: number): void }} ctx
 * @param {{ size: number, rows: string[] } | null | undefined} qr
 * @param {number} x
 * @param {number} y
 * @param {number} size
 * @param {{ dark?: string, light?: string, quiet?: number }} [opts]
 */
export function drawQr(ctx, qr, x, y, size, opts = {}) {
  if (!qr) return;
  const quiet = opts.quiet ?? 4;
  const px = size / (qr.size + 2 * quiet);
  ctx.fillStyle = opts.light ?? "#fff";
  ctx.fillRect(x, y, size, size);
  ctx.fillStyle = opts.dark ?? "#000";
  qr.rows.forEach((row, r) => {
    for (let c = 0; c < row.length; c++) {
      if (row[c] !== "1") continue;
      let w = 1;
      while (row[c + w] === "1") w++;
      // Snap to whole pixels so neighbouring modules leave no seams.
      const x0 = Math.round(x + (c + quiet) * px);
      const x1 = Math.round(x + (c + w + quiet) * px);
      const y0 = Math.round(y + (r + quiet) * px);
      const y1 = Math.round(y + (r + 1 + quiet) * px);
      ctx.fillRect(x0, y0, x1 - x0, y1 - y0);
      c += w - 1;
    }
  });
}

/**
 * @typedef {object} PartyClientOptions
 * @property {string} url The WebSocket URL (`ws(s)://HOST/party/ws`).
 * @property {string} room The room code.
 * @property {string} [key] The join key (players) or the screen/watch key.
 * @property {"player" | "screen" | "watch"} [role] Default `player`.
 * @property {StorageLike | null} [storage] Where the identity is kept (default `localStorage` when there is one).
 * @property {typeof WebSocket} [WebSocket] Default the global one.
 */

/**
 * @typedef {object} StorageLike
 * @property {(key: string) => string | null} getItem
 * @property {(key: string, value: string) => void} setItem
 * @property {(key: string) => void} removeItem
 */

/**
 * @typedef {"idle" | "connecting" | "open" | "reconnecting" | "closed"} Status
 */

export class PartyClient {
  /**
   * The party of the page's link: `?r=CODE` (or `#r=CODE`), `#k=KEY` and
   * `#role=screen|watch` (players: none).
   * @param {Partial<PartyClientOptions>} [opts]
   */
  static fromLocation(opts = {}) {
    const loc = globalThis.location;
    const q = new URLSearchParams(loc.search);
    const h = new URLSearchParams(loc.hash.replace(/^#/, ""));
    const room = opts.room ?? h.get("r") ?? q.get("r") ?? "";
    const key = opts.key ?? h.get("k") ?? q.get("k") ?? undefined;
    const url = opts.url ?? `${loc.protocol === "https:" ? "wss:" : "ws:"}//${loc.host}/party/ws`;
    const r = opts.role ?? h.get("role") ?? "player";
    const role = r === "screen" || r === "watch" ? r : "player";
    return new PartyClient({ ...opts, url, room, key, role });
  }

  /** @param {PartyClientOptions} opts */
  constructor(opts) {
    this.url = opts.url;
    this.room = opts.room.toUpperCase();
    this.key = opts.key;
    /** @type {"player" | "screen" | "watch"} */
    this.role = opts.role ?? "player";
    /** @type {StorageLike | null} */
    this.storage = opts.storage !== undefined ? opts.storage : safeStorage();
    this.WS = opts.WebSocket ?? globalThis.WebSocket;
    /** The current view (`{ roster, room, me }` or `{ roster, screen, join }`). @type {Record<string, any>} */
    this.view = {};
    /** Who this client is, from `welcome`. @type {Record<string, any> | null} */
    this.you = null;
    /** @type {Status} */
    this.status = "idle";
    /** The reason of the last `bye`, if any. @type {string | null} */
    this.byeReason = null;
    /** @type {Map<string, Set<(...args: any[]) => void>>} */
    this._listeners = new Map();
    /** @type {WebSocket | null} */
    this._ws = null;
    this._rev = 0;
    this._attempt = 0;
    /** @type {ReturnType<typeof setTimeout> | null} */
    this._retry = null;
    /** @type {ReturnType<typeof setInterval> | null} */
    this._pinger = null;
    /** @type {string | undefined} */
    this._name = undefined;
    this._nextAct = 1;
    /** @type {Map<number, { type: string, data: any, resolve: (v: any) => void, reject: (e: Error) => void }>} */
    this._pending = new Map();
    /** Clock: server time = Date.now() + offset. */
    this._offset = 0;
    this._bestRtt = Infinity;
    /** @type {{ rtt: number, offset: number }[]} */
    this._samples = [];
    /** @type {{ resolve: (v: any) => void, reject: (e: Error) => void } | null} */
    this._waiting = null;
    this._onVisible = () => {
      if (globalThis.document?.visibilityState === "visible" && this.status === "reconnecting") {
        this._attempt = 0;
        this._open();
      }
    };
  }

  /** @returns {string} */
  get _storageKey() {
    const host = (() => {
      try {
        return new URL(this.url).host;
      } catch {
        return "";
      }
    })();
    return `tvpipe-party:${host}:${this.room}:${this.role}`;
  }

  /** The saved identity (`resume`, `name`) of this room, if any. */
  get saved() {
    try {
      const s = this.storage?.getItem(this._storageKey);
      const v = s ? JSON.parse(s) : null;
      return isObject(v) ? /** @type {{ resume?: string, name?: string }} */ (v) : null;
    } catch {
      return null;
    }
  }

  /** Whether `connect()` can come back as an earlier player. */
  get canResume() {
    return this.role !== "player" || typeof this.saved?.resume === "string";
  }

  /**
   * @param {string} event `view`, `welcome`, `msg`, `status`, `bye`, `ack`
   * @param {(...args: any[]) => void} fn
   */
  on(event, fn) {
    let set = this._listeners.get(event);
    if (!set) this._listeners.set(event, (set = new Set()));
    set.add(fn);
    return () => set.delete(fn);
  }

  /**
   * @param {string} event
   * @param {(...args: any[]) => void} fn
   */
  off(event, fn) {
    this._listeners.get(event)?.delete(fn);
  }

  /**
   * @param {string} event
   * @param {...any} args
   */
  _emit(event, ...args) {
    for (const fn of [...(this._listeners.get(event) ?? [])]) {
      try {
        fn(...args);
      } catch (err) {
        console.error(err);
      }
    }
  }

  /**
   * Joins as a new player with this name (or comes back as the saved one).
   * Rejects with a {@link PartyActionError} (`name-taken`, `full`, …).
   * @param {string} name
   */
  join(name) {
    this._name = name;
    return this.connect();
  }

  /** Connects: a screen, or a player with a saved identity. Resolves with `welcome`. */
  connect() {
    if (this._name === undefined) this._name = this.saved?.name;
    return new Promise((resolve, reject) => {
      this._waiting = { resolve, reject };
      this.byeReason = null;
      this._attempt = 0;
      globalThis.document?.addEventListener?.("visibilitychange", this._onVisible);
      this._open();
    });
  }

  _open() {
    if (this._retry) clearTimeout(this._retry);
    this._retry = null;
    if (this._ws) {
      const old = this._ws;
      this._ws = null;
      old.onclose = null;
      try {
        old.close();
      } catch {}
    }
    this._setStatus(this.you ? "reconnecting" : "connecting");
    /** @type {WebSocket} */
    let ws;
    try {
      ws = new this.WS(this.url);
    } catch (err) {
      return this._fail("protocol", String(err));
    }
    this._ws = ws;
    ws.onopen = () => {
      const saved = this.saved;
      /** @type {Record<string, any>} */
      const hello = { t: "hello", v: PROTOCOL, room: this.room, role: this.role, key: this.key };
      if (this.role === "player") {
        if (saved?.resume) hello.resume = saved.resume;
        if (this._name !== undefined) hello.name = this._name;
      }
      hello.agent = globalThis.navigator?.userAgent ?? "";
      ws.send(JSON.stringify(hello));
    };
    ws.onmessage = (ev) => {
      if (typeof ev.data !== "string") return;
      /** @type {any} */
      let msg;
      try {
        msg = JSON.parse(ev.data);
      } catch {
        return;
      }
      if (isObject(msg)) this._onMessage(msg);
    };
    ws.onclose = (ev) => {
      if (this._ws !== ws) return;
      this._ws = null;
      this._stopPing();
      const reason = this.byeReason;
      if (reason && FINAL.has(reason)) return this._fail(reason, ev.reason || reason);
      if (this.status === "closed") return;
      this._setStatus(this.you ? "reconnecting" : "connecting");
      const delay = Math.min(5000, 250 * 2 ** this._attempt++) * (0.75 + Math.random() * 0.5);
      this._retry = setTimeout(() => this._open(), delay);
    };
  }

  /**
   * @param {string} reason
   * @param {string} message
   */
  _fail(reason, message) {
    this._setStatus("closed");
    if (reason === "no-player" || reason === "kicked" || reason === "bad-key") this._forget();
    const err = new PartyActionError(reason, message);
    for (const p of this._pending.values()) p.reject(err);
    this._pending.clear();
    if (this._waiting) {
      this._waiting.reject(err);
      this._waiting = null;
    }
    globalThis.document?.removeEventListener?.("visibilitychange", this._onVisible);
  }

  _forget() {
    try {
      this.storage?.removeItem(this._storageKey);
    } catch {}
  }

  /** @param {Status} s */
  _setStatus(s) {
    if (this.status === s) return;
    this.status = s;
    this._emit("status", s);
  }

  /** @param {Record<string, any>} msg */
  _onMessage(msg) {
    switch (msg.t) {
      case "welcome": {
        const first = this.you === null;
        this.you = msg.you;
        this.view = msg.view ?? {};
        this._rev = msg.rev ?? 0;
        this._attempt = 0;
        this._sample(0, msg.now);
        if (this.role === "player" && msg.you?.resume) {
          try {
            this.storage?.setItem(this._storageKey, JSON.stringify({ resume: msg.you.resume, name: msg.you.name }));
          } catch {}
        }
        this._setStatus("open");
        // Continue after what the server has seen; resend what it has not acked.
        const last = Number(msg.you?.lastAct ?? 0);
        if (first) {
          // Actions queued before the first welcome were numbered blindly
          // (a reloaded page starts at 1): number them after the server's.
          const queued = [...this._pending.values()];
          this._pending.clear();
          this._nextAct = last + 1;
          for (const p of queued) this._pending.set(this._nextAct++, p);
        }
        for (const [id, p] of this._pending) this._sendAct(id, p.type, p.data);
        this._startPing();
        this._emit("welcome", msg);
        this._emit("view", this.view, null);
        if (this._waiting) {
          this._waiting.resolve(msg);
          this._waiting = null;
        }
        return;
      }
      case "state": {
        if (msg.full) {
          this.view = msg.full;
        } else {
          if (msg.rev !== this._rev + 1) {
            this._send({ t: "resync" });
          }
          applyDiff(this.view, msg);
        }
        this._rev = msg.rev;
        this._emit("view", this.view, msg);
        return;
      }
      case "ack": {
        const p = this._pending.get(msg.id);
        if (!p) return;
        this._pending.delete(msg.id);
        if (msg.ok) p.resolve(msg.result ?? null);
        else p.reject(new PartyActionError(msg.error?.code ?? "error", msg.error?.message ?? "refused"));
        this._emit("ack", msg);
        return;
      }
      case "msg":
        this._emit("msg", msg.type, msg.data);
        return;
      case "pong":
        this._sample(msg.c, msg.now);
        return;
      case "bye":
        this.byeReason = String(msg.reason);
        this._emit("bye", this.byeReason, msg.message);
        return;
    }
  }

  /**
   * One clock sample: `c` is when the ping left (Date.now()), `now` the
   * server's time when it answered. Keeps the offset of the fastest of
   * the last 16 round trips.
   * @param {number} c
   * @param {number} now
   */
  _sample(c, now) {
    if (typeof now !== "number") return;
    const t = Date.now();
    const rtt = c > 0 ? t - c : 1000; // welcome: a rough first guess
    this._samples.push({ rtt, offset: now - (c > 0 ? (c + t) / 2 : t) });
    if (this._samples.length > 16) this._samples.shift();
    const best = this._samples.reduce((a, b) => (b.rtt < a.rtt ? b : a));
    this._offset = best.offset;
    this._bestRtt = best.rtt;
  }

  _startPing() {
    this._stopPing();
    let n = 0;
    const ping = () => this._send({ t: "ping", c: Date.now() });
    ping();
    this._pinger = setInterval(() => {
      ping();
      if (++n === 4 && this._pinger) {
        clearInterval(this._pinger);
        this._pinger = setInterval(ping, 10_000);
      }
    }, 300);
  }

  _stopPing() {
    if (this._pinger) clearInterval(this._pinger);
    this._pinger = null;
  }

  /** The server's clock (ms since the epoch): deadlines in views use it. */
  now() {
    return Date.now() + this._offset;
  }

  /** The fastest recent round trip to the server, ms. */
  get rtt() {
    return this._bestRtt;
  }

  /** @param {Record<string, any>} msg */
  _send(msg) {
    const ws = this._ws;
    if (!ws || ws.readyState !== 1 || this.status !== "open") return false;
    ws.send(JSON.stringify(msg));
    return true;
  }

  /**
   * @param {number} id
   * @param {string} type
   * @param {any} data
   */
  _sendAct(id, type, data) {
    this._send({ t: "act", id, type, data });
  }

  /**
   * Sends an action; resolves with the host's result, rejects with a
   * {@link PartyActionError} when the host refuses it. Survives reconnects
   * (sent again, applied once).
   * @param {string} type
   * @param {any} [data]
   * @returns {Promise<any>}
   */
  act(type, data = null) {
    if (this.status === "closed") return Promise.reject(new PartyActionError("closed", "Not connected"));
    if (this._pending.size >= MAX_PENDING) return Promise.reject(new PartyActionError("busy", "Too many actions waiting"));
    const id = this._nextAct++;
    return new Promise((resolve, reject) => {
      this._pending.set(id, { type, data, resolve, reject });
      this._sendAct(id, type, data);
    });
  }

  /** Leaves the party for good (the identity is forgotten). */
  leave() {
    this._send({ t: "leave" });
    this.byeReason = "left";
    this._forget();
    this.close();
  }

  /** Closes the connection (no reconnect). */
  close() {
    this._setStatus("closed");
    if (this._retry) clearTimeout(this._retry);
    this._stopPing();
    const ws = this._ws;
    this._ws = null;
    if (ws) {
      ws.onclose = null;
      try {
        ws.close();
      } catch {}
    }
    for (const p of this._pending.values()) p.reject(new PartyActionError("closed", "Closed"));
    this._pending.clear();
    globalThis.document?.removeEventListener?.("visibilitychange", this._onVisible);
  }

  /** For tests: drop the socket as a network loss would (the client reconnects). */
  _drop() {
    try {
      this._ws?.close(4999, "dropped");
    } catch {}
  }
}

/** @returns {StorageLike | null} */
function safeStorage() {
  try {
    const s = globalThis.localStorage;
    if (!s) return null;
    s.getItem("x");
    return s;
  } catch {
    return null;
  }
}

/**
 * A tiny in-memory storage (for tests, or browsers that refuse `localStorage`).
 * @returns {StorageLike}
 */
export function memoryStorage() {
  const m = new Map();
  return {
    getItem: (k) => (m.has(k) ? m.get(k) : null),
    setItem: (k, v) => void m.set(k, String(v)),
    removeItem: (k) => void m.delete(k),
  };
}
