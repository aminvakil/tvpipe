/**
 * `createParty()`: a server on 127.0.0.1, one party, the public link
 * (through `tvpipe tunnel`) and its QR code, in one call; `toTv()` puts
 * the party's main screen on a TV (docs/party-design.md §7).
 */

import { Party, type Logger, type LogLevel, type PartyOptions } from "./party.ts";
import { PartyServer, type ServerOptions } from "./server.ts";
import { loadSdk, type CanvasLike, type QrRows, type SessionLike, type StreamSessionLike, type TunnelLike, type TvpipeOptions } from "./tvpipe.ts";

export type TunnelChoice = boolean | "cloudflare" | "opentunnel" | "auto";

export interface CreatePartyOptions extends ServerOptions, PartyOptions {
  /**
   * A public link for phones (default `"auto"`: Cloudflare, else
   * OpenTunnel when Cloudflare refuses); `false`: local only (the link is
   * http://127.0.0.1, for tests and phones on this computer).
   */
  tunnel?: TunnelChoice | undefined;
  /** Make the QR code of the join link (default true; needs @tvpipe/sdk). */
  qr?: boolean | ((text: string) => Promise<QrRows | null>) | undefined;
  /** Options for every tvpipe process (`binary`, `configDir`, `env`). */
  tvpipe?: TvpipeOptions | undefined;
  /** Log lines (default: warnings and errors to stderr). `false`: none. */
  log?: Logger | undefined;
  /** With the default logger: the least level printed (default `warn`). */
  logLevel?: LogLevel | undefined;
}

export interface PartyLinks {
  /** What phones open (the QR code): public with a tunnel, else local. */
  join: string;
  /** The join link on this computer. */
  localJoin: string;
  /** The screen page on this computer (for `share`), with the screen key. */
  screen: string;
  /** A read-only screen for people elsewhere (with a tunnel). */
  watch: string | undefined;
}

/** A party with its server, link and tunnel; `close()` ends all of them. */
export interface HostedParty extends Party {
  readonly server: PartyServer;
  readonly links: PartyLinks;
  readonly tunnel: TunnelLike | undefined;
  /** Puts the main screen on a TV (or a browser tab, `to: "browser"`). */
  toTv(options: ToTvOptions): Promise<TvHandle>;
  /** Ends the party, the tunnel, any TV sessions and the server. */
  shutdown(): Promise<void>;
}

const LEVELS: LogLevel[] = ["debug", "info", "warn", "error"];

export function consoleLogger(min: LogLevel = "warn"): Logger {
  const floor = LEVELS.indexOf(min);
  return (level, message) => {
    if (LEVELS.indexOf(level) >= floor) console.error(`party: ${message}`);
  };
}

export async function createParty(options: CreatePartyOptions = {}): Promise<HostedParty> {
  const log = options.log ?? consoleLogger(options.logLevel);
  const server = new PartyServer({ ...options, log });
  await server.listen();
  const party = server.createParty({ ...options, log });
  let tunnel: TunnelLike | undefined;
  const tvs = new Set<TvHandle>();
  try {
    if ((options.tunnel ?? "auto") !== false) {
      tunnel = await openTunnel(server.port, options.tunnel ?? "auto", options.tvpipe ?? {}, log);
      server.allowHost(tunnel.host);
    }
  } catch (err) {
    await server.close();
    throw err;
  }
  const local = server.origin;
  const pub = tunnel ? `https://${tunnel.host}` : undefined;
  const links: PartyLinks = {
    join: `${pub ?? local}/?r=${party.code}#k=${party.joinKey}`,
    localJoin: `${local}/?r=${party.code}#k=${party.joinKey}`,
    screen: `${local}/screen/#r=${party.code}&k=${party.screenKey}&role=screen`,
    watch: pub ? `${pub}/screen/#r=${party.code}&k=${party.watchKey}&role=watch` : undefined,
  };
  let qr: QrRows | null = null;
  if (options.qr !== false) {
    try {
      qr =
        typeof options.qr === "function"
          ? await options.qr(links.join)
          : await (await loadSdk()).qr(links.join, options.tvpipe ?? {});
    } catch (err) {
      log("warn", `no QR code: ${err instanceof Error ? err.message : String(err)}`);
    }
  }
  party.joinInfo = { code: party.code, url: links.join, qr };
  let shut: Promise<void> | undefined;
  const hosted = Object.assign(party, {
    server,
    links,
    tunnel,
    toTv: async (o: ToTvOptions) => {
      const h = await toTv(hosted, o);
      tvs.add(h);
      return h;
    },
    shutdown: () =>
      (shut ??= (async () => {
        await Promise.allSettled([...tvs].map((t) => t.stop()));
        party.close();
        await server.close();
        await tunnel?.stop().catch(() => {});
      })()),
  }) as HostedParty;
  return hosted;
}

