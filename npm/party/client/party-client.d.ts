/** Types of the browser client (party-client.js). */

export declare const PROTOCOL: 1;

export interface QrRows {
  size: number;
  rows: string[];
}

export declare class PartyActionError extends Error {
  readonly code: string;
  constructor(code: string, message: string);
}

export declare function applyDiff<T extends Record<string, any>>(view: T, diff: { set?: Record<string, any>; del?: string[][] }): T;
export declare function qrSvg(qr: QrRows | null | undefined, opts?: { dark?: string; light?: string; quiet?: number }): string;
export declare function drawQr(
  ctx: { fillStyle: any; fillRect(x: number, y: number, w: number, h: number): void },
  qr: QrRows | null | undefined,
  x: number,
  y: number,
  size: number,
  opts?: { dark?: string; light?: string; quiet?: number },
): void;

export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}
export declare function memoryStorage(): StorageLike;

export type Role = "player" | "screen" | "watch";
export type Status = "idle" | "connecting" | "open" | "reconnecting" | "closed";

export interface PartyClientOptions {
  url: string;
  room: string;
  key?: string | undefined;
  role?: Role | undefined;
  storage?: StorageLike | null | undefined;
  WebSocket?: typeof WebSocket | undefined;
  /** Upgrade to WebRTC data channels when the host offers it (default true; players only). */
  rtc?: boolean | undefined;
  RTCPeerConnection?: typeof RTCPeerConnection | undefined;
  /** How long the channels may take to open, ms (default 3000). */
  rtcTimeout?: number | undefined;
}

export interface RosterEntry {
  id: string;
  name: string;
  vip: boolean;
  connected: boolean;
}

export interface Welcome {
  t: "welcome";
  v: number;
  room: { code: string };
  you: { id: string; role: Role; name?: string; vip?: boolean; resume?: string; lastAct?: number };
  now: number;
  view: Record<string, any>;
  rev: number;
  /** The host offers the WebRTC fast path (Rust hosts with `webrtc`). */
  rtc?: boolean;
}

export declare class PartyClient {
  static fromLocation(opts?: Partial<PartyClientOptions>): PartyClient;
  constructor(opts: PartyClientOptions);
  readonly url: string;
  readonly room: string;
  readonly key: string | undefined;
  readonly role: Role;
  readonly storage: StorageLike | null;
  /** `{ roster, room, me }` for players, `{ roster, screen, join }` for screens. */
  view: Record<string, any>;
  you: Welcome["you"] | null;
  status: Status;
  byeReason: string | null;
  readonly saved: { resume?: string; name?: string } | null;
  readonly canResume: boolean;
  readonly rtt: number;
  on(event: "view", fn: (view: Record<string, any>, diff: unknown) => void): () => void;
  on(event: "welcome", fn: (welcome: Welcome) => void): () => void;
  on(event: "msg", fn: (type: string, data: any) => void): () => void;
  on(event: "status", fn: (status: Status) => void): () => void;
  on(event: "bye", fn: (reason: string, message?: string) => void): () => void;
  /** Messages switched to WebRTC (`rtc`) or back to the WebSocket (`ws`). */
  on(event: "transport", fn: (transport: "rtc" | "ws") => void): () => void;
  on(event: string, fn: (...args: any[]) => void): () => void;
  off(event: string, fn: (...args: any[]) => void): void;
  join(name: string): Promise<Welcome>;
  connect(): Promise<Welcome>;
  act<T = any>(type: string, data?: unknown): Promise<T>;
  /** Continuous input (latest wins, never acked, may be lost). */
  input(data: unknown): void;
  /** Where messages travel now. */
  readonly transport: "rtc" | "ws";
  now(): number;
  leave(): void;
  close(): void;
  _drop(): void;
}
