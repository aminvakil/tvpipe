/**
 * Streams a moving picture and a 440 Hz tone that a program makes itself,
 * here to a browser tab (open the printed URL). Pass receivers to stream
 * to TVs instead: `node examples/canvas-tone.ts "Living Room"`.
 *
 * No canvas library: the frames are RGBA bytes drawn by hand. With
 * node-canvas, skia-canvas or @napi-rs/canvas use `writeCanvas(canvas)`
 * or `drawLoop(canvas, draw)` instead of `writeVideo`.
 *
 * Run from npm/tvpipe after `npm run build` (the example imports the
 * package by name, as an application would).
 */

import { stream } from "tvpipe";

const width = 640;
const height = 360;
const fps = 30;
const sampleRate = 48000;
const seconds = Number(process.env["SECONDS"] ?? 20);
const to = process.argv.slice(2);

const s = await stream({
  to: to.length > 0 ? to : "browser",
  video: { width, height, fps, format: "rgba" },
  audio: { sampleRate, channels: 2 },
  player: to.length > 0 ? undefined : { open: process.env["OPEN"] === "1" },
  logs: "warn",
});
if (s.playerUrl) console.log(`open ${s.playerUrl}`);
s.on("receiver", (r) => console.log(`${r.name}: ${r.state}${r.error ? ` (${r.error})` : ""}`));
s.on("viewer", (v) => console.log(`viewer ${v.tab} ${v.state}`));
s.on("stats", (st) => {
  if (st.elapsedMs % 5000 < 1000) console.log(`${(st.elapsedMs / 1000).toFixed(0)} s: ${st.videoFrames} frames`);
});
process.once("SIGINT", () => void s.stop());

const frame = new Uint8Array(width * height * 4);
/** A gradient background, a bar sweeping across and a square bouncing. */
function draw(t: number): void {
  const bar = Math.floor(((t / 2) % 1) * width);
  const sx = Math.floor((Math.sin(t * 1.3) * 0.5 + 0.5) * (width - 80));
  const sy = Math.floor((Math.cos(t * 1.7) * 0.5 + 0.5) * (height - 80));
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const i = (y * width + x) * 4;
      const inBar = Math.abs(x - bar) < 6;
      const inSquare = x >= sx && x < sx + 80 && y >= sy && y < sy + 80;
      frame[i] = inSquare ? 255 : inBar ? 255 : (x * 255) / width;
      frame[i + 1] = inSquare ? 200 : inBar ? 255 : (y * 255) / height;
      frame[i + 2] = inSquare ? 0 : inBar ? 255 : 128 + 64 * Math.sin(t);
      frame[i + 3] = 255;
    }
  }
}

// One video frame and its audio every tick, both on the stream's clock.
const samplesPerFrame = sampleRate / fps;
const audio = new Float32Array(samplesPerFrame * 2);
let sample = 0;
const start = performance.now();
for (let n = 0; s.running && n < seconds * fps; n++) {
  const pts = Math.round((n * 1e6) / fps);
  draw(n / fps);
  s.writeVideo(frame, pts);
  for (let i = 0; i < samplesPerFrame; i++, sample++) {
    // A beep: 440 Hz for 200 ms of every second.
    const on = sample % sampleRate < sampleRate / 5;
    audio[i * 2] = audio[i * 2 + 1] = on ? 0.3 * Math.sin((2 * Math.PI * 440 * sample) / sampleRate) : 0;
  }
  s.writeAudio(audio, pts);
  const due = start + ((n + 1) * 1000) / fps;
  await new Promise((r) => setTimeout(r, Math.max(0, due - performance.now())));
}
const end = await s.end();
console.log(`ended (${end.reason}); dropped ${s.dropped.video} frames under backpressure`);
