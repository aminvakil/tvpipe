/** Drawing the join link's QR code without a browser (the video screen). */

import type { QrRows } from "./tvpipe.ts";

export interface FillContext {
  fillStyle: unknown;
  fillRect(x: number, y: number, w: number, h: number): void;
}

/**
 * Draws a QR code into a 2D canvas context: `size` px square including a
 * quiet zone of `quiet` modules (default 4), modules snapped to whole
 * pixels. The same as the browser client's `drawQr`.
 */
export function drawQr(
  ctx: FillContext,
  qr: QrRows | null | undefined,
  x: number,
  y: number,
  size: number,
  opts: { dark?: string; light?: string; quiet?: number } = {},
): void {
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
 * The QR code as RGBA pixels, `scale` px per module, `quiet` modules of
 * white around it: for raw frames (`writeVideo`) without a canvas.
 */
export function qrToRgba(qr: QrRows, scale: number, quiet = 4): { width: number; height: number; data: Uint8Array } {
  const n = (qr.size + 2 * quiet) * scale;
  const data = new Uint8Array(n * n * 4).fill(255);
  for (let y = 0; y < qr.size; y++) {
    const row = qr.rows[y] ?? "";
    for (let x = 0; x < qr.size; x++) {
      if (row[x] !== "1") continue;
      for (let dy = 0; dy < scale; dy++) {
        const base = (((y + quiet) * scale + dy) * n + (x + quiet) * scale) * 4;
        for (let dx = 0; dx < scale; dx++) {
          const i = base + dx * 4;
          data[i] = 0;
          data[i + 1] = 0;
          data[i + 2] = 0;
        }
      }
    }
  }
  return { width: n, height: n, data };
}
