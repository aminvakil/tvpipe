/**
 * Canvases, duck-typed: anything with `width`, `height` and a 2D context
 * with `getImageData` (node-canvas, skia-canvas, @napi-rs/canvas,
 * OffscreenCanvas). No dependency on any of them.
 */

export interface ImageDataLike {
  readonly width: number;
  readonly height: number;
  readonly data: Uint8ClampedArray | Uint8Array;
}

export interface Context2DLike {
  getImageData(x: number, y: number, width: number, height: number): ImageDataLike;
}

export interface CanvasLike {
  readonly width: number;
  readonly height: number;
  getContext(type: "2d"): Context2DLike | null;
}

/** The canvas's RGBA pixels (a fresh copy, as `getImageData` makes one). */
export function canvasPixels(canvas: CanvasLike): ImageDataLike {
  const ctx = canvas.getContext("2d");
  if (!ctx || typeof ctx.getImageData !== "function") {
    throw new TypeError("the canvas has no 2D context with getImageData");
  }
  return ctx.getImageData(0, 0, canvas.width, canvas.height);
}
