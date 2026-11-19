import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test, type TestContext } from "node:test";

import { stream, TvpipeError, type CanvasLike, type StreamOptions, type StreamSession } from "../src/index.ts";
import { fixtures, opts, sleep, until, Viewer, within } from "./helpers.ts";

/** A stream that is killed when the test ends, also when it failed. */
async function started(t: TestContext, options: Omit<StreamOptions, "binary" | "configDir">): Promise<StreamSession> {
  const s = await within(stream({ ...opts(t), ...options }), 30000, "stream to start");
  t.after(() => s.kill());
  return s;
}

/** A canvas without a canvas library: a solid color that changes per draw. */
function fakeCanvas(width: number, height: number): CanvasLike & { draws: number } {
  const data = new Uint8ClampedArray(width * height * 4);
  const canvas = {
    width,
    height,
    draws: 0,
    getContext: () => ({
      fillStyle: "",
      getImageData: (_x: number, _y: number, w: number, h: number) => ({ width: w, height: h, data: data.slice() }),
      fillRect: () => {
        canvas.draws++;
        data.fill(canvas.draws % 256);
      },
    }),
  };
  return canvas;
}

test("raw RGBA and PCM reach a viewer; end() plays out the tail", async (t) => {
  const s = await started(t, {
    to: "browser",
    video: { width: 64, height: 48, fps: 30 },
    audio: { sampleRate: 48000, channels: 2 },
  });
  assert.ok(s.playerUrl);
  const viewer = new Viewer(s.playerUrl!);
  await viewer.opened;
  const frame = new Uint8Array(64 * 48 * 4);
  const tone = new Float32Array(1600 * 2);
  for (let i = 0; i < 60; i++) {
    frame.fill(i * 4);
    assert.equal(s.writeVideo(frame), true);
    for (let j = 0; j < tone.length; j += 2) tone[j] = tone[j + 1] = Math.sin((i * 1600 + j / 2) / 10) * 0.2;
    s.writeAudio(tone);
    await sleep(33);
  }
  await until("video at the viewer", () => viewer.count(3) > 5);
  assert.ok(viewer.count(2) > 0, "audio at the viewer");
  assert.equal(s.info?.route, "builtin");
  const end = await within(s.end(), 20000, "end");
  assert.equal(end.reason, "eof");
  assert.equal(s.dropped.video, 0);
  viewer.close();
  // Wrong sizes are refused before they reach tvpipe.
  assert.throws(() => s.writeVideo(new Uint8Array(3)), (e: unknown) => e instanceof TvpipeError && e.code === "invalid-argument");
});

test("a live producer that outpaces tvpipe drops frames instead of queueing them", async (t) => {
  const s = await started(t, { to: "browser", video: { width: 640, height: 480, fps: 30 } });
  const frame = new Uint8Array(640 * 480 * 4);
  let accepted = 0;
  for (let i = 0; i < 30; i++) if (s.writeVideo(frame)) accepted++;
  assert.ok(accepted <= 3, `accepted ${accepted}`);
  assert.equal(s.dropped.video, 30 - accepted);
  await s.drain();
  assert.equal(s.writeVideo(frame), true);
  await within(s.stop(), 20000, "stop");
  // After the end nothing is written.
  assert.equal(s.writeVideo(frame), false);
});

test("a canvas, drawn at the frame rate", async (t) => {
  const s = await started(t, { to: "browser", video: { width: 64, height: 48, fps: 30 } });
  const viewer = new Viewer(s.playerUrl!);
  await viewer.opened;
  const canvas = fakeCanvas(64, 48);
  const times: number[] = [];
  const loop = s.drawLoop(canvas, (ctx, t) => {
    times.push(t);
    (ctx as unknown as { fillRect(): void }).fillRect();
  });
  await until("video at the viewer", () => viewer.count(3) > 5);
  loop.stop();
  const n = times.length;
  await sleep(100);
  assert.equal(times.length, n, "stopped");
  assert.ok(times.every((t, i) => i === 0 || t > times[i - 1]!));
  assert.throws(() => s.writeCanvas(fakeCanvas(32, 32)), /32x32/);
  await within(s.stop(), 20000, "stop");
  viewer.close();
});

/** Access units of an Annex B stream with access unit delimiters. */
function accessUnits(data: Uint8Array): { au: Uint8Array; key: boolean }[] {
  const starts: number[] = [];
  const nalAt: number[] = [];
  for (let i = 0; i + 3 < data.length; i++) {
    if (data[i] === 0 && data[i + 1] === 0 && data[i + 2] === 1) {
      const start = i > 0 && data[i - 1] === 0 ? i - 1 : i;
      nalAt.push(i + 3);
      if ((data[i + 3]! & 0x1f) === 9) starts.push(start);
      i += 2;
    }
  }
  return starts.map((s, k) => {
    const end = starts[k + 1] ?? data.length;
    const au = data.subarray(s, end);
    const key = nalAt.some((n) => n >= s && n < end && (data[n]! & 0x1f) === 5);
    return { au, key };
  });
}

test("encoded H.264 with keyframe requests", async (t) => {
  const aus = accessUnits(readFileSync(join(fixtures, "testsrc-64x48-30.h264")));
  assert.equal(aus.length, 60);
  assert.ok(aus[0]!.key && aus[15]!.key && !aus[1]!.key);
  const s = await started(t, { to: "browser", video: { width: 64, height: 48, fps: 30, codec: "h264" } });
  let keyframes = 0;
  s.on("keyframe", () => keyframes++);
  const viewer = new Viewer(s.playerUrl!);
  await viewer.opened;
  for (let i = 0; i < 90; i++) {
    const { au, key } = aus[i % aus.length]!;
    s.writeH264(au, { keyframe: key, ptsUs: Math.round((i * 1e6) / 30) });
    await sleep(33);
  }
  await until("video at the viewer", () => viewer.count(3) > 10);
  assert.equal(s.info?.route, "encoded");
  assert.ok(keyframes >= 1, "a keyframe request when the input opened");
  assert.throws(() => s.writeVideo(new Uint8Array(64 * 48 * 4)), /H\.264/);
  const end = await within(s.end(), 20000, "end");
  assert.equal(end.reason, "eof");
  viewer.close();
});

test("WebCodecs chunks", async (t) => {
  const aus = accessUnits(readFileSync(join(fixtures, "testsrc-64x48-30.h264")));
  const s = await started(t, { to: "browser", video: { width: 64, height: 48, fps: 30, codec: "h264" } });
  const viewer = new Viewer(s.playerUrl!);
  await viewer.opened;
  for (let i = 0; i < 60; i++) {
    const { au, key } = aus[i]!;
    s.writeChunk({
      type: key ? "key" : "delta",
      timestamp: Math.round((i * 1e6) / 30),
      byteLength: au.byteLength,
      copyTo: (dst) => (dst instanceof Uint8Array ? dst : new Uint8Array(dst)).set(au),
    });
    await sleep(33);
  }
  await until("video at the viewer", () => viewer.count(3) > 10);
  await within(s.stop(), 20000, "stop");
  viewer.close();
});