async function openTunnel(port: number, choice: TunnelChoice, tvpipe: TvpipeOptions, log: Logger): Promise<TunnelLike> {
  const sdk = await loadSdk();
  const provider = choice === true || choice === "auto" ? "cloudflare" : choice;
  try {
    return await sdk.tunnel({ ...tvpipe, port, provider: provider as string });
  } catch (err) {
    if (choice !== "auto") throw err;
    log("warn", `Cloudflare tunnel failed (${err instanceof Error ? err.message : String(err)}); trying OpenTunnel`);
    return await sdk.tunnel({ ...tvpipe, port, provider: "opentunnel" });
  }
}

// ---------------------------------------------------------------------------
// The TV

export interface ToTvCommon {
  /** Receivers (as in @tvpipe/sdk): names, `"browser"`, or a list. */
  to?: string | string[] | undefined;
  /** Start the TV session again when it fails (default true). */
  restart?: boolean | undefined;
  /** More options for @tvpipe/sdk's `share()`/`stream()` (latency, player, …). */
  sdk?: Record<string, unknown> | undefined;
}

export interface ToTvWeb extends ToTvCommon {
  /** The screen page (`/screen/`) in tvpipe's own browser (`share URL`). */
  mode: "web";
  /** The page's size (default: the TV's shape, up to 1920x1080). */
  size?: string | { width: number; height: number } | undefined;
}

export interface ToTvVideo extends ToTvCommon {
  /** Frames the game draws (`stream()` + `drawLoop`). */
  mode: "video";
  /** A canvas of the stream's size (e.g. @napi-rs/canvas `createCanvas(1280, 720)`). */
  canvas: CanvasLike;
  /** Draws the screen view (`party.screenView()`) at time `t` (ms). */
  render: (ctx: any, view: Record<string, any>, t: number) => void;
  fps?: number | undefined;
  /** PCM the game writes with `handle.session.writeAudio` (default none). */
  audio?: { sampleRate: number; channels: number } | undefined;
}

export type ToTvOptions = ToTvWeb | ToTvVideo;

export interface TvHandle {
  /** The current session (changes when it restarts). */
  readonly session: SessionLike | StreamSessionLike;
  /** Stops it for good. */
  stop(): Promise<void>;
}

/** Puts a party's main screen on a TV; see {@link ToTvOptions}. */
export async function toTv(party: Party & { links: PartyLinks }, options: ToTvOptions): Promise<TvHandle> {
  const sdk = await loadSdk();
  let stopped = false;
  let loop: { stop(): void } | undefined;
  const start = async (): Promise<SessionLike | StreamSessionLike> => {
    const common = { ...(options.sdk ?? {}), ...(options.to !== undefined ? { to: options.to } : {}) };
    if (options.mode === "web") {
      return sdk.share(party.links.screen, { ...common, ...(options.size ? { size: options.size } : {}) });
    }
    const { canvas, render } = options;
    const s = await sdk.stream({
      ...common,
      video: { width: canvas.width, height: canvas.height, fps: options.fps ?? 30, format: "rgba" },
      ...(options.audio ? { audio: options.audio } : {}),
    });
    loop = s.drawLoop(canvas, (ctx, t) => render(ctx, party.screenView(), t));
    return s;
  };
  let session = await start();
  const handle: TvHandle = {
    get session() {
      return session;
    },
    async stop() {
      stopped = true;
      loop?.stop();
      await session.stop().catch(() => {});
    },
  };
  const watch = (s: SessionLike) => {
    s.done.then(
      () => {},
      async () => {
        // Failed (every receiver gone, tvpipe died): try again, slowly.
        let wait = 2000;
        while (!stopped && options.restart !== false && !party.closed) {
          await new Promise((r) => setTimeout(r, wait));
          if (stopped) return;
          try {
            session = await start();
            watch(session);
            return;
          } catch {
            wait = Math.min(30_000, wait * 2);
          }
        }
      },
    );
  };
  watch(session);
  return handle;
}
