/**
 * `tunnel()`: a public https link for any server on 127.0.0.1 (`tvpipe
 * tunnel PORT`), and `qr()`: QR codes as data (`tvpipe qr --rows`).
 */

import { execFile } from "node:child_process";

import { startOnce } from "./api.ts";
import { resolveBinary, type BinaryOptions } from "./binary.ts";
import { TvpipeError } from "./errors.ts";
import type { TunnelProvider } from "./events.ts";
import { TvpipeProcess, type ProcessOptions } from "./process.ts";
import { Session, type SessionOptions } from "./session.ts";

export interface TunnelOptions extends ProcessOptions, SessionOptions {
  /** The port of a server on 127.0.0.1. */
  port: number;
  /** Cloudflare (the default) or OpenTunnel (docs/tunnels.md). */
  provider?: TunnelProvider | undefined;
}

/** A running tunnel: `url` works until `stop()` (or this program ends). */
export class TunnelSession extends Session {
  /** `https://HOST/`. */
  url = "";
  /** The public host name: requests arrive with it as `Host`. */
  host = "";
  provider: TunnelProvider = "cloudflare";
}

/**
 * Puts a server on 127.0.0.1:`port` on a public https link; resolves once
 * the link works. The tunnel only carries requests: the server must
 * check `Host` (the tunnel's host or loopback) and, for WebSockets,
 * `Origin`, and decide who may do what.
 */
export function tunnel(options: TunnelOptions): Promise<TunnelSession> {
  const port = options.port;
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    return Promise.reject(new TvpipeError("invalid-argument", "`port` must be a port number"));
  }
  if (options.signal?.aborted) return Promise.reject(new TvpipeError("aborted", "aborted"));
  const args = ["tunnel", String(port)];
  if (options.provider) args.push(`--tunnel=${options.provider}`);
  return startOnce(
    options,
    args,
    (proc: TvpipeProcess) => {
      const s = new TunnelSession(proc, options);
      s.on("tunnel", (ev) => {
        if (ev.page !== "port") return;
        s.url = ev.url;
        s.host = new URL(ev.url).host;
        s.provider = ev.provider;
      });
      return s;
    },
    (ev) => ev.type === "tunnel" && ev.page === "port",
  );
}

/** A QR code: `size` modules per side, `rows[y][x]` is "1" for dark. */
export class QrCode {
  readonly size: number;
  readonly rows: string[];

  constructor(rows: string[]) {
    this.rows = rows;
    this.size = rows.length;
  }

  /** Whether the module in column `x`, row `y` is dark (outside: light). */
  dark(x: number, y: number): boolean {
    return this.rows[y]?.[x] === "1";
  }

  /** An SVG document, `quiet` light modules around it (default 4). */
  svg(options: { quiet?: number; dark?: string; light?: string } = {}): string {
    const quiet = options.quiet ?? 4;
    const n = this.size + 2 * quiet;
    let d = "";
    this.rows.forEach((row, y) => {
      for (const m of row.matchAll(/1+/g)) d += `M${m.index + quiet} ${y + quiet}h${m[0].length}v1h-${m[0].length}z`;
    });
    const esc = (c: string) => c.replace(/[^#a-zA-Z0-9(),.% ]/g, "");
    return (
      `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${n} ${n}" shape-rendering="crispEdges">` +
      `<rect width="${n}" height="${n}" fill="${esc(options.light ?? "#fff")}"/><path d="${d}" fill="${esc(options.dark ?? "#000")}"/></svg>`
    );
  }

  /** `{ size, rows }`, e.g. for a browser page. */
  toJSON(): { size: number; rows: string[] } {
    return { size: this.size, rows: this.rows };
  }
}

/** The QR code of `text` (byte mode, level M or L, the smallest version that fits). */
export function qr(text: string, options: BinaryOptions & Pick<ProcessOptions, "env"> = {}): Promise<QrCode> {
  const binary = resolveBinary(options);
  return new Promise((resolve, reject) => {
    execFile(
      binary,
      ["qr", "--rows", text],
      { windowsHide: true, env: { ...process.env, ...options.env }, maxBuffer: 1 << 20 },
      (err, stdout, stderr) => {
        if (err) {
          reject(new TvpipeError("invalid-argument", `tvpipe qr: ${String(stderr).trim() || err.message}`, { cause: err }));
          return;
        }
        const rows = stdout.split(/\r?\n/).filter((l) => l.length > 0);
        if (rows.length === 0 || !rows.every((r) => /^[01]+$/.test(r) && r.length === rows.length)) {
          reject(new TvpipeError("protocol", "tvpipe qr: unexpected output"));
          return;
        }
        resolve(new QrCode(rows));
      },
    );
  });
}
