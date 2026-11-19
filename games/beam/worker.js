// The worker variant: an OffscreenCanvas drawn and encoded here on a
// timer, so the window being hidden does not stop it.
import { SurfaceEncoder } from "/_tvpipe/lib/surface.js";
import { makeScene } from "./scene.js";

self.onmessage = async (ev) => {
  const { canvas, info } = ev.data;
  const scene = makeScene(canvas, { tint: [0.6, 0.3, 0] });
  const enc = new SurfaceEncoder(info, canvas);
  enc.on("stats", (s) => self.postMessage({ stats: s }));
  enc.on("status", (s) => self.postMessage({ status: s }));
  enc.on("error", (e) => self.postMessage({ error: String(e) }));
  await enc.ready;
  self.postMessage({ codec: enc.codec });
  // A frame every 1/fps, catching up on late wake-ups.
  const step = 1000 / info.fps;
  let next = performance.now();
  const tick = () => {
    scene.draw();
    enc.frame(true);
    next += step;
    const now = performance.now();
    if (next < now - step) next = now;
    setTimeout(tick, Math.max(0, next - now));
  };
  tick();
};
