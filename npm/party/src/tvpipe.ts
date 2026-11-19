/**
 * What the party library uses from `@tvpipe/sdk` (an optional peer
 * dependency, loaded only when a tunnel, a QR code or a TV is asked for),
 * typed here so the library builds without it.
 */

export interface QrRows {
  size: number;
  rows: string[];
}

export interface TunnelLike {
  url: string;
  host: string;
  provider: string;
  stop(): Promise<unknown>;
  done: Promise<unknown>;
}

export interface SessionLike {
  done: Promise<unknown>;
  stop(): Promise<unknown>;
  on(event: string, fn: (...args: any[]) => void): unknown;
  playerUrl?: string | undefined;
  publicUrl?: string | undefined;
}

export interface CanvasLike {
  width: number;
  height: number;
  getContext(type: "2d"): any;
}

export interface StreamSessionLike extends SessionLike {
  drawLoop(canvas: CanvasLike, draw: (ctx: any, timeMs: number) => void): { stop(): void };
  writeAudio(pcm: Int16Array | Float32Array, ptsUs?: number): boolean;
}

/** Options passed through to every tvpipe process (binary, configDir, env, logs). */
export type TvpipeOptions = Record<string, unknown>;

export interface Sdk {
  tunnel(options: { port: number; provider?: string } & TvpipeOptions): Promise<TunnelLike>;
  qr(text: string, options?: TvpipeOptions): Promise<QrRows>;
  share(url: string, options: Record<string, unknown>): Promise<SessionLike>;
  stream(options: Record<string, unknown>): Promise<StreamSessionLike>;
}

let sdk: Promise<Sdk> | undefined;

/** `@tvpipe/sdk`, or a clear error when it is not installed. */
export function loadSdk(): Promise<Sdk> {
  sdk ??= import("@tvpipe/sdk" as string).then(
    (m) => m as Sdk,
    (err: unknown) => {
      sdk = undefined;
      throw new Error(
        `@tvpipe/party needs @tvpipe/sdk for tunnels, QR codes and the TV: npm install @tvpipe/sdk (${err instanceof Error ? err.message : String(err)})`,
      );
    },
  );
  return sdk;
}
