/**
 * WebSocket server side (RFC 6455) over a Node socket: the upgrade
 * handshake, frames, fragmentation, ping/pong and the close handshake.
 * Node ships a WebSocket client but no server; this is the part a party
 * server needs, nothing more (no extensions, no subprotocols).
 *
 * Every violation by the peer closes the connection with the right code
 * (1002 protocol, 1007 invalid UTF-8, 1009 too big) and never throws.
 */

import { createHash } from "node:crypto";
import { EventEmitter } from "node:events";
import type { IncomingMessage } from "node:http";
import type { Duplex } from "node:stream";

const GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

export const CloseCode = {
  NORMAL: 1000,
  GOING_AWAY: 1001,
  PROTOCOL: 1002,
  UNSUPPORTED: 1003,
  NO_STATUS: 1005,
  ABNORMAL: 1006,
  INVALID_DATA: 1007,
  POLICY: 1008,
  TOO_BIG: 1009,
  INTERNAL: 1011,
} as const;

const OP_CONT = 0x0;
const OP_TEXT = 0x1;
const OP_BINARY = 0x2;
const OP_CLOSE = 0x8;
const OP_PING = 0x9;
const OP_PONG = 0xa;

/** `Sec-WebSocket-Accept` for a `Sec-WebSocket-Key`. */
export function acceptKey(key: string): string {
  return createHash("sha1").update(key + GUID).digest("base64");
}

/** Whether a request asks for a WebSocket upgrade. */
export function isUpgrade(req: IncomingMessage): boolean {
  return (
    (req.headers.upgrade ?? "").toLowerCase() === "websocket" &&
    (req.headers.connection ?? "")
      .toLowerCase()
      .split(",")
      .some((t) => t.trim() === "upgrade")
  );
}

/** Refuses an upgrade with a plain HTTP response and closes the socket. */
export function refuseUpgrade(socket: Duplex, status: number, reason: string): void {
  const body = `${status} ${reason}\n`;
  socket.end(
    `HTTP/1.1 ${status} ${reason}\r\nContent-Type: text/plain; charset=utf-8\r\n` +
      `Content-Length: ${Buffer.byteLength(body)}\r\nConnection: close\r\n\r\n${body}`,
  );
}

export interface WsOptions {
  /** Largest message (all fragments), bytes. Default 1 MiB. */
  maxMessage?: number | undefined;
}

export interface WsEvents {
  message: [data: string | Buffer, binary: boolean];
  close: [code: number, reason: string];
  pong: [data: Buffer];
}

/**
 * Completes the handshake (the caller checked Host, Origin and the path)
 * and returns the connection, or refuses a malformed request (400) and
 * returns `undefined`.
 */
export function accept(
  req: IncomingMessage,
  socket: Duplex,
  head: Buffer,
  options: WsOptions = {},
): WsConnection | undefined {
  const key = req.headers["sec-websocket-key"];
  const version = req.headers["sec-websocket-version"];
  if (
    req.method !== "GET" ||
    !isUpgrade(req) ||
    typeof key !== "string" ||
    Buffer.from(key, "base64").length !== 16 ||
    version !== "13"
  ) {
    refuseUpgrade(socket, 400, "Bad Request");
    return undefined;
  }
  socket.write(
    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
      `Sec-WebSocket-Accept: ${acceptKey(key)}\r\n\r\n`,
  );
  return new WsConnection(socket, head, options);
}

/** Encodes one unmasked (server) frame, or a masked one with `mask`. */
export function encodeFrame(opcode: number, payload: Buffer, fin = true, mask?: Buffer): Buffer {
  const len = payload.length;
  const ext = len < 126 ? 0 : len < 65536 ? 2 : 8;
  const head = Buffer.alloc(2 + ext + (mask ? 4 : 0));
  head[0] = (fin ? 0x80 : 0) | opcode;
  head[1] = (mask ? 0x80 : 0) | (ext === 0 ? len : ext === 2 ? 126 : 127);
  if (ext === 2) head.writeUInt16BE(len, 2);
  if (ext === 8) head.writeBigUInt64BE(BigInt(len), 2);
  if (!mask) return Buffer.concat([head, payload]);
  mask.copy(head, 2 + ext);
  const body = Buffer.from(payload);
  for (let i = 0; i < body.length; i++) body[i]! ^= mask[i & 3]!;
  return Buffer.concat([head, body]);
}

