/**
 * The machine interface's events (docs/machine.md), with camelCase fields.
 */

export type ReceiverProtocol = "cast" | "airplay" | "browser";

/**
 * - `connected`: ready to play.
 * - `failed`: could not resolve, connect or start, or failed mid-stream.
 * - `lost`: the connection broke; reconnecting (`reconnect` seconds).
 * - `reconnected`: back after `lost`.
 * - `gave-up`: reconnecting failed.
 * - `ended`: the receiver's user ended the session.
 * - `stopped`: stopped by tvpipe (the session ended).
 */
export type ReceiverState =
  | "connected"
  | "failed"
  | "lost"
  | "reconnected"
  | "gave-up"
  | "ended"
  | "stopped";

export interface HelloEvent {
  type: "hello";
  protocol: number;
  version: string;
  command: string;
}

export interface LogEvent {
  type: "log";
  level: "error" | "warn" | "info" | "debug" | "trace";
  module: string;
  message: string;
}

export interface ReceiverEvent {
  type: "receiver";
  name: string;
  /** Known on `connected` and on `failed` while connecting. */
  protocol?: ReceiverProtocol;
  state: ReceiverState;
  error?: string;
  /** A stable code, e.g. `pairing-required`. */
  code?: string;
}

export interface PlayerEvent {
  type: "player";
  name: string;
  /** The page of a `browser` receiver: open it in a browser to watch. */
  url: string;
}

export interface ViewerEvent {
  type: "viewer";
  name: string;
  tab: number;
  state: "connected" | "disconnected";
  /** It came over the public link. */
  public?: boolean;
}

export interface SharePageEvent {
  type: "share-page";
  url: string;
}

/** `tunnel` (or `player.tunnel`): the public link works. */
export interface TunnelEvent {
  type: "tunnel";
  page: "player" | "share";
  /** The player's name (`page: "player"`). */
  name?: string;
  provider: TunnelProvider;
  /** The link, with its token. */
  url: string;
}

/** Who carries a public link: Cloudflare (the default) or OpenTunnel. */
export type TunnelProvider = "cloudflare" | "opentunnel";

export interface ConnectedEvent {
  type: "connected";
  receivers: string[];
}

export interface StartedEvent {
  type: "started";
  route: "passthrough" | "ffmpeg" | "builtin" | "encoded" | "browser";
  video: { codec: string; width: number; height: number; fps: number };
  audio: { codec: string; sampleRate: number; channels: number }[];
  latencyMs: number;
}

export interface KeyframeEvent {
  type: "keyframe";
}

export interface ReceiverStats {
  name: string;
  pushed: number;
  dropped: number;
  droppedFull: number;
  droppedWaiting: number;
}

export interface StatsEvent {
  type: "stats";
  elapsedMs: number;
  videoFrames: number;
  audioFrames: number;
  receivers: ReceiverStats[];
}

export interface PinRequiredEvent {
  type: "pin-required";
  name: string;
  address: string;
}

export interface PairedEvent {
  type: "paired";
  name: string;
  model?: string;
  file: string;
}

export interface UnpairedEvent {
  type: "unpaired";
  name: string;
  id: string;
}

export interface DevicesEvent {
  type: "devices";
  devices: RawDevice[];
}

export interface ReceiverEnd extends ReceiverStats {
  end: "stopped" | "ended" | "failed";
  error?: string;
}

export interface EndEvent {
  type: "end";
  reason: "eof" | "stopped" | "receivers-gone" | "error" | "done";
  receivers?: ReceiverEnd[];
}

export interface ErrorEvent {
  type: "error";
  kind: string;
  code?: string;
  message: string;
  device?: string;
}

/** A device as `discover` reports it (camelCased). */
export interface RawDevice {
  name: string;
  addresses: string[];
  model?: string;
  airplay?: Record<string, unknown>;
  raop?: Record<string, unknown>;
  cast?: Record<string, unknown>;
}

export type TvpipeEvent =
  | HelloEvent
  | LogEvent
  | ReceiverEvent
  | PlayerEvent
  | ViewerEvent
  | SharePageEvent
  | TunnelEvent
  | ConnectedEvent
  | StartedEvent
  | KeyframeEvent
  | StatsEvent
  | PinRequiredEvent
  | PairedEvent
  | UnpairedEvent
  | DevicesEvent
  | EndEvent
  | ErrorEvent;

/** snake_case keys to camelCase, recursively. */
export function camelize(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(camelize);
  if (value !== null && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value)) {
      out[k.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase())] = camelize(v);
    }
    return out;
  }
  return value;
}

/** Parses one stdout line; `undefined` for anything that is not an event. */
export function parseEvent(line: string): TvpipeEvent | undefined {
  if (!line.startsWith("{")) return undefined;
  try {
    const v = camelize(JSON.parse(line)) as { type?: unknown };
    return typeof v.type === "string" ? (v as TvpipeEvent) : undefined;
  } catch {
    return undefined;
  }
}
