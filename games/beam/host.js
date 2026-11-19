// beam's host page: a WebGL picture beamed to the TVs the launcher chose,
// as a surface (encoded here) or as a page tvpipe captures, for comparing
// the two (docs/playshare-design.md §17). URL parameters preset the
// controls and start at once: ?path=canvas&clock=raf&fps=30&size=1280x720
// &audio=1&second=same&to=browser&autostart=1.
import { Tvpipe } from "/_tvpipe/lib/index.js";
import { makeScene } from "./scene.js";

const $ = (id) => document.getElementById(id);
const params = new URLSearchParams(location.search);
for (const k of ["path", "clock", "fps", "size", "audio", "second"]) if (params.has(k)) $(k).value = params.get(k);
const state = { surfaces: [], stats: {}, codec: {}, status: {}, cast: null, running: false, errors: [] };
globalThis.beam = state;

const tvpipe = await Tvpipe.connect();
const chosen = params.has("to") ? params.get("to").split(",") : await tvpipe.selection();
const show = () => {
  $("status").textContent = JSON.stringify({ to: chosen, codec: state.codec, status: state.status, cast: state.cast, errors: state.errors }, null, 1);
  $("stats").textContent = Object.entries(state.stats).map(([k, s]) => `${k}: ${JSON.stringify(s)}`).join("\n");
};
tvpipe.on("cast", (s) => {
  state.cast = s;
  show();
});

let stopFns = [];
let audioCtx;

function beeper() {
  audioCtx ??= new AudioContext({ sampleRate: 48000 });
  const out = audioCtx.createGain();
  const tick = () => {
    const o = audioCtx.createOscillator();
    const g = audioCtx.createGain();
    o.frequency.value = 880;
    const t = audioCtx.currentTime + 0.01;
    g.gain.setValueAtTime(0.0001, t);
    g.gain.exponentialRampToValueAtTime(0.3, t + 0.01);
    g.gain.exponentialRampToValueAtTime(0.0001, t + 0.12);
    o.connect(g).connect(out);
    o.start(t);
    o.stop(t + 0.15);
  };
  const timer = setInterval(tick, 1000);
  stopFns.push(() => clearInterval(timer));
  return { context: audioCtx, node: out };
}

async function startSurface(name, canvas, clock, fps, tint, audio) {
  if (clock === "worker") {
    const info = await tvpipe.openSurface({ width: canvas.width, height: canvas.height, fps, to: chosen });
    const off = canvas.transferControlToOffscreen();
    const w = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
    w.onmessage = (ev) => {
      if (ev.data.stats) state.stats[name] = ev.data.stats;
      if (ev.data.status) {
        state.status[name] = ev.data.status.state;
        if (ev.data.status.player) state.status[`${name}Player`] = ev.data.status.player;
      }
      if (ev.data.codec) state.codec[name] = ev.data.codec;
      if (ev.data.error) state.errors.push(ev.data.error);
      show();
    };
    w.postMessage({ canvas: off, info }, [off]);
    stopFns.push(() => {
      w.terminate();
      void tvpipe.closeSurface(info.id);
    });
    return;
  }
  const scene = makeScene(canvas, { tint });
  const s = await tvpipe.surface({ canvas, fps, to: chosen, audio });
  state.codec[name] = s.codec;
  s.on("stats", (st) => {
    state.stats[name] = st;
    show();
  });
  s.on("status", (st) => {
    state.status[name] = st.state;
    if (st.player) state.status[`${name}Player`] = st.player;
    show();
  });
  s.on("error", (e) => state.errors.push(String(e)));
  let live = true;
  if (clock === "timer") {
    // A timer draws and encodes (the encoder's own clock: "timer" only encodes).
    const step = 1000 / fps;
    let next = performance.now();
    let t;
    const tick = () => {
      scene.draw();
      s.frame(true);
      next += step;
      const now = performance.now();
      if (next < now - step) next = now;
      t = setTimeout(tick, Math.max(0, next - now));
    };
    tick();
    stopFns.push(() => clearTimeout(t));
  } else {
    const loop = () => {
      if (!live) return;
      scene.draw();
      s.frame();
      requestAnimationFrame(loop);
    };
    requestAnimationFrame(loop);
  }
  stopFns.push(() => {
    live = false;
    s.close();
  });
  state.surfaces.push(s);
}

async function start() {
  if (state.running) return;
  state.running = true;
  $("start").disabled = true;
  $("stop").disabled = false;
  const [w, h] = $("size").value.split("x").map(Number);
  const fps = Number($("fps").value);
  const clock = $("clock").value;
  try {
    if ($("path").value === "tab") {
      // The same picture as a page: tvpipe's headless browser renders and captures it.
      const preview = makeScene($("c"));
      let live = true;
      const loop = () => live && (preview.draw(), requestAnimationFrame(loop));
      requestAnimationFrame(loop);
      stopFns.push(() => (live = false), () => void tvpipe.stopCast());
      state.cast = await tvpipe.cast({ page: "/tv.html", to: chosen });
    } else {
      for (const c of [$("c"), $("c2")]) [c.width, c.height] = [w, h];
      const audio = $("audio").value === "1" ? beeper() : undefined;
      await startSurface("screen1", $("c"), clock, fps, [0, 0, 0], audio);
      if ($("second").value) {
        $("c2").hidden = false;
        await startSurface("screen2", $("c2"), clock === "worker" ? "raf" : clock, fps, [3, 1.5, 0], undefined);
      }
    }
  } catch (e) {
    state.errors.push(String(e));
  }
  show();
}

function stop() {
  for (const f of stopFns.splice(0)) f();
  state.running = false;
  state.surfaces = [];
  $("start").disabled = false;
  $("stop").disabled = true;
  show();
}

$("start").onclick = () => void start();
$("stop").onclick = stop;
globalThis.beamStart = start;
globalThis.beamStop = stop;
show();
if (params.get("autostart") === "1") void start();