function validCloseCode(code: number): boolean {
  return (code >= 1000 && code <= 1003) || (code >= 1007 && code <= 1014) || (code >= 3000 && code <= 4999);
}

const utf8 = new TextDecoder("utf-8", { fatal: true });

/** One server-side WebSocket connection. */
export class WsConnection extends EventEmitter<WsEvents> {
  readonly socket: Duplex;
  readonly #max: number;
  #buf: Buffer = Buffer.alloc(0);
  #frag: Buffer[] = [];
  #fragOp = 0;
  #fragLen = 0;
  #closeSent = false;
  #closed = false;
  #closeCode: number = CloseCode.ABNORMAL;
  #closeReason = "";

  constructor(socket: Duplex, head: Buffer, options: WsOptions = {}) {
    super();
    this.socket = socket;
    this.#max = options.maxMessage ?? 1 << 20;
    if ("setNoDelay" in socket && typeof socket.setNoDelay === "function") socket.setNoDelay(true);
    socket.on("data", (d: Buffer) => this.#onData(d));
    socket.on("error", () => {});
    socket.on("close", () => this.#finish());
    // The peer went away without a close frame.
    socket.on("end", () => this.terminate());
    if (head.length > 0) queueMicrotask(() => this.#onData(head));
  }

  /** Bytes written to the socket but not yet sent. */
  get bufferedAmount(): number {
    const s = this.socket as Duplex & { writableLength?: number };
    return s.writableLength ?? 0;
  }

  get closed(): boolean {
    return this.#closed || this.#closeSent;
  }

  send(data: string | Buffer | Uint8Array): void {
    if (this.closed) return;
    const binary = typeof data !== "string";
    const payload = typeof data === "string" ? Buffer.from(data, "utf8") : Buffer.from(data);
    this.socket.write(encodeFrame(binary ? OP_BINARY : OP_TEXT, payload));
  }

  ping(data: Buffer = Buffer.alloc(0)): void {
    if (!this.closed) this.socket.write(encodeFrame(OP_PING, data.subarray(0, 125)));
  }

  /** Starts the close handshake; the socket ends once the peer answers (or after 2 s). */
  close(code: number = CloseCode.NORMAL, reason = ""): void {
    if (this.#closeSent || this.#closed) return;
    this.#closeSent = true;
    this.#closeCode = code;
    this.#closeReason = reason;
    let r = Buffer.from(reason, "utf8");
    if (r.length > 123) r = r.subarray(0, 123);
    const payload = Buffer.alloc(2 + r.length);
    payload.writeUInt16BE(code, 0);
    r.copy(payload, 2);
    this.socket.write(encodeFrame(OP_CLOSE, payload));
    const t = setTimeout(() => this.socket.destroy(), 2000);
    t.unref();
  }

  /** Drops the connection without a handshake. */
  terminate(): void {
    this.socket.destroy();
    this.#finish();
  }

  #finish(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.emit("close", this.#closeCode, this.#closeReason);
  }

  #fail(code: number, reason: string): void {
    this.close(code, reason);
    this.#buf = Buffer.alloc(0);
    // Stop reading: nothing the peer sends after a violation matters.
    this.socket.pause();
    const t = setTimeout(() => this.socket.destroy(), 100);
    t.unref();
  }

  #onData(d: Buffer): void {
    if (this.#closed) return;
    this.#buf = this.#buf.length === 0 ? d : Buffer.concat([this.#buf, d]);
    while (!this.#closed) {
      const b = this.#buf;
      if (b.length < 2) return;
      const b0 = b[0]!;
      const b1 = b[1]!;
      const fin = (b0 & 0x80) !== 0;
      const rsv = b0 & 0x70;
      const op = b0 & 0x0f;
      const masked = (b1 & 0x80) !== 0;
      let len = b1 & 0x7f;
      let off = 2;
      if (len === 126) {
        if (b.length < 4) return;
        len = b.readUInt16BE(2);
        off = 4;
      } else if (len === 127) {
        if (b.length < 10) return;
        const big = b.readBigUInt64BE(2);
        if (big > BigInt(this.#max)) return this.#fail(CloseCode.TOO_BIG, "message too big");
        len = Number(big);
        off = 10;
      }
      if (rsv !== 0) return this.#fail(CloseCode.PROTOCOL, "reserved bits set");
      if (!masked) return this.#fail(CloseCode.PROTOCOL, "client frames must be masked");
      const control = op >= 0x8;
      if (control) {
        if (op !== OP_CLOSE && op !== OP_PING && op !== OP_PONG) return this.#fail(CloseCode.PROTOCOL, "unknown opcode");
        if (!fin || len > 125) return this.#fail(CloseCode.PROTOCOL, "bad control frame");
      } else if (op !== OP_CONT && op !== OP_TEXT && op !== OP_BINARY) {
        return this.#fail(CloseCode.PROTOCOL, "unknown opcode");
      }
      if (!control && this.#fragLen + len > this.#max) return this.#fail(CloseCode.TOO_BIG, "message too big");
      if (b.length < off + 4 + len) return;
      const mask = b.subarray(off, off + 4);
      const payload = Buffer.from(b.subarray(off + 4, off + 4 + len));
      for (let i = 0; i < payload.length; i++) payload[i]! ^= mask[i & 3]!;
      this.#buf = b.subarray(off + 4 + len);
      if (control) {
        this.#control(op, payload);
        continue;
      }
      if (op === OP_CONT) {
        if (this.#fragOp === 0) return this.#fail(CloseCode.PROTOCOL, "continuation without a start");
      } else {
        if (this.#fragOp !== 0) return this.#fail(CloseCode.PROTOCOL, "new message inside a fragmented one");
        this.#fragOp = op;
      }
      this.#frag.push(payload);
      this.#fragLen += payload.length;
      if (!fin) continue;
      const whole = this.#frag.length === 1 ? this.#frag[0]! : Buffer.concat(this.#frag);
      const msgOp = this.#fragOp;
      this.#frag = [];
      this.#fragLen = 0;
      this.#fragOp = 0;
      if (msgOp === OP_TEXT) {
        let text: string;
        try {
          text = utf8.decode(whole);
        } catch {
          return this.#fail(CloseCode.INVALID_DATA, "invalid UTF-8");
        }
        if (!this.#closeSent) this.emit("message", text, false);
      } else if (!this.#closeSent) {
        this.emit("message", whole, true);
      }
    }
  }

  #control(op: number, payload: Buffer): void {
    if (op === OP_PING) {
      if (!this.closed) this.socket.write(encodeFrame(OP_PONG, payload));
      return;
    }
    if (op === OP_PONG) {
      this.emit("pong", payload);
      return;
    }
    // Close.
    let code: number = CloseCode.NO_STATUS;
    let reason = "";
    if (payload.length === 1) return this.#fail(CloseCode.PROTOCOL, "bad close frame");
    if (payload.length >= 2) {
      code = payload.readUInt16BE(0);
      try {
        reason = utf8.decode(payload.subarray(2));
      } catch {
        return this.#fail(CloseCode.INVALID_DATA, "invalid UTF-8 in close");
      }
      if (!validCloseCode(code)) return this.#fail(CloseCode.PROTOCOL, "invalid close code");
    }
    if (!this.#closeSent) {
      this.#closeSent = true;
      this.#closeCode = code;
      this.#closeReason = reason;
      const echo = Buffer.alloc(code === CloseCode.NO_STATUS ? 0 : 2);
      if (echo.length) echo.writeUInt16BE(code, 0);
      this.socket.write(encodeFrame(OP_CLOSE, echo));
    }
    this.socket.end();
    this.#finish();
  }
}
