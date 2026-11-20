"use strict";
// Jinx — "tactile tabletop" TV mockup. No dependencies, no network, no build step: open tv.html.
// Everything is laid out in a 1280x720 stage (the TV page's layout); the window scales it to fit.

const stage = /** @type {HTMLElement} */ (document.getElementById("stage"));
const table = /** @type {HTMLElement} */ (document.getElementById("table"));
const fx = /** @type {HTMLElement} */ (document.getElementById("fx"));
const params = new URLSearchParams(location.search);

/* =================================================================== helpers */

function h(tag, attrs, ...kids) {
  const e = document.createElement(tag);
  if (attrs) for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === "class") e.className = v;
    else if (k === "style") e.style.cssText = v;
    else if (k === "html") e.innerHTML = v;
    else e.setAttribute(k, v);
  }
  for (const k of kids.flat()) if (k != null && k !== false) e.append(k.nodeType ? k : document.createTextNode(String(k)));
  return e;
}
const rnd = (a, b) => a + Math.random() * (b - a);
const pick = (arr) => arr[Math.floor(Math.random() * arr.length)];
const EASE = {
  out: "cubic-bezier(.2,.8,.2,1)",
  in: "cubic-bezier(.55,0,.9,.35)",
  back: "cubic-bezier(.3,1.6,.5,1)",
  inout: "cubic-bezier(.65,0,.35,1)",
  lin: "linear",
};
/** Web Animations with fill: both (staggered entrances hold their first frame during the delay). */
function A(el, frames, dur, opt = {}) {
  return el.animate(frames, { duration: dur, fill: "both", easing: EASE.out, ...opt });
}
/** Every async sequence checks `gen` after each wait: changing phase cancels it. */
let gen = 0;
const wait = (ms) => { const g = gen; return new Promise((r) => setTimeout(() => r(g === gen), ms)); };
let intervals = [];
function every(ms, fn) { const id = setInterval(fn, ms); intervals.push(id); return id; }
let scale = 1;
/** An element's box in stage coordinates. */
function sp(el) {
  const r = el.getBoundingClientRect(), s = stage.getBoundingClientRect();
  return { x: (r.left - s.left) / scale, y: (r.top - s.top) / scale, w: r.width / scale, h: r.height / scale };
}
/** Shrinks a single word until it fits (clue words are up to ~15 characters). */
function fit(el, max, min, avail) {
  let s = max;
  el.style.fontSize = s + "px";
  while (el.scrollWidth > avail && s > min) { s -= 2; el.style.fontSize = s + "px"; }
  if (el.scrollWidth > avail) el.style.letterSpacing = "0";
  // Still too wide at the 24 px floor (a 15-letter word on a small card): condense, never go smaller.
  if (el.scrollWidth > avail) el.style.transform = `scaleX(${Math.max(0.74, avail / el.scrollWidth)})`;
}
function remove(el, ms) { setTimeout(() => el.remove(), ms); }

/* =================================================================== sound (Web Audio, synthesized) */

const SND = { on: params.get("sound") === "1", ctx: null, noise: null, out: null };
function ac() {
  if (!SND.on) return null;
  try {
    if (!SND.ctx) {
      const ctx = new AudioContext();
      const b = ctx.createBuffer(1, ctx.sampleRate, ctx.sampleRate);
      const d = b.getChannelData(0);
      for (let i = 0; i < d.length; i++) d[i] = Math.random() * 2 - 1;
      SND.ctx = ctx; SND.noise = b;
      const comp = ctx.createDynamicsCompressor();
      SND.out = ctx.createGain(); SND.out.gain.value = 0.7;
      SND.out.connect(comp).connect(ctx.destination);
    }
    if (SND.ctx.state === "suspended") void SND.ctx.resume();
    return SND.ctx;
  } catch { return null; }
}
function env(g, t, gain, attack, dur) {
  g.gain.setValueAtTime(0.0001, t);
  g.gain.exponentialRampToValueAtTime(gain, t + attack);
  g.gain.exponentialRampToValueAtTime(0.0001, t + dur);
}
function noise({ t = 0, dur = 0.1, type = "bandpass", f = 1000, f2, q = 1, gain = 0.4, attack = 0.003 }) {
  const ctx = ac(); if (!ctx) return;
  const t0 = ctx.currentTime + 0.01 + t;
  const s = ctx.createBufferSource(); s.buffer = SND.noise;
  const fl = ctx.createBiquadFilter(); fl.type = type; fl.Q.value = q;
  fl.frequency.setValueAtTime(f, t0);
  if (f2) fl.frequency.exponentialRampToValueAtTime(f2, t0 + dur);
  const g = ctx.createGain(); env(g, t0, gain, attack, dur);
  s.connect(fl).connect(g).connect(SND.out);
  s.start(t0, Math.random() * 0.5); s.stop(t0 + dur + 0.05);
}
function tone({ t = 0, f = 440, f2, dur = 0.2, type = "sine", gain = 0.3, attack = 0.004 }) {
  const ctx = ac(); if (!ctx) return;
  const t0 = ctx.currentTime + 0.01 + t;
  const o = ctx.createOscillator(); o.type = type;
  o.frequency.setValueAtTime(f, t0);
  if (f2) o.frequency.exponentialRampToValueAtTime(f2, t0 + dur);
  const g = ctx.createGain(); env(g, t0, gain, attack, dur);
  o.connect(g).connect(SND.out);
  o.start(t0); o.stop(t0 + dur + 0.05);
}
const sfx = {
  slide() { noise({ dur: 0.18, f: 1800, f2: 700, q: 0.8, gain: 0.22, attack: 0.03 }); },
  thwap() { noise({ dur: 0.07, type: "lowpass", f: 1400, gain: 0.55 }); tone({ f: 150, f2: 80, dur: 0.07, gain: 0.25 }); },
  whoosh() { noise({ dur: 0.28, f: 500, f2: 2600, q: 1.2, gain: 0.18, attack: 0.08 }); },
  flipTick() { noise({ dur: 0.03, type: "highpass", f: 3000, gain: 0.25 }); noise({ t: 0.07, dur: 0.03, type: "highpass", f: 3000, gain: 0.2 }); },
  thunk() { tone({ f: 240, f2: 110, dur: 0.14, type: "triangle", gain: 0.5 }); noise({ dur: 0.05, f: 800, q: 2, gain: 0.3 }); },
  slam() { tone({ f: 120, f2: 38, dur: 0.45, gain: 0.95 }); noise({ dur: 0.3, type: "lowpass", f: 500, gain: 0.9 }); noise({ dur: 0.08, type: "highpass", f: 2000, gain: 0.3 }); },
  shatter() {
    noise({ dur: 0.45, type: "highpass", f: 1400, gain: 0.45, attack: 0.002 });
    noise({ dur: 0.3, f: 2400, f2: 900, q: 3, gain: 0.3 });
    for (let i = 0; i < 7; i++) tone({ t: rnd(0.01, 0.25), f: rnd(1800, 4200), dur: 0.05, gain: 0.06, type: "triangle" });
  },
  rip() { noise({ dur: 0.38, f: 2600, f2: 500, q: 4, gain: 0.45, attack: 0.02 }); },
  stamp() { noise({ dur: 0.09, type: "lowpass", f: 1100, gain: 0.8 }); tone({ f: 190, f2: 90, dur: 0.1, gain: 0.45 }); },
  marker() { noise({ dur: 0.22, f: 3200, f2: 2400, q: 6, gain: 0.18, attack: 0.03 }); },
  clink() { const f = rnd(2400, 2900); tone({ f, dur: 0.14, gain: 0.12 }); tone({ f: f * 1.52, dur: 0.1, gain: 0.07 }); tone({ t: 0.05, f: f * 0.83, dur: 0.12, gain: 0.08 }); },
  punch() { noise({ dur: 0.025, type: "highpass", f: 2500, gain: 0.35 }); noise({ t: 0.012, dur: 0.05, f: 450, q: 2, gain: 0.45 }); },
  unpunch() { noise({ dur: 0.04, f: 900, q: 2, gain: 0.25 }); },
  snip() { noise({ dur: 0.025, type: "highpass", f: 4000, gain: 0.4 }); noise({ t: 0.06, dur: 0.03, type: "highpass", f: 3500, gain: 0.45 }); },
  tick() { tone({ f: 1900, dur: 0.025, type: "square", gain: 0.05 }); noise({ dur: 0.02, f: 3000, q: 3, gain: 0.2 }); },
  pop() { tone({ f: 380, f2: 900, dur: 0.09, gain: 0.22 }); },
  bonk() { tone({ f: 220, f2: 140, dur: 0.16, type: "triangle", gain: 0.3 }); },
  zip() { noise({ dur: 0.3, f: 700, f2: 3200, q: 2, gain: 0.12, attack: 0.05 }); },
  sparkle() { for (let i = 0; i < 9; i++) tone({ t: i * 0.045, f: rnd(1500, 3600), dur: 0.09, gain: 0.06 }); },
  fanfare() {
    [523, 659, 784, 1047].forEach((f, i) => { tone({ t: i * 0.1, f, dur: i === 3 ? 0.7 : 0.18, gain: 0.25 }); tone({ t: i * 0.1, f: f * 4, dur: 0.05, gain: 0.04 }); });
    [523, 659, 784].forEach((f) => tone({ t: 0.4, f, dur: 0.8, type: "triangle", gain: 0.12 }));
    noise({ t: 0.4, dur: 0.7, type: "highpass", f: 6000, gain: 0.12, attack: 0.01 });
  },
  trombone() {
    [[392, 370], [370, 349], [349, 330], [330, 300]].forEach(([a, b], i) => {
      const ctx = ac(); if (!ctx) return;
      const t0 = ctx.currentTime + 0.02 + i * 0.32, dur = i === 3 ? 0.9 : 0.3;
      const o = ctx.createOscillator(); o.type = "sawtooth";
      o.frequency.setValueAtTime(a, t0); o.frequency.linearRampToValueAtTime(b, t0 + dur);
      if (i === 3) { const l = ctx.createOscillator(); const lg = ctx.createGain(); l.frequency.value = 6; lg.gain.value = 9; l.connect(lg).connect(o.frequency); l.start(t0); l.stop(t0 + dur); }
      const fl = ctx.createBiquadFilter(); fl.type = "lowpass"; fl.frequency.value = 1100;
      const g = ctx.createGain(); env(g, t0, 0.16, 0.03, dur);
      o.connect(fl).connect(g).connect(SND.out); o.start(t0); o.stop(t0 + dur + 0.05);
    });
  },
  drumroll(ms) {
    const n = Math.floor(ms / 45);
    for (let i = 0; i < n; i++) noise({ t: i * 0.045, dur: 0.05, f: 260, q: 1.5, gain: 0.12 + 0.3 * (i / n) * Math.random() + 0.1 * (i / n) });
  },
  cymbal() { noise({ dur: 0.9, type: "highpass", f: 5500, gain: 0.2, attack: 0.005 }); },
};

/* =================================================================== fake data */

const PLAYERS = {
  anna: { n: "Anna", c: "#ff7a5c", face: ["dot", "smile", "blush"] },
  ben: { n: "Ben", c: "#4aa8f5", face: ["dot", "flat", "glasses"] },
  chiara: { n: "Chiara", c: "#8bd648", face: ["happy", "open", "tuft"] },
  dev: { n: "Dev", c: "#ffc63d", face: ["dot", "flat", "stache"] },
  emeka: { n: "Emeka", c: "#b48fff", face: ["sleepy", "o", "brows"] },
  fatima: { n: "Fatima", c: "#ff8cc6", face: ["wink", "smile", "bow"] },
  gus: { n: "Gus", c: "#ff9845", face: ["wide", "grin", "brows"] },
  hiroko: { n: "Hiroko", c: "#3fd9c7", face: ["happy", "smile", "swoop"] },
};
const ORDER = ["anna", "ben", "chiara", "dev", "emeka", "fatima", "gus", "hiroko"];
const GIVERS = ORDER.slice(0, 7);
const GUESSER = "hiroko";
const SECRET = "Volcano";
const CLUES = [
  { by: "anna", word: "Lava", bold: true },
  { by: "ben", word: "Eruption" },
  { by: "chiara", word: "lava" },
  { by: "dev", word: "Mountain" },
  { by: "emeka", word: "Volcanic" },
  { by: "fatima", word: "Pompeii", bold: true },
  { by: "gus", word: "LAVA" },
];
if (params.get("long") === "1") { CLUES[1].word = "Watermelonseeds"; CLUES[3].word = "Congratulations"; } // ?long=1: 15-letter clues
const SURVIVORS = [1, 3, 5]; // Ben, Dev, Fatima
const PICKS = [1, 2]; // indexes into SURVIVORS: Dev first, then Fatima
// Rounds 1..10: before this round 2 found, 1 missed.
const TALLY = ["won", "lost", "won", "todo", "todo", "todo", "todo", "todo", "todo", "todo"];
let wrongPath = params.get("wrong") === "1";

/* =================================================================== drawn things */

const INK = "#1d1a2b";
function faceSvg(f) {
  const st = (w = 6) => `fill="none" stroke="${INK}" stroke-width="${w}" stroke-linecap="round" stroke-linejoin="round"`;
  let s = "";
  switch (f[0]) {
    case "dot": s += `<circle cx="36" cy="44" r="6.5" fill="${INK}"/><circle cx="64" cy="44" r="6.5" fill="${INK}"/>`; break;
    case "happy": s += `<path d="M27 47 Q36 35 45 47 M55 47 Q64 35 73 47" ${st()}/>`; break;
    case "wide": s += `<circle cx="35" cy="42" r="11" fill="#fff" stroke="${INK}" stroke-width="5"/><circle cx="65" cy="42" r="11" fill="#fff" stroke="${INK}" stroke-width="5"/><circle cx="37" cy="44" r="5" fill="${INK}"/><circle cx="67" cy="44" r="5" fill="${INK}"/>`; break;
    case "sleepy": s += `<path d="M27 45 Q36 52 45 45 M55 45 Q64 52 73 45" ${st()}/>`; break;
    case "wink": s += `<circle cx="36" cy="44" r="6.5" fill="${INK}"/><path d="M55 47 Q64 37 73 47" ${st()}/>`; break;
  }
  switch (f[1]) {
    case "smile": s += `<path d="M34 63 Q50 78 66 63" ${st()}/>`; break;
    case "open": s += `<path d="M35 60 Q50 88 65 60 Z" fill="${INK}" stroke="${INK}" stroke-width="4" stroke-linejoin="round"/>`; break;
    case "flat": s += `<path d="M38 70 L62 70" ${st()}/>`; break;
    case "o": s += `<circle cx="50" cy="68" r="7" ${st(5)}/>`; break;
    case "grin": s += `<path d="M31 61 L69 61 Q50 88 31 61 Z" fill="#fff" stroke="${INK}" stroke-width="5" stroke-linejoin="round"/>`; break;
  }
  switch (f[2]) {
    case "blush": s += `<circle cx="23" cy="58" r="7" fill="#ff8a7a"/><circle cx="77" cy="58" r="7" fill="#ff8a7a"/>`; break;
    case "glasses": s += `<circle cx="35" cy="44" r="15" ${st(4)}/><circle cx="65" cy="44" r="15" ${st(4)}/><path d="M48 44 L52 44" ${st(4)}/>`; break;
    case "stache": s += `<path d="M31 59 Q41 50 50 57 Q59 50 69 59 Q59 66 50 61 Q41 66 31 59 Z" fill="${INK}"/>`; break;
    case "tuft": s += `<path d="M40 20 Q46 4 51 17 Q56 5 62 19" ${st(5)}/>`; break;
    case "brows": s += `<path d="M25 27 L44 33 M75 27 L56 33" ${st(5)}/>`; break;
    case "bow": s += `<path d="M68 16 L82 7 L83 25 Z M68 16 L56 5 L54 24 Z" fill="#df4a3c" stroke="${INK}" stroke-width="3" stroke-linejoin="round"/><circle cx="68" cy="16" r="4" fill="${INK}"/>`; break;
    case "swoop": s += `<path d="M22 32 Q38 12 72 18" ${st(6)}/>`; break;
  }
  return `<svg viewBox="0 0 100 100">${s}</svg>`;
}
function token(id, size, cls = "", style = "") {
  const p = PLAYERS[id];
  return h("div", { class: `token ${cls}`, style: `--s:${size}px;--c:${p.c};${style}`, html: faceSvg(p.face) });
}
const FLAME = `<svg viewBox="0 0 40 40"><path d="M20 3 C22 11 31 14 31 25 A11 11 0 0 1 9 25 C9 18 14 15 15 9 C17 13 18 15 20 17 C21 12 20 7 20 3 Z" fill="#df4a3c" stroke="${INK}" stroke-width="2.5" stroke-linejoin="round"/><path d="M20 21 C22 25 25 26 25 29.5 A5 5 0 0 1 15 29.5 C15 26.5 18 25 20 21 Z" fill="#ffe066"/></svg>`;
const flame = () => h("div", { class: "flame", html: FLAME });
const PENCIL = `<svg viewBox="0 0 40 40"><g transform="rotate(40 20 20)"><rect x="15" y="1" width="10" height="27" rx="2" fill="#f6b93b" stroke="${INK}" stroke-width="3"/><path d="M15 28 L20 39 L25 28 Z" fill="#fbf1dc" stroke="${INK}" stroke-width="3" stroke-linejoin="round"/></g></svg>`;

function makeCard(c, o) {
  const w = o.w ?? 210, hh = o.h ?? 150;
  const bold = c.bold && o.showBold !== false;
  const el = h("div", { class: `card${o.down ? " down" : ""}${bold ? " bold" : ""}`, style: `left:${o.x}px;top:${o.y}px;width:${w}px;height:${hh}px;${o.style ?? ""}` });
  const word = h("div", { class: "word" }, c.word ?? "");
  const front = h("div", { class: "face front" }, h("div", { class: "hl", style: "transform:scaleX(0)" }), word);
  const back = h("div", { class: "face back" }, h("div", { class: "emblem" }, h("span", null, "J")));
  if (!o.bare) { front.append(token(c.by, 50, "sticker")); back.append(token(c.by, 54, "sticker")); }
  if (bold) { front.append(flame()); back.append(flame()); }
  el.append(h("div", { class: "flip" }, front, back));
  if (!o.bare && o.who !== false) el.append(h("div", { class: "who" }, PLAYERS[c.by].n));
  (o.parent ?? table).append(el);
  fit(word, o.font ?? 40, 24, w - 14);
  return el;
}

function fakeQr(n = 29) {
  let seed = 1234567;
  const r = () => (seed = (seed * 16807) % 2147483647) / 2147483647;
  const finder = (x, y, fx0, fy0) => {
    const dx = x - fx0, dy = y - fy0;
    if (dx < -1 || dy < -1 || dx > 7 || dy > 7) return null;
    if (dx === -1 || dy === -1 || dx === 7 || dy === 7) return false;
    if (dx === 0 || dy === 0 || dx === 6 || dy === 6) return true;
    return dx >= 2 && dx <= 4 && dy >= 2 && dy <= 4;
  };
  let d = "";
  for (let y = 0; y < n; y++) for (let x = 0; x < n; x++) {
    let on = finder(x, y, 0, 0) ?? finder(x, y, n - 7, 0) ?? finder(x, y, 0, n - 7);
    if (on === null) on = x === 6 || y === 6 ? (x + y) % 2 === 0 : r() < 0.48;
    if (on) d += `M${x} ${y}h1v1h-1z`;
  }
  return `<svg viewBox="-2 -2 ${n + 4} ${n + 4}" shape-rendering="crispEdges"><rect x="-2" y="-2" width="${n + 4}" height="${n + 4}" fill="#fff"/><path d="${d}" fill="${INK}"/></svg>`;
}

/* =================================================================== shared table pieces */

function mat() { table.append(h("div", { class: "mat" })); }
function header({ tab, guesser = false, tally = TALLY }) {
  if (tab) table.append(h("div", { class: "tab paper" }, tab));
  if (guesser) table.append(h("div", { class: "who-guesses" }, token(GUESSER, 52, "", "position:relative;flex:none"), `${PLAYERS[GUESSER].n} guesses`));
  if (tally) {
    const won = tally.filter((t) => t === "won").length, played = tally.filter((t) => t !== "todo").length;
    const el = h("div", { class: "tally" }, h("span", { class: "tl" }, `${won} of ${played}`), ...tally.map((t) => h("i", { class: `mini ${t}`, style: "display:block;width:24px;height:34px" })));
    el.style.gap = "7px";
    el.firstChild.style.marginRight = "8px";
    table.append(el);
  }
}
/** A pile of round chips: the pot. */
function makePot(n) {
  const el = h("div", { class: "pot" }, h("div", { class: "base" }), h("div", { class: "chips" }), h("div", { class: "label paper" }, h("small", null, "THE POT"), h("b", null, String(n))));
  table.append(el);
  const pot = { el, n, chips: el.querySelector(".chips"), num: el.querySelector("b") };
  for (let i = 0; i < n; i++) pot.chips.append(h("div", { class: "chip", style: `bottom:${i * 15}px;transform:translateX(${((i * 37) % 9) - 4}px)` }));
  return pot;
}
function potCount(pot, n) {
  pot.n = n; pot.num.textContent = String(n);
  A(pot.num, [{ transform: "scale(1.35)", color: "#df4a3c" }, { transform: "scale(1)", color: INK }], 300);
}
/** Chips leave the pot: off the table ("away") or to a token ({x, y}). */
async function drain(pot, k, to = "away", gap = 170) {
  for (let i = 0; i < k && pot.chips.lastChild; i++) {
    const chip = /** @type {HTMLElement} */ (pot.chips.lastChild);
    const p = sp(chip);
    chip.remove();
    const c2 = h("div", { class: "chip", style: `left:${p.x}px;top:${p.y}px` });
    fx.append(c2);
    if (to === "away") {
      const dir = i % 2 ? 1 : -1;
      A(c2, [
        { transform: "none", opacity: 1 },
        { offset: 0.3, transform: `translate(${dir * 50}px,-46px) rotate(${dir * 25}deg)`, opacity: 1, easing: EASE.in },
        { transform: `translate(${dir * 170}px,330px) rotate(${dir * 200}deg)`, opacity: 0 },
      ], 700, { easing: EASE.out });
      remove(c2, 720);
    } else {
      A(c2, [
        { transform: "none" },
        { offset: 0.4, transform: `translate(${(to.x - p.x - 65) * 0.4}px,${(to.y - p.y) * 0.4 - 90}px) scale(1.1)`, easing: EASE.in },
        { transform: `translate(${to.x - p.x - 65}px,${to.y - p.y - 13}px) scale(.5)`, opacity: 0.2 },
      ], 520, { easing: EASE.out });
      remove(c2, 540);
      setTimeout(sfx.clink, 480);
    }
    if (to === "away") sfx.clink();
    potCount(pot, pot.n - 1);
    if (!(await wait(gap))) return false;
  }
  return true;
}
function shake(px = 8, ms = 320) {
  A(table, [
    { transform: "translate(0,0)" }, { transform: `translate(${-px}px,${px * 0.6}px)` }, { transform: `translate(${px * 0.8}px,${-px * 0.5}px)` },
    { transform: `translate(${-px * 0.4}px,${px * 0.3}px)` }, { transform: "translate(0,0)" },
  ], ms, { easing: EASE.lin, fill: "none" });
}
function dust(x, y, n = 8, spread = 90, color = "var(--paper)") {
  for (let i = 0; i < n; i++) {
    const a = (i / n) * Math.PI * 2 + rnd(-0.3, 0.3), d = rnd(spread * 0.6, spread);
    const p = h("div", { class: "dust", style: `left:${x - 17}px;top:${y - 17}px;background:${color}` });
    fx.append(p);
    A(p, [{ transform: "scale(.6)", opacity: 0.9 }, { transform: `translate(${Math.cos(a) * d}px,${Math.sin(a) * d * 0.5}px) scale(${rnd(0.15, 0.4)})`, opacity: 0 }], rnd(380, 520));
    remove(p, 540);
  }
}
const CONFETTI = ["#ff7a5c", "#4aa8f5", "#8bd648", "#ffc63d", "#b48fff", "#ff8cc6", "#fbf1dc", "#3fd9c7"];
function confetti(x, y, n = 44, up = 1) {
  for (let i = 0; i < n; i++) {
    const p = h("div", { class: `confetti${i % 3 === 0 ? " sq" : ""}`, style: `left:${x}px;top:${y}px;background:${CONFETTI[i % CONFETTI.length]}` });
    fx.append(p);
    const a = rnd(-Math.PI * 0.95, -Math.PI * 0.05), v = rnd(220, 520) * up;
    const dx = Math.cos(a) * v, dy = Math.sin(a) * v;
    const dur = rnd(1300, 2000);
    A(p, [
      { transform: "translate(0,0) rotate(0)", opacity: 1 },
      { offset: 0.35, transform: `translate(${dx}px,${dy}px) rotate(${rnd(-200, 200)}deg)`, opacity: 1, easing: "cubic-bezier(.4,0,1,1)" },
      { transform: `translate(${dx * 1.3}px,${dy + 760}px) rotate(${rnd(-600, 600)}deg)`, opacity: 1 },
    ], dur, { easing: "cubic-bezier(.1,.7,.4,1)" });
    remove(p, dur + 20);
  }
}
function floatText(x, y, text, cls = "plus", dur = 1600, parent = fx) {
  const f = h("div", { class: `float ${cls}`, style: `left:${x}px;top:${y}px` }, text);
  parent.append(f);
  A(f, [
    { transform: "translate(-50%,10px) scale(.6)", opacity: 0 },
    { offset: 0.15, transform: "translate(-50%,0) scale(1.1)", opacity: 1 },
    { offset: 0.75, transform: "translate(-50%,-40px) scale(1)", opacity: 1 },
    { transform: "translate(-50%,-60px) scale(1)", opacity: 0 },
  ], dur, { easing: EASE.lin });
  remove(f, dur + 20);
}
/** A rubber stamp slammed onto the table (centered at x, y). */
function stampAt(x, y, text, cls = "", rot = -8, size = 64) {
  const s = h("div", { class: `stamp ${cls}`, style: `left:${x}px;top:${y}px;font-size:${size}px;z-index:40` }, text);
  table.append(s);
  A(s, [
    { transform: `translate(-50%,-50%) scale(2.8) rotate(${rot - 16}deg)`, opacity: 0 },
    { offset: 0.65, transform: `translate(-50%,-50%) scale(.9) rotate(${rot}deg)`, opacity: 1 },
    { transform: `translate(-50%,-50%) scale(1) rotate(${rot}deg)`, opacity: 1 },
  ], 260, { easing: EASE.in });
  setTimeout(() => { sfx.stamp(); shake(Math.min(8, size / 10)); }, 170);
  return s;
}
/** Masking tape with a reason, wiped on left to right (centered at x). */
function tapeAt(x, y, text, rot = 2) {
  const wrap = h("div", { class: "abs", style: `left:${x}px;top:${y}px;z-index:41` });
  const t = h("div", { class: "tape", style: "position:relative" }, text);
  wrap.append(t);
  table.append(wrap);
  const w = t.offsetWidth;
  const left = Math.max(64, Math.min(1216 - w, x - w / 2));
  wrap.style.left = left + "px";
  wrap.style.transform = `rotate(${rot}deg)`;
  A(wrap, [{ clipPath: "inset(-10px 100% -10px -10px)" }, { clipPath: "inset(-10px -10px -10px -10px)" }], 320, { easing: EASE.inout });
  sfx.zip();
  return wrap;
}
function fadeOut(el, ms = 300) { A(el, [{ opacity: 1 }, { opacity: 0 }], ms); remove(el, ms + 20); }

/** Breaks a card into paper shards that fly apart (cx, cy: the card's centre on the stage). */
function shatter(card, cx, cy, rot, w, hh, bold) {
  const back = card.querySelector(".back");
  const c = { x: w / 2 + rnd(-20, 20), y: hh / 2 + rnd(-14, 14) };
  const per = 2 * (w + hh);
  const ts = [0, w, w + hh, 2 * w + hh];
  for (let i = 0; i < 7; i++) ts.push(rnd(0, per));
  ts.sort((a, b) => a - b);
  const at = (t) => (t <= w ? [t, 0] : t <= w + hh ? [w, t - w] : t <= 2 * w + hh ? [w - (t - w - hh), hh] : [0, hh - (t - 2 * w - hh)]);
  for (let i = 0; i < ts.length; i++) {
    const p1 = at(ts[i]), p2 = at(ts[(i + 1) % ts.length]);
    if (Math.hypot(p1[0] - p2[0], p1[1] - p2[1]) < 2) continue;
    const poly = [[c.x, c.y], p1];
    poly.push(p2);
    const piece = h("div", { class: `card shard${bold ? " bold" : ""}`, style: `left:${cx - w / 2}px;top:${cy - hh / 2}px;width:${w}px;height:${hh}px;clip-path:polygon(${poly.map((p) => `${p[0]}px ${p[1]}px`).join(",")})` });
    const face = back.cloneNode(true);
    face.style.transform = "none";
    piece.append(face);
    fx.append(piece);
    const gx = (c.x + p1[0] + p2[0]) / 3, gy = (c.y + p1[1] + p2[1]) / 3;
    let dx = gx - w / 2, dy = gy - hh / 2;
    const len = Math.hypot(dx, dy) || 1;
    const d = rnd(170, 340);
    dx = (dx / len) * d; dy = (dy / len) * d * 0.8;
    const spin = rnd(-320, 320);
    piece.style.transformOrigin = `${gx}px ${gy}px`;
    const dur = rnd(650, 950);
    A(piece, [
      { transform: `rotate(${rot}deg)`, opacity: 1 },
      { offset: 0.45, transform: `translate(${dx * 0.7}px,${dy * 0.7 - 50}px) rotate(${rot + spin * 0.5}deg) scale(.95)`, opacity: 1, easing: "cubic-bezier(.5,0,1,1)" },
      { transform: `translate(${dx}px,${dy + 160}px) rotate(${rot + spin}deg) scale(.8)`, opacity: 0 },
    ], dur, { easing: "cubic-bezier(.15,.75,.35,1)" });
    remove(piece, dur + 20);
  }
}
/** Two halves of a torn card drift apart and drop off the table. */
function tear(card) {
  const r = { x: parseFloat(card.style.left), y: parseFloat(card.style.top), w: card.offsetWidth, h: card.offsetHeight };
  const zig = [];
  for (let i = 0; i <= 8; i++) zig.push([r.w / 2 + (i % 2 ? 12 : -10) + rnd(-5, 5), (i / 8) * r.h]);
  const halves = [
    [[0, 0], ...zig, [0, r.h]],
    [[r.w, 0], ...zig, [r.w, r.h]],
  ];
  halves.forEach((poly, k) => {
    const piece = h("div", { class: "card shard", style: `left:${r.x}px;top:${r.y}px;width:${r.w}px;height:${r.h}px;clip-path:polygon(${poly.map((p) => `${p[0]}px ${p[1]}px`).join(",")})` });
    const face = card.querySelector(".back").cloneNode(true);
    face.style.transform = "none";
    piece.append(face);
    const strike = card.querySelector(".strike");
    if (strike) piece.append(strike.cloneNode(true));
    fx.append(piece);
    const dir = k ? 1 : -1;
    A(piece, [
      { transform: "translateY(-10px) scale(1.06)", opacity: 1 },
      { offset: 0.35, transform: `translate(${dir * 26}px,-6px) rotate(${dir * 7}deg) scale(1.04)`, opacity: 1, easing: EASE.in },
      { transform: `translate(${dir * 90}px,180px) rotate(${dir * 24}deg) scale(.9)`, opacity: 0 },
    ], 820);
    remove(piece, 840);
  });
  card.remove();
}
/** A thick marker X across an element (drawn stroke by stroke). */
function drawX(card, w, hh) {
  const svg = h("div", {
    class: "strike", style: "position:absolute;inset:-8px;z-index:4", html: `<svg viewBox="0 0 ${w + 16} ${hh + 16}" width="${w + 16}" height="${hh + 16}" style="overflow:visible">
    <path class="a" d="M20 18 Q ${w / 2} ${hh / 2 - 6} ${w - 4} ${hh - 2}" stroke="#fbf1dc" stroke-width="26" fill="none" stroke-linecap="round"/>
    <path class="a" d="M20 18 Q ${w / 2} ${hh / 2 - 6} ${w - 4} ${hh - 2}" stroke="${INK}" stroke-width="15" fill="none" stroke-linecap="round"/>
    <path class="b" d="M${w - 6} 16 Q ${w / 2 + 6} ${hh / 2} 18 ${hh}" stroke="#fbf1dc" stroke-width="26" fill="none" stroke-linecap="round"/>
    <path class="b" d="M${w - 6} 16 Q ${w / 2 + 6} ${hh / 2} 18 ${hh}" stroke="${INK}" stroke-width="15" fill="none" stroke-linecap="round"/></svg>`,
  });
  card.append(svg);
  const L = Math.hypot(w, hh) + 40;
  svg.querySelectorAll("path").forEach((p) => {
    p.style.strokeDasharray = String(L);
    A(p, [{ strokeDashoffset: L }, { strokeDashoffset: 0 }], 230, { delay: p.classList.contains("b") ? 300 : 0, easing: EASE.inout });
  });
  sfx.marker(); setTimeout(sfx.marker, 300);
  return svg;
}
/** One marker line through a word (it stays readable). */
function strikeThrough(el) {
  const w = el.offsetWidth, hh = el.offsetHeight;
  const svg = h("div", { style: "position:absolute;left:0;top:0;z-index:4", html: `<svg width="${w}" height="${hh}" style="overflow:visible"><path d="M14 ${hh * 0.62} Q ${w / 2} ${hh * 0.42} ${w - 14} ${hh * 0.5}" stroke="#fbf1dc" stroke-width="13" fill="none" stroke-linecap="round"/><path d="M14 ${hh * 0.62} Q ${w / 2} ${hh * 0.42} ${w - 14} ${hh * 0.5}" stroke="${INK}" stroke-width="6" fill="none" stroke-linecap="round"/></svg>` });
  el.append(svg);
  svg.querySelectorAll("path").forEach((p) => { p.style.strokeDasharray = String(w + 40); A(p, [{ strokeDashoffset: w + 40 }, { strokeDashoffset: 0 }], 300, { easing: EASE.inout }); });
  sfx.marker();
}
function envelope(x, y) {
  const el = h("div", { class: "envelope", style: `left:${x}px;top:${y}px` },
    h("div", { class: "body" }),
    h("div", { class: "pocket" }),
    h("div", { class: "flapwrap" }, h("div", { class: "flap" })),
    h("div", { class: "seal" }, "?"));
  table.append(el);
  return el;
}
function dial(x, y) {
  let ticks = "";
  for (let i = 0; i < 12; i++) { const a = (i / 12) * Math.PI * 2; ticks += `<path d="M${Math.sin(a) * 64} ${-Math.cos(a) * 64} L${Math.sin(a) * 75} ${-Math.cos(a) * 75}" stroke="${INK}" stroke-width="${i % 3 ? 3 : 6}" stroke-linecap="round"/>`; }
  const el = h("div", { class: "dial", style: `left:${x}px;top:${y}px`, html: `<svg viewBox="-85 -85 170 170"><circle r="80" fill="#fbf1dc" stroke="${INK}" stroke-width="6"/><path class="wedge"/>${ticks}<circle r="38" fill="${INK}"/><circle r="38" fill="none" stroke="#fbf1dc" stroke-width="3" stroke-dasharray="4 6"/></svg><div class="num"></div>` });
  table.append(el);
  const wedge = el.querySelector(".wedge"), num = el.querySelector(".num");
  return {
    el,
    set(s, total) {
      const f = Math.max(0, Math.min(1, s / total)), a = f * Math.PI * 2, r = 60;
      const d = f >= 0.999 ? `M0 -${r} A${r} ${r} 0 1 1 -0.01 -${r} Z` : f <= 0 ? "" : `M0 0 L0 -${r} A${r} ${r} 0 ${f > 0.5 ? 1 : 0} 1 ${Math.sin(a) * r} ${-Math.cos(a) * r} Z`;
      wedge.setAttribute("d", d);
      wedge.setAttribute("fill", s <= 10 ? "#df4a3c" : "#f6b93b");
      num.textContent = String(s);
    },
  };
}
function rosette() {
  const pts = [];
  for (let i = 0; i < 32; i++) { const a = (i / 32) * Math.PI * 2, r = i % 2 ? 43 : 50; pts.push(`${50 + Math.sin(a) * r}% ${50 - Math.cos(a) * r}%`); }
  return h("div", { class: "rosette", style: "position:absolute;width:124px;height:160px;z-index:30" },
    h("div", { style: "position:absolute;left:22px;top:72px;width:80px;height:88px" },
      h("i", { style: "position:absolute;left:0;top:0;width:36px;height:88px;background:#df4a3c;transform:rotate(14deg);clip-path:polygon(0 0,100% 0,100% 100%,50% 80%,0 100%);box-shadow:inset 0 0 0 3px #1d1a2b" }),
      h("i", { style: "position:absolute;right:0;top:0;width:36px;height:88px;background:#df4a3c;transform:rotate(-14deg);clip-path:polygon(0 0,100% 0,100% 100%,50% 80%,0 100%)" })),
    h("div", { style: `position:absolute;left:0;top:0;width:124px;height:124px;background:#1d1a2b;clip-path:polygon(${pts.join(",")})` }),
    h("div", { style: `position:absolute;left:5px;top:5px;width:114px;height:114px;background:#f6b93b;clip-path:polygon(${pts.join(",")})` }),
    h("div", { style: "position:absolute;left:24px;top:24px;width:76px;height:76px;border-radius:50%;background:#fbf1dc;box-shadow:0 0 0 4px #1d1a2b;display:grid;place-items:center;font-weight:700;font-size:30px;color:#1d1a2b" }, "MVP"));
}

/* =================================================================== phases */

/* ---------- 1. lobby */
const seatPos = (i) => ({ x: 588 + (i % 4) * 157, y: 158 + Math.floor(i / 4) * 214 });
let lobbyJoined = 0;
function lobby() {
  mat();
  const logo = h("div", { class: "logo", style: "left:64px;top:50px" });
  [["J", 8, -4], ["I", 1, 3], ["N", 1, -2], ["X", 8, 4]].forEach(([l, n, r]) => logo.append(h("div", { class: "tile", style: `transform:rotate(${r}deg)` }, l, h("sub", null, n))));
  table.append(logo);
  [...logo.children].forEach((t, i) => A(t, [{ transform: "translateY(-200px) rotate(30deg)", opacity: 0 }, { transform: t.style.transform, opacity: 1 }], 420, { delay: 100 + i * 110, easing: EASE.back }));
  [0, 1, 2, 3].forEach((i) => setTimeout(sfx.thunk, 240 + i * 110));
  table.append(h("div", { class: "tagline", style: "left:66px;top:176px" }, "One word each. Don't think alike."));
  const qr = h("div", { class: "qrcard paper", style: "left:96px;top:236px;transform:rotate(-2deg)" },
    h("div", { html: fakeQr() }),
    h("div", { class: "roomlbl" }, "ROOM CODE"),
    h("div", { class: "code" }, "KQZM"));
  table.append(qr);
  qr.firstChild.firstChild.style.cssText = "width:256px;height:256px;display:block";
  table.append(h("div", { class: "tapebit", style: "left:70px;top:226px;transform:rotate(-38deg)" }));
  table.append(h("div", { class: "tapebit", style: "left:342px;top:226px;transform:rotate(34deg)" }));
  table.append(h("div", { class: "url", style: "left:96px;top:640px" }, "or open tvp.link and type KQZM"));
  table.append(h("div", { class: "abs", style: "left:588px;top:52px;font-size:32px;font-weight:700;letter-spacing:.12em" }, "AT THE TABLE ", h("span", { id: "count", style: "color:#f6b93b" }, "0 / 8")));
  table.append(h("div", { class: "abs", style: "left:588px;top:92px;font-size:24px;color:#a8d3cd;font-weight:600" }, "3 to 8 players · draw your face on your phone"));
  for (let i = 0; i < 8; i++) {
    const p = seatPos(i);
    table.append(h("div", { class: "seat empty", id: `seat${i}`, style: `left:${p.x}px;top:${p.y}px` },
      h("div", { class: "token ghost", style: "--s:100px;left:25px;top:0" }),
      h("div", { class: "placecard" }, "open seat")));
  }
  const slip = h("div", { class: "banner paper", style: "left:900px;top:600px;transform:translateX(-50%) rotate(-1deg)" }, h("b", null, "Anna"), " starts the game from her phone");
  table.append(slip);
  lobbyJoined = 0;
  (async () => {
    if (!(await wait(900))) return;
    for (let i = 0; i < 5; i++) { joinNext(); if (!(await wait(650))) return; }
  })();
}
function joinNext() {
  if (lobbyJoined >= 8) return;
  const i = lobbyJoined++;
  const id = ORDER[i];
  const seat = document.getElementById(`seat${i}`);
  if (!seat) return;
  seat.classList.remove("empty");
  seat.innerHTML = "";
  const t = token(id, 100, "", "left:25px;top:0");
  if (i === 0) t.append(h("div", { class: "crown" }));
  const pc = h("div", { class: "placecard paper" }, PLAYERS[id].n);
  seat.append(t, pc);
  A(t, [
    { transform: "translateY(-120px) scale(1.6)", opacity: 0, boxShadow: "0 0 0 3px #1d1a2b, 0 60px 40px 0 rgba(0,0,0,.2)" },
    { offset: 0.6, transform: "translateY(0) scale(1)", opacity: 1, boxShadow: "0 0 0 3px #1d1a2b, 0 6px 0 3px rgba(0,0,0,.28)", easing: EASE.out },
    { offset: 0.78, transform: "translateY(0) scale(1.1,.88)" },
    { transform: "translateY(0) scale(1)", boxShadow: "0 0 0 3px #1d1a2b, 0 6px 0 3px rgba(0,0,0,.28)" },
  ], 440, { easing: EASE.in });
  A(pc, [{ transform: "perspective(300px) rotateX(-90deg)", opacity: 0 }, { transform: "perspective(300px) rotateX(0)", opacity: 1 }], 320, { delay: 300, easing: EASE.back });
  setTimeout(() => { sfx.thunk(); const p = seatPos(i); dust(p.x + 75, p.y + 96, 6, 70); }, 260);
  setTimeout(sfx.flipTick, 380);
  const c = document.getElementById("count");
  if (c) { c.textContent = `${lobbyJoined} / 8`; A(c, [{ transform: "scale(1.4)" }, { transform: "scale(1)" }], 300, { delay: 260 }); c.style.display = "inline-block"; }
}

/* ---------- 2. writing */
let wr = null;
const giverX = (i) => 640 + (i - 3) * 150;
function writing() {
  mat();
  header({ tab: "Round 4 of 10" });
  const g = token(GUESSER, 110, "", "left:323px;top:104px");
  g.append(h("div", { class: "blindfold" }));
  table.append(g);
  table.append(h("div", { class: "abs", style: "left:457px;top:114px;font-size:38px;font-weight:700" }, `${PLAYERS[GUESSER].n} guesses`));
  table.append(h("div", { class: "abs", style: "left:459px;top:164px;font-size:27px;font-weight:600;color:#a8d3cd" }, "Everyone else: ONE word, on your phone"));
  envelope(190, 272);
  table.append(h("div", { class: "zone", style: "left:560px;top:268px;width:300px;height:196px" }));
  const count = h("div", { class: "paper abs", style: "left:710px;top:444px;transform:translateX(-50%);padding:4px 18px;font-weight:700;font-size:26px;white-space:nowrap;z-index:30" }, "0 of 7 in");
  table.append(count);
  const d = dial(960, 280);
  const tokens = GIVERS.map((id, i) => {
    const t = token(id, 84, "", `left:${giverX(i) - 42}px;top:500px`);
    const pen = h("div", { class: "pencil", html: PENCIL });
    t.append(pen);
    A(pen, [{ transform: "rotate(-14deg) translateY(0)" }, { transform: "rotate(10deg) translateY(-4px)" }], rnd(260, 340), { iterations: Infinity, direction: "alternate", easing: "ease-in-out" });
    table.append(t);
    table.append(h("div", { class: "seatname", style: `left:${giverX(i) - 75}px;top:596px` }, PLAYERS[id].n));
    return t;
  });
  wr = { d, tokens, count, done: new Set(), left: 40, n: 0 };
  d.set(40, 40);
  const schedule = { 35: "anna", 32: "dev", 28: "ben", 25: "fatima", 21: "chiara", 16: "emeka", 8: "gus" };
  const g0 = gen;
  every(500, () => {
    if (g0 !== gen || !wr || wr.n >= 7 || wr.left <= 0) return;
    wr.left--;
    d.set(wr.left, 40);
    if (wr.left <= 10) { sfx.tick(); A(d.el, [{ transform: "rotate(-5deg)" }, { transform: "rotate(5deg)" }, { transform: "rotate(0)" }], 160, { fill: "none" }); }
    const who = schedule[wr.left];
    if (who) submit(GIVERS.indexOf(who));
  });
}
function submit(i) {
  if (!wr) return;
  if (i == null) i = GIVERS.findIndex((_, k) => !wr.done.has(k));
  if (i < 0 || wr.done.has(i)) return;
  wr.done.add(i);
  wr.n++;
  const fxp = 710 + rnd(-26, 26), fyp = 360 + rnd(-16, 12), rot = rnd(-16, 16);
  const c = makeCard({ by: GIVERS[i] }, { down: true, bare: true, x: fxp - 75, y: fyp - 51, w: 150, h: 102, style: `z-index:${wr.n + 1}` });
  const dx = giverX(i) - fxp, dy = 542 - fyp;
  A(c, [
    { transform: `translate(${dx}px,${dy}px) rotate(${rot - 50}deg) scale(.45)`, opacity: 0 },
    { offset: 0.15, opacity: 1 },
    { offset: 0.55, transform: `translate(${dx * 0.4}px,${dy * 0.4 - 80}px) rotate(${rot - 14}deg) scale(1.1)`, opacity: 1 },
    { transform: `rotate(${rot}deg)`, opacity: 1 },
  ], 560, { easing: "cubic-bezier(.3,.6,.4,1)" });
  sfx.slide();
  setTimeout(() => { sfx.thwap(); }, 540);
  const t = wr.tokens[i];
  t.querySelector(".pencil")?.remove();
  const ck = h("div", { class: "check" }, "✓");
  t.append(ck);
  A(ck, [{ transform: "scale(0) rotate(-40deg)" }, { transform: "scale(1) rotate(0)" }], 300, { delay: 120, easing: EASE.back });
  setTimeout(sfx.pop, 140);
  wr.count.textContent = `${wr.n} of 7 in`;
  A(wr.count, [{ transform: "translateX(-50%) scale(1.25)" }, { transform: "translateX(-50%) scale(1)" }], 300, { delay: 540 });
  if (wr.n === 7) setTimeout(() => { if (wr) stampAt(710, 362, "All in!", "green", -7, 50); }, 750);
}
function lastSeconds() { if (wr) { wr.left = 11; } }

/* ---------- 3. jinx */
const gridPos = (i) => {
  const w = 210, g = 26, row = i < 4 ? 0 : 1, col = i < 4 ? i : i - 4, cnt = row === 0 ? 4 : 3;
  const total = cnt * w + (cnt - 1) * g;
  return { x: 64 + (916 - total) / 2 + col * (w + g), y: row === 0 ? 146 : 366 };
};
const revPos = (i) => ({ x: 112 + i * 290, y: 190 });
let jx = null;
function jinx() {
  mat();
  header({ tab: "Round 4 of 10", guesser: true });
  const pot = makePot(12);
  const cards = CLUES.map((c, i) => {
    const p = gridPos(i);
    return makeCard(c, { down: true, x: p.x, y: p.y, showBold: true });
  });
  cards.forEach((c, i) => {
    const p = gridPos(i);
    const dx = 710 - (p.x + 105), dy = 360 - (p.y + 75);
    A(c, [{ transform: `translate(${dx}px,${dy}px) rotate(${rnd(-20, 20)}deg) scale(.7)` }, { transform: "none" }], 460, { delay: 80 + i * 100, easing: EASE.out });
    setTimeout(sfx.slide, 80 + i * 100);
    c.querySelectorAll(".flame").forEach((f) => A(f, [{ transform: "scale(0)" }, { transform: "scale(1)" }], 300, { delay: 900, easing: EASE.back }));
  });
  setTimeout(sfx.pop, 920);
  jx = { pot, cards };
  (async () => { if (await wait(1300)) await runJinx(); })();
}
async function runJinx() {
  if (!jx) return;
  const { pot, cards } = jx;
  const msg = h("div", { class: "banner paper", style: "top:604px;left:522px" }, "Checking for jinxes…");
  table.append(msg);
  A(msg, [{ transform: "translateX(-50%) translateY(80px)" }, { transform: "translateX(-50%)" }], 300);
  // The inspector's magnifying glass sweeps both rows.
  const lens = h("div", { class: "abs", style: "left:0;top:0;width:150px;height:150px;z-index:50", html: `<svg viewBox="0 0 140 140" width="150" height="150"><circle cx="56" cy="56" r="44" fill="rgba(255,255,255,.22)" stroke="${INK}" stroke-width="15"/><circle cx="56" cy="56" r="44" fill="none" stroke="#f6b93b" stroke-width="7"/><path d="M90 90 L128 128" stroke="${INK}" stroke-width="24" stroke-linecap="round"/><path d="M92 92 L125 125" stroke="#a86b32" stroke-width="13" stroke-linecap="round"/></svg>` });
  fx.append(lens);
  const L = (x, y) => `translate(${x - 60}px,${y - 60}px)`;
  A(lens, [
    { transform: L(-80, 220), offset: 0 }, { transform: L(150, 210), offset: 0.12 }, { transform: L(880, 230), offset: 0.45 },
    { transform: L(260, 440), offset: 0.62 }, { transform: L(780, 450), offset: 0.88 }, { transform: L(1100, 760), offset: 1 },
  ], 1500, { easing: EASE.inout });
  sfx.whoosh(); setTimeout(sfx.whoosh, 700);
  remove(lens, 1520);
  if (!(await wait(1500))) return;

  // 1) The LAVA cards (Anna bold, Chiara, Gus) fly together, SLAM, shatter.
  const grp = [0, 2, 6];
  const cs = grp.map((i) => { const p = gridPos(i); return { x: p.x + 105, y: p.y + 75 }; });
  const mx = 640, my = 221; // Chiara's slot: the others fly to her
  const rots = [-10, 5, 13];
  grp.forEach((i, k) => {
    const c = cards[i];
    c.style.zIndex = String(20 + k);
    const dx = mx + (k - 1) * 10 - cs[k].x, dy = my + (k - 1) * 6 - cs[k].y;
    A(c, [
      { transform: "none", easing: EASE.out },
      { offset: 0.32, transform: `translate(${dx * -0.06}px,${dy * -0.06 - 22}px) rotate(${rots[k] * -0.4}deg) scale(1.14)`, easing: EASE.in },
      { transform: `translate(${dx}px,${dy}px) rotate(${rots[k]}deg) scale(1)` },
    ], 680);
  });
  sfx.whoosh();
  if (!(await wait(680))) return;
  sfx.slam(); shake(13, 380); dust(mx, my + 70, 12, 130);
  if (!(await wait(200))) return;
  sfx.shatter();
  grp.forEach((i, k) => { shatter(cards[i], mx + (k - 1) * 10, my + (k - 1) * 6, rots[k], 210, 150, CLUES[i].bold); cards[i].remove(); });
  if (!(await wait(160))) return;
  const s1 = stampAt(mx, my - 10, "Jinx!", "", -8, 84);
  if (!(await wait(330))) return;
  const t1 = tapeAt(mx, my + 64, "same word × 3", 2);
  if (!(await wait(400))) return;
  floatText(758, 420, "Anna's bold card −2", "minus", 2400, table); // in Gus's empty slot
  drain(pot, 1);
  if (!(await wait(1500))) return;

  // 2) Emeka's VOLCANIC: too close to the word. Struck with a marker, stamped VOID, torn up.
  const v = cards[4];
  v.style.zIndex = "20";
  A(v, [{ transform: "none" }, { transform: "translateY(-10px) scale(1.06)" }], 220);
  if (!(await wait(240))) return;
  drawX(v, 210, 150);
  if (!(await wait(620))) return;
  const vp = gridPos(4);
  const s2 = stampAt(vp.x + 105, vp.y + 70, "Void", "ink", 6, 54);
  if (!(await wait(320))) return;
  const t2 = tapeAt(vp.x + 105, vp.y + 128, "too close to the word", -2);
  if (!(await wait(800))) return;
  sfx.rip(); tear(v); fadeOut(s2, 500);
  drain(pot, 1);
  if (!(await wait(900))) return;

  // 3) Clean up: survivors slide into the reveal row; the guesser's count.
  [s1, t1, t2].forEach((e) => fadeOut(e, 300));
  fadeOut(msg, 300);
  SURVIVORS.forEach((ci, k) => {
    const c = cards[ci], p = gridPos(ci), q = revPos(k);
    c.style.transformOrigin = "0 0";
    A(c, [{ transform: "none" }, { transform: `translate(${q.x - p.x}px,${q.y - p.y}px) scale(${240 / 210},${165 / 150})` }], 620, { delay: k * 80, easing: EASE.inout });
    setTimeout(sfx.slide, k * 80);
  });
  if (!(await wait(800))) return;
  const res = h("div", { class: "banner paper", style: "top:470px;left:522px;font-size:34px;padding:12px 34px" }, h("b", { style: "color:#df4a3c" }, "4 lost"), " · ", h("b", null, "3 clues left"), " for Hiroko");
  table.append(res);
  A(res, [{ transform: "translateX(-50%) scale(1.6) rotate(-6deg)", opacity: 0 }, { transform: "translateX(-50%) scale(1) rotate(-1deg)", opacity: 1 }], 280, { easing: EASE.in });
  setTimeout(() => { sfx.stamp(); shake(4); }, 200);
}

/* ---------- 4. reveal */
let rv = null;
function reveal() {
  mat();
  header({ tab: "Round 4 of 10", guesser: true });
  const pot = makePot(10);
  const cards = SURVIVORS.map((ci, k) => { const q = revPos(k); return makeCard(CLUES[ci], { down: true, x: q.x, y: q.y, w: 240, h: 165, font: 44 }); });
  const guess = h("div", { class: "guess" }, h("div", { class: "lbl" }, `${PLAYERS[GUESSER].n}'s guess`), h("div", { class: "slot" }, "Hiroko may guess after any card"));
  table.append(guess);
  rv = { pot, cards, step: 0, turned: 0, typing: false };
  (async () => {
    if (!(await wait(900))) return;
    for (let i = 0; i < 3; i++) { await revealStep(); if (!(await wait(1400))) return; }
  })();
}
async function revealStep() {
  if (!rv) return;
  if (rv.turned < PICKS.length) return turnCard(PICKS[rv.turned]);
  if (!rv.typing) return typeGuess();
}
async function turnCard(k) {
  if (!rv || rv.cards[k].dataset.up) return;
  const c = rv.cards[k];
  c.dataset.up = "1";
  const order = ++rv.turned;
  const q = revPos(k), cx = q.x + 120;
  const name = PLAYERS[CLUES[SURVIVORS[k]].by].n;
  const b = h("div", { class: "bubble", style: `left:${cx}px;top:112px;z-index:30` }, `Hiroko trusts ${name}`);
  table.append(b);
  A(b, [{ transform: "translateX(-50%) scale(0)", opacity: 0 }, { transform: "translateX(-50%) scale(1)", opacity: 1 }], 280, { easing: EASE.back });
  sfx.pop();
  c.style.zIndex = "10";
  A(c, [{ transform: "none" }, { transform: "translateY(-14px) scale(1.05)" }], 240);
  if (!(await wait(800))) return;
  sfx.whoosh(); setTimeout(sfx.flipTick, 250);
  A(c.querySelector(".flip"), [{ transform: "rotateY(180deg)" }, { transform: "rotateY(0deg)" }], 640, { easing: EASE.inout });
  A(c, [{ transform: "translateY(-14px) scale(1.05)" }, { offset: 0.5, transform: "translateY(-50px) scale(1.32)" }, { transform: "none" }], 640, { easing: EASE.inout });
  if (!(await wait(620))) return;
  sfx.thwap(); shake(3); dust(cx, q.y + 160, 6, 120);
  fadeOut(b, 250);
  A(c.querySelector(".hl"), [{ transform: "scaleX(0)" }, { transform: "scaleX(1)" }], 280, { delay: 100, easing: EASE.inout });
  const badge = h("div", { class: "order" }, String(order));
  c.append(badge);
  A(badge, [{ transform: "scale(0)" }, { transform: "scale(1)" }], 260, { delay: 200, easing: EASE.back });
  if (CLUES[SURVIVORS[k]].bold) {
    c.querySelectorAll(".front .flame").forEach((f) => A(f, [{ transform: "scale(1)" }, { transform: "scale(1.6) rotate(-12deg)" }, { transform: "scale(1)" }], 500, { delay: 200 }));
    setTimeout(() => floatText(q.x + 230, q.y - 40, "bold ×2", "plus", 1500, fx), 250);
    setTimeout(sfx.sparkle, 250);
  }
  if (!(await wait(450))) return;
  await drain(rv.pot, 2);
}
async function typeGuess(text = wrongPath ? "ETNA" : "VOLCANO") {
  if (!rv || rv.typing) return;
  rv.typing = true;
  const wg = table.querySelector(".who-guesses");
  if (wg) fadeOut(wg, 250);
  const sign = h("div", { class: "sign", style: "left:522px;z-index:30" },
    h("div", { class: "strings", html: `<svg viewBox="0 0 220 60" width="220" height="60"><path d="M110 0 L20 56 M110 0 L200 56" stroke="${INK}" stroke-width="4" fill="none"/><circle cx="110" cy="4" r="7" fill="#df4a3c" stroke="${INK}" stroke-width="3"/></svg>` }),
    h("div", { class: "board" }, h("b", null, "SHHH!"), h("span", null, "Hiroko is guessing")));
  table.append(sign);
  A(sign, [
    { transform: "translateY(-240px) rotate(0)", offset: 0 },
    { transform: "translateY(0) rotate(-12deg)", offset: 0.22, easing: "ease-out" },
    { transform: "rotate(9deg)", offset: 0.42, easing: "ease-in-out" },
    { transform: "rotate(-5deg)", offset: 0.6, easing: "ease-in-out" },
    { transform: "rotate(2.5deg)", offset: 0.76, easing: "ease-in-out" },
    { transform: "rotate(-1deg)", offset: 0.9, easing: "ease-in-out" },
    { transform: "rotate(0)", offset: 1 },
  ], 1700, { easing: EASE.lin });
  setTimeout(sfx.thunk, 330);
  const guess = table.querySelector(".guess");
  const slot = guess.querySelector(".slot");
  if (slot) fadeOut(slot, 200);
  const dymo = h("div", { class: "dymo" }, h("i", { class: "caret" }));
  guess.append(dymo);
  A(dymo, [{ transform: "translateX(-50%) scaleX(.2)", opacity: 0 }, { transform: "translateX(-50%) scaleX(1)", opacity: 1 }], 260, { delay: 500, easing: EASE.back });
  if (!(await wait(1100))) return;
  const caret = dymo.querySelector(".caret");
  const script = text === "VOLCANO" ? ["V", "O", "L", "C", 700, "A", "M", "O", 650, "<", "<", 350, "N", "O", 1000, "\n"] : ["E", "T", 500, "N", "A", 1000, "\n"];
  for (const s of script) {
    if (typeof s === "number") { if (!(await wait(s))) return; continue; }
    if (s === "<") {
      const l = caret.previousElementSibling;
      if (l) { A(l, [{ transform: "none", opacity: 1 }, { transform: "translateY(26px) rotate(30deg) scale(.4)", opacity: 0 }], 140); remove(l, 150); }
      sfx.unpunch();
    } else if (s === "\n") {
      dymo.classList.add("done");
      sfx.snip();
      A(dymo, [{ transform: "translateX(-50%) rotate(0)" }, { transform: "translateX(-50%) translateY(6px) rotate(-3deg)" }, { transform: "translateX(-50%) rotate(-1.5deg)" }], 300, { easing: EASE.back });
      if (!(await wait(300))) return;
      const p = sp(dymo);
      stampAt(Math.min(p.x + p.w + 120, 880), p.y + p.h / 2 + 6, "Locked in", "gold", 8, 30);
      return;
    } else {
      const l = h("span", { class: "l" }, s);
      caret.before(l);
      A(l, [{ transform: "translateY(-16px) scale(1.8)", opacity: 0 }, { transform: "none", opacity: 1 }], 140);
      A(dymo, [{ transform: "translateX(-50%) translateY(4px)" }, { transform: "translateX(-50%)" }], 120, { easing: EASE.out });
      sfx.punch();
    }
    if (!(await wait(rnd(150, 300)))) return;
  }
}

/* ---------- 5. verdict */
const GIVER_DELTAS = { anna: -1, ben: 0, chiara: 0, dev: 1, emeka: 0, fatima: 2, gus: 0 };
function verdict() {
  mat();
  header({ tab: "Round 4 of 10", guesser: true });
  const pot = makePot(6);
  const g = token(GUESSER, 150, "", "left:84px;top:220px");
  table.append(g);
  table.append(h("div", { class: "seatname", style: "left:84px;top:382px;width:150px;font-size:30px" }, PLAYERS[GUESSER].n));
  const env = envelope(410, 160);
  const secret = h("div", { class: "secret paper", style: "left:-110px;top:10px;z-index:2;transform:scale(.5)" }, h("small", null, "THE WORD WAS"), h("div", { class: "w" }, SECRET));
  env.insertBefore(secret, env.querySelector(".pocket"));
  const guess = h("div", { class: "guess", style: "top:452px" }, h("div", { class: "lbl" }, `${PLAYERS[GUESSER].n}'s guess`));
  const dymo = h("div", { class: "dymo done", style: "transform:translateX(-50%) rotate(-1.5deg)" }, ...(wrongPath ? "ETNA" : "VOLCANO").split("").map((l) => h("span", { class: "l" }, l)));
  guess.append(dymo);
  table.append(guess);
  const row = h("div", { class: "abs", style: "left:96px;top:600px;width:880px;height:70px" });
  table.append(row);
  rv = null;
  const vd = { pot, g, env, secret, dymo, row };
  (async () => { if (await wait(700)) await runVerdict(vd, !wrongPath); })();
}
async function runVerdict(vd, right) {
  const { pot, g, env, secret, dymo, row } = vd;
  sfx.drumroll(1100);
  A(env, [{ transform: "rotate(0)" }, { transform: "rotate(-2.5deg)" }, { transform: "rotate(2.5deg)" }, { transform: "rotate(0)" }], 120, { iterations: 9, fill: "none", easing: EASE.lin });
  if (!(await wait(1150))) return;
  sfx.cymbal(); sfx.whoosh();
  const fw = env.querySelector(".flapwrap");
  A(fw, [{ transform: "rotateX(0)" }, { transform: "rotateX(180deg)" }], 340, { easing: EASE.inout });
  A(env.querySelector(".seal"), [{ transform: "scale(1)", opacity: 1 }, { transform: "translateY(-60px) scale(1.3) rotate(40deg)", opacity: 0 }], 340);
  setTimeout(() => { fw.style.zIndex = "1"; }, 170);
  if (!(await wait(360))) return;
  A(secret, [{ transform: "scale(.5)" }, { transform: "translateY(-150px) scale(.5)" }], 380, { easing: EASE.out });
  sfx.slide();
  if (!(await wait(380))) return;
  secret.style.zIndex = "6";
  for (const part of env.querySelectorAll(".body,.pocket,.flapwrap")) A(part, [{ transform: part === fw ? "rotateX(180deg)" : "none", opacity: 1 }, { transform: `translateY(120px)${part === fw ? " rotateX(180deg)" : ""}`, opacity: 0 }], 420, { easing: EASE.in });
  A(secret, [{ transform: "translateY(-150px) scale(.5)" }, { transform: "translateY(-30px) scale(1)" }], 460, { easing: EASE.back });
  if (!(await wait(500))) return;
  sfx.thwap(); shake(5);
  A(dymo.parentElement, [{ transform: "none" }, { transform: "translateY(-80px)" }], 360, { easing: EASE.back });
  if (!(await wait(420))) return;
  const sp0 = sp(secret);
  const wcx = sp0.x + sp0.w / 2, wcy = sp0.y + sp0.h / 2;
  if (right) {
    stampAt(wcx + 230, wcy - 92, "Right!", "green", 10, 60);
    setTimeout(() => { confetti(wcx, wcy, 54); sfx.fanfare(); }, 150);
    A(secret, [{ transform: "translateY(-30px) scale(1)" }, { transform: "translateY(-30px) scale(1.08) rotate(-2deg)" }, { transform: "translateY(-30px) scale(1)" }], 400, { delay: 160, easing: EASE.back });
  } else {
    strikeThrough(dymo);
    if (!(await wait(560))) return;
    stampAt(wcx + 230, wcy - 92, "Not it", "ink", 10, 60);
    setTimeout(sfx.trombone, 200);
    A(secret, [{ transform: "translateY(-30px) rotate(0)" }, { transform: "translateY(-30px) rotate(-2deg)" }, { transform: "translateY(-30px) rotate(1.5deg)" }, { transform: "translateY(-30px) rotate(0)" }], 600, { delay: 200 });
  }
  if (!(await wait(1100))) return;
  // The pot: to the guesser, or back to the bank.
  if (right) {
    const tp = sp(g);
    const ok = await drain(pot, 6, { x: tp.x + tp.w / 2, y: tp.y + tp.h / 2 }, 110);
    if (!ok) return;
    if (!(await wait(450))) return;
    floatText(tp.x + tp.w / 2, tp.y - 30, "+6", "plus", 2200, table);
    A(g, [{ transform: "scale(1)" }, { transform: "scale(1.15) rotate(-6deg)" }, { transform: "scale(1)" }], 400, { easing: EASE.back, fill: "none" });
    sfx.sparkle();
  } else {
    if (!(await drain(pot, 6, "away", 90))) return;
  }
  if (!(await wait(400))) return;
  // Everyone else's points: their tokens slide up with a ticket each.
  GIVERS.forEach((id, i) => {
    const dlt = right ? GIVER_DELTAS[id] : id === "anna" ? -1 : 0;
    const x = i * 126;
    const t = token(id, 56, "", `left:${x}px;top:4px`);
    const tag = h("div", { class: "abs", style: `left:${x + 62}px;top:14px;font-size:30px;font-weight:700;color:${dlt > 0 ? "#8ff0a8" : dlt < 0 ? "#ff9b8f" : "#7fb3ac"}` }, dlt > 0 ? `+${dlt}` : dlt < 0 ? `−${-dlt}` : "0");
    row.append(t, tag);
    A(t, [{ transform: "translateY(110px)" }, { transform: "none" }], 380, { delay: i * 90, easing: EASE.back });
    A(tag, [{ transform: "scale(0)" }, { transform: dlt ? "scale(1.25)" : "scale(1)" }, { transform: "scale(1)" }], 360, { delay: 500 + i * 120, easing: EASE.back });
    if (dlt > 0) setTimeout(sfx.clink, 500 + i * 120); else if (dlt < 0) setTimeout(sfx.bonk, 500 + i * 120);
  });
  // The room's tally: this round's slot turns over.
  const mini = table.querySelectorAll(".tally .mini")[3];
  const tl = table.querySelector(".tally .tl");
  if (mini) {
    A(mini, [{ transform: "rotateY(0)" }, { transform: "rotateY(90deg)" }], 180, { fill: "none", delay: 1200 });
    setTimeout(() => { mini.className = `mini ${right ? "won" : "lost"}`; if (tl) tl.textContent = right ? "3 of 4" : "2 of 4"; sfx.flipTick(); }, 1380);
    A(mini, [{ transform: "rotateY(90deg) scale(1.3)" }, { transform: "rotateY(0) scale(1)" }], 260, { delay: 1380, fill: "none", easing: EASE.back });
  }
}

/* ---------- 6. replay + MVP */
let rp = null;
function replay() {
  mat();
  header({ tab: "Round 4 · replay", tally: TALLY.map((t, i) => (i === 3 ? "won" : t)) });
  const word = h("div", { class: "paper abs", style: "left:400px;top:30px;padding:6px 22px;font-size:40px;font-weight:700;letter-spacing:.06em;transform:rotate(1.5deg)" }, "VOLCANO ", h("span", { style: "color:#2f9e57" }, "✓"));
  table.append(word);
  const all = [];
  // Row 1: the survivors in the order they were turned.
  const row1 = [[5, "1"], [3, "2"], [1, null]];
  row1.forEach(([ci, ord], k) => {
    const c = makeCard(CLUES[ci], { down: false, x: 100 + k * 290, y: 128, w: 240, h: 150, font: 42 });
    c.querySelector(".hl").style.transform = ord ? "scaleX(1)" : "scaleX(0)";
    if (ord) c.append(h("div", { class: "order" }, ord));
    else c.append(h("div", { class: "tape", style: "right:-30px;top:-22px;transform:rotate(8deg);font-size:24px;padding:4px 14px;z-index:6" }, "not turned"));
    all.push(c);
  });
  // Row 2: the jinx — three LAVA cards taped back together.
  const stack = [];
  [[0, -9, 0, 0], [2, 2, 26, 10], [6, 11, 52, 4]].forEach(([ci, r, ox, oy]) => {
    const c = makeCard(CLUES[ci], { down: false, x: 100 + ox, y: 362 + oy, w: 210, h: 140, font: 40, who: false, style: `transform:rotate(${r}deg)` });
    c.append(h("div", { class: "crack", html: `<svg viewBox="0 0 210 140" width="210" height="140"><path d="M0 52 L44 60 L70 40 L112 74 L150 58 L210 88" fill="none" stroke="#8a7b65" stroke-width="3"/><path d="M112 74 L104 140" fill="none" stroke="#8a7b65" stroke-width="3"/></svg>` }));
    stack.push(c); all.push(c);
  });
  const tapes = [h("div", { class: "tapebit", style: "left:96px;top:452px;transform:rotate(-58deg);z-index:9" }), h("div", { class: "tapebit", style: "left:296px;top:372px;transform:rotate(38deg);z-index:9" }), h("div", { class: "tapebit", style: "left:270px;top:500px;transform:rotate(-20deg);z-index:9" })];
  tapes.forEach((t) => table.append(t));
  const cap1 = h("div", { class: "tape", style: "left:90px;top:552px;transform:rotate(-1.5deg);font-size:28px;z-index:9" }, "3 of you wrote LAVA");
  table.append(cap1);
  // The void card.
  const v = makeCard(CLUES[4], { down: false, x: 640, y: 352, w: 210, h: 140, font: 40 });
  all.push(v);
  const cap2 = h("div", { class: "tape", style: "left:560px;top:570px;transform:rotate(2deg);font-size:26px" }, "too close to the word");
  table.append(cap2);
  // Right column: points this round, and the MVP vote.
  const note = h("div", { class: "paper abs", style: "left:1000px;top:128px;width:216px;padding:16px 18px;transform:rotate(1.5deg)" },
    h("div", { style: "font-size:24px;font-weight:700;letter-spacing:.12em;color:#5d5244" }, "THIS ROUND"),
    ...[["hiroko", "+6"], ["fatima", "+2"], ["dev", "+1"], ["anna", "−1"]].map(([id, d]) => h("div", { style: "display:flex;align-items:center;gap:10px;margin-top:10px;font-size:28px;font-weight:700" }, token(id, 40, "", "position:relative;flex:none;box-shadow:0 0 0 3px #1d1a2b"), h("span", { style: "flex:1" }, PLAYERS[id].n), h("span", { style: `color:${d.startsWith("+") ? "#2f9e57" : "#df4a3c"}` }, d))));
  table.append(note);
  const mvpNote = h("div", { class: "paper abs", style: "left:1000px;top:420px;width:216px;padding:14px 18px;font-size:26px;font-weight:700;transform:rotate(-1.5deg);text-align:center" }, "Hiroko picks the ", h("span", { style: "color:#df4a3c" }, "MVP clue"), "…");
  table.append(mvpNote);
  all.forEach((c, i) => {
    const base = c.style.transform || "none";
    A(c, [{ transform: `translateY(500px) rotate(${rnd(-30, 30)}deg)` }, { transform: base }], 520, { delay: i * 110, easing: EASE.out });
    setTimeout(sfx.slide, i * 110);
  });
  tapes.forEach((t, i) => { A(t, [{ opacity: 0, scale: "1.8" }, { opacity: 1, scale: "1" }], 160, { delay: 1100 + i * 180, easing: EASE.in }); setTimeout(sfx.stamp, 1220 + i * 180); });
  [cap1, cap2].forEach((c, i) => A(c, [{ opacity: 0, translate: "0 20px" }, { opacity: 1, translate: "0 0" }], 300, { delay: 1300 + i * 150 }));
  setTimeout(() => strikeThrough(v.querySelector(".front")), 1500);
  rp = { target: all[0], mvpNote, done: false };
  (async () => { if (await wait(2600)) await mvp(); })();
}
async function mvp() {
  if (!rp || rp.done) return;
  rp.done = true;
  const c = rp.target;
  const p = sp(c);
  const r = rosette();
  const x = p.x + p.w - 52, y = p.y + p.h - 70;
  r.style.left = x + "px"; r.style.top = y + "px";
  table.append(r);
  sfx.whoosh();
  A(r, [{ transform: "translate(420px,-380px) rotate(80deg) scale(2.4)", opacity: 0 }, { offset: 0.2, opacity: 1 }, { transform: "rotate(-12deg) scale(.85)", opacity: 1 }], 520, { easing: EASE.in });
  if (!(await wait(520))) return;
  sfx.stamp(); shake(7); dust(x + 62, y + 62, 10, 110, "#f6b93b");
  A(r, [{ transform: "rotate(-12deg) scale(.95,.75)" }, { transform: "rotate(-12deg) scale(.85)" }], 220, { easing: EASE.back });
  const shine = h("div", { class: "shine" }, h("i"));
  c.querySelector(".front").append(shine);
  A(shine.firstChild, [{ transform: "skewX(-20deg) translateX(-120px)" }, { transform: "skewX(-20deg) translateX(380px)" }], 700, { delay: 150, easing: EASE.inout });
  setTimeout(sfx.sparkle, 150);
  floatText(p.x + p.w / 2 - 20, p.y + 14, "+3 Fatima", "plus", 2200, fx);
  rp.mvpNote.innerHTML = "";
  rp.mvpNote.append(h("div", { style: "font-size:24px;letter-spacing:.12em;color:#5d5244" }, "MVP CLUE"), h("div", { style: "font-size:34px" }, "POMPEII"), h("div", { style: "color:#2f9e57" }, "Fatima +3"));
  A(rp.mvpNote, [{ transform: "rotate(-1.5deg) scale(1.2)" }, { transform: "rotate(-1.5deg) scale(1)" }], 300, { easing: EASE.back });
}

/* ---------- 7. scores */
const BEFORE = { fatima: 22, dev: 19, anna: 17, hiroko: 15, ben: 14, chiara: 12, gus: 9, emeka: 8 };
const DELTA = { hiroko: 6, fatima: 5, dev: 1, anna: -1, ben: 0, chiara: 0, gus: 0, emeka: 0 };
function scores() {
  mat();
  const pad = h("div", { class: "pad paper" }, h("div", { class: "binding" }, ...Array.from({ length: 9 }, (_, i) => h("i", { style: `left:${40 + i * 68}px` }))), h("div", { class: "margin" }), h("h2", null, "SCORES", h("span", null, "after round 4 of 10")));
  table.append(pad);
  const ids = Object.keys(BEFORE);
  const rows = ids.map((id, i) => {
    const d = DELTA[id];
    const r = h("div", { class: "prow", style: `top:${120 + i * 64}px` },
      h("div", { class: "rank" }, String(i + 1)), token(id, 46), h("div", { class: "n" }, PLAYERS[id].n),
      h("div", { class: `d ${d > 0 ? "plus" : d < 0 ? "minus" : ""}`, style: d ? "" : "color:#a89a85" }, d > 0 ? `+${d}` : d < 0 ? `−${-d}` : "–"),
      h("div", { class: "t" }, String(BEFORE[id])));
    pad.append(r);
    return { id, r, i };
  });
  A(pad, [{ transform: "translateY(760px) rotate(6deg)" }, { transform: "rotate(-1deg)" }], 620, { easing: EASE.back });
  sfx.slide();
  const room = h("div", { class: "room" },
    h("div", { class: "k" }, "THE ROOM, TOGETHER"),
    h("div", { class: "big" }, "3", h("span", { style: "font-size:56px;color:#a8d3cd" }, " of 4")),
    h("div", { class: "sub" }, "words found so far"),
    h("div", { class: "slots" }, ...["won", "lost", "won", "won", "todo", "todo", "todo", "todo", "todo", "todo"].map((s) => h("i", { class: s }, s === "won" ? "✓" : s === "lost" ? "✕" : ""))),
    h("div", { class: "next" }, h("span", { style: "color:#a8d3cd" }, "Next up:"), token("anna", 76), "Anna guesses"));
  table.append(room);
  A(room, [{ transform: "translateX(500px)", opacity: 0 }, { transform: "none", opacity: 1 }], 520, { delay: 200 });
  (async () => {
    if (!(await wait(800))) return;
    let k = 0;
    for (const { id, r } of rows) {
      const d = r.querySelector(".d");
      A(d, [{ clipPath: "inset(0 100% 0 0)" }, { clipPath: "inset(0 0 0 0)" }], 240, { delay: k * 110, easing: EASE.inout });
      if (DELTA[id]) setTimeout(sfx.marker, k * 110);
      k++;
    }
    if (!(await wait(k * 110 + 400))) return;
    // Totals count up.
    for (const { id, r } of rows) {
      const t = r.querySelector(".t"), a = BEFORE[id], b = a + DELTA[id];
      if (a === b) continue;
      const steps = Math.abs(b - a);
      for (let s = 1; s <= steps; s++) setTimeout(() => { t.textContent = String(a + Math.sign(b - a) * s); }, (s * 500) / steps);
      A(t, [{ transform: "scale(1)" }, { transform: "scale(1.3)" }, { transform: "scale(1)" }], 500, { fill: "none" });
    }
    sfx.clink(); setTimeout(sfx.clink, 200);
    if (!(await wait(800))) return;
    // Re-sort.
    const sorted = [...ids].sort((x, y) => BEFORE[y] + DELTA[y] - (BEFORE[x] + DELTA[x]));
    rows.forEach(({ id, r, i }) => {
      const ni = sorted.indexOf(id);
      if (ni === i) return;
      r.style.zIndex = ni < i ? "3" : "1";
      A(r, [{ transform: "none" }, { offset: 0.5, transform: `translateY(${(ni - i) * 32}px) scale(${ni < i ? 1.04 : 0.98})` }, { transform: `translateY(${(ni - i) * 64}px)` }], 700, { easing: EASE.inout });
      if (ni < i) setTimeout(() => { const p = sp(r); floatText(p.x + 300, p.y + 8, `▲${i - ni}`, "plus", 1500, fx); }, 700);
    });
    sfx.whoosh();
    if (!(await wait(720))) return;
    rows.forEach(({ id, r }) => { r.querySelector(".rank").textContent = String(sorted.indexOf(id) + 1); });
    const first = rows.find((x) => x.id === sorted[0]).r;
    first.classList.add("first");
    A(first, [{ backgroundColor: "#fbf1dc" }, { backgroundColor: "#fde7a6" }], 300);
    sfx.pop();
  })();
}

/* ---------- 8. end: the room's result, then the MVP of the night */
const NIGHT = [
  ["Volcano", 1], ["Dragon", 1], ["Castle", 1], ["Ghost", 0], ["Pizza", 1], ["Robot", 0], ["Desert", 1], ["Ocean", 1], ["Piano", 0], ["Snowman", 1],
];
let res = null;
function result() {
  mat();
  table.append(h("div", { class: "tab paper", style: "left:50%;transform:translateX(-50%) rotate(-1deg)" }, "Game over · the room"));
  const box = h("div", { class: "abs", style: "inset:0" });
  table.append(box);
  const tiles = h("div", { class: "bigtiles", style: "top:112px" }, h("div", { class: "tile" }, "7"), h("div", { class: "of" }, "of"), h("div", { class: "tile" }, "1"), h("div", { class: "tile" }, "0"));
  box.append(tiles);
  box.append(h("div", { class: "caption", style: "top:280px" }, "words found together"));
  [...tiles.children].forEach((t, i) => { A(t, [{ transform: "translateY(-300px) rotate(-25deg)", opacity: 0 }, { transform: `rotate(${[-3, 0, 2, -2][i]}deg)`, opacity: 1 }], 420, { delay: 200 + i * 140, easing: EASE.back }); setTimeout(sfx.thunk, 400 + i * 140); });
  const fan = h("div", { class: "ministack abs", style: "inset:0" });
  box.append(fan);
  NIGHT.forEach(([w, won], i) => {
    const col = i % 5, rowi = Math.floor(i / 5);
    const x = 640 + (col - 2) * 204 - 90, y = 340 + rowi * 104, a = ((i * 7) % 5) - 2;
    const c = h("div", { class: `mc ${won ? "won" : "lost"}`, style: `left:${x}px;top:${y}px;width:180px;height:80px;font-size:30px;transform:rotate(${a}deg);z-index:${i}` }, w);
    fan.append(c);
    A(c, [{ transform: `translate(${640 - x - 90}px,420px) rotate(${a * 8}deg)` }, { transform: `rotate(${a}deg)` }], 480, { delay: 900 + i * 80, easing: EASE.out });
    setTimeout(sfx.slide, 900 + i * 80);
  });
  res = { box, done: false };
  (async () => {
    if (!(await wait(2000))) return;
    stampAt(640, 590, "In sync!", "gold", -4, 64);
    setTimeout(() => confetti(640, 560, 30), 200);
    if (!(await wait(2600))) return;
    await nightMvp();
  })();
}
async function nightMvp() {
  if (!res || res.done) return;
  res.done = true;
  for (const e of table.querySelectorAll(".stamp")) fadeOut(e, 300);
  A(res.box, [{ transform: "none", opacity: 1 }, { transform: "translateY(120px)", opacity: 0 }], 420, { easing: EASE.in });
  const spot = h("div", { class: "spot", style: "left:390px;top:96px;width:500px;height:500px" });
  table.append(spot);
  A(spot, [{ transform: "scale(0)" }, { transform: "scale(1)" }], 400, { delay: 300, easing: EASE.back });
  const ped = h("div", { class: "pedestal", style: "left:490px;top:396px" });
  table.append(ped);
  A(ped, [{ transform: "scaleX(0)" }, { transform: "scaleX(1)" }], 300, { delay: 500, easing: EASE.back });
  table.querySelector(".tab").textContent = "Game over · MVP of the night";
  sfx.drumroll(1500);
  if (!(await wait(1550))) return;
  const t = token("fatima", 220, "", "left:530px;top:200px;z-index:5");
  table.append(t);
  A(t, [
    { transform: "translateY(-420px) scale(1.5)", opacity: 0 },
    { offset: 0.6, transform: "translateY(0) scale(1)", opacity: 1, easing: EASE.out },
    { offset: 0.8, transform: "scale(1.12,.86)", easing: EASE.out },
    { transform: "scale(1)" },
  ], 520, { easing: EASE.in });
  if (!(await wait(320))) return;
  sfx.slam(); sfx.cymbal(); shake(12, 400); dust(640, 428, 12, 160);
  setTimeout(() => { confetti(640, 300, 70, 1.2); sfx.fanfare(); }, 120);
  const rib = h("div", { class: "ribbon", style: "top:508px;z-index:6" }, "MVP OF THE NIGHT");
  table.append(rib);
  A(rib, [{ clipPath: "polygon(50% 0,50% 0,50% 100%,50% 100%)" }, { clipPath: "polygon(0 0,100% 0,95% 50%,100% 100%,0 100%,5% 50%)" }], 420, { delay: 300, easing: EASE.out });
  const cap = h("div", { class: "caption", style: "top:610px;font-size:32px" }, h("b", null, "Fatima"), " · 41 points · 3 MVP clues · never jinxed");
  table.append(cap);
  A(cap, [{ opacity: 0, transform: "translateY(20px)" }, { opacity: 1, transform: "none" }], 400, { delay: 650 });
}

/* ---------- 9. awards */
const AWARDS = [
  { t: "Mind meld", who: ["anna", "chiara"], names: "Anna & Chiara", stat: "jinxed each other 4 times", icon: `<svg viewBox="0 0 110 90"><circle cx="40" cy="45" r="32" fill="#ff7a5c" stroke="${INK}" stroke-width="5"/><circle cx="70" cy="45" r="32" fill="#8bd648" fill-opacity=".85" stroke="${INK}" stroke-width="5"/><path d="M55 18 A32 32 0 0 1 55 72 A32 32 0 0 1 55 18 Z" fill="${INK}"/></svg>` },
  { t: "Lone wolf", who: ["dev"], names: "Dev", stat: "6 one-of-a-kind clues that got turned", icon: `<svg viewBox="0 0 110 90"><path d="M55 6 L65 34 L95 35 L71 53 L80 83 L55 65 L30 83 L39 53 L15 35 L45 34 Z" fill="#f6b93b" stroke="${INK}" stroke-width="5" stroke-linejoin="round"/></svg>` },
  { t: "Kamikaze", who: ["gus"], names: "Gus", stat: "lost 5 clues to jinxes (and kept going)", icon: `<svg viewBox="0 0 110 90"><circle cx="50" cy="54" r="30" fill="${INK}"/><path d="M68 30 Q80 14 92 18" stroke="${INK}" stroke-width="6" fill="none" stroke-linecap="round"/><path d="M92 6 L95 15 L104 14 L97 20 L102 28 L93 24 L88 31 L88 21 L80 18 L89 15 Z" fill="#f6b93b" stroke="${INK}" stroke-width="2.5" stroke-linejoin="round"/><circle cx="40" cy="44" r="7" fill="#fbf1dc" opacity=".6"/></svg>` },
];
let aw = null;
function awards() {
  mat();
  table.append(h("div", { class: "tab paper", style: "left:50%;transform:translateX(-50%) rotate(-1deg)" }, "Game over · awards"));
  const cards = AWARDS.map((a, i) => {
    const x = 110 + i * 370, y = 140;
    const burst = h("div", { class: "burst", style: `left:${x + 160 - 260}px;top:${y + 230 - 260}px;transform:scale(0)` });
    table.append(burst);
    const el = h("div", { class: "award", style: `left:${x}px;top:${y}px` },
      h("div", { class: "flip" },
        h("div", { class: "face front" }, h("h3", null, a.t), h("div", { class: "icon", html: a.icon }), h("div", { class: "winners" }, ...a.who.map((id) => token(id, 68))), h("div", { class: "names" }, a.names), h("div", { class: "stat" }, a.stat)),
        h("div", { class: "face back" }, h("b", null, "?"))));
    table.append(el);
    A(el, [{ transform: `translateY(560px) rotate(${(i - 1) * 20}deg)` }, { transform: `rotate(${(i - 1) * 2}deg)` }], 560, { delay: 150 + i * 150, easing: EASE.out });
    setTimeout(sfx.slide, 150 + i * 150);
    return { el, burst, up: false };
  });
  aw = { cards };
  (async () => {
    if (!(await wait(1200))) return;
    for (let i = 0; i < 3; i++) { if (!(await flipAward())) return; if (!(await wait(900))) return; }
  })();
}
async function flipAward() {
  if (!aw) return false;
  const c = aw.cards.find((x) => !x.up);
  if (!c) return true;
  c.up = true;
  const i = aw.cards.indexOf(c);
  sfx.drumroll(600);
  A(c.el, [{ transform: `rotate(${(i - 1) * 2}deg)` }, { transform: `translateY(-20px) rotate(${(i - 1) * 2}deg) scale(1.04)` }], 300, { delay: 300 });
  if (!(await wait(650))) return false;
  sfx.whoosh(); setTimeout(sfx.flipTick, 250);
  A(c.el.querySelector(".flip"), [{ transform: "rotateY(180deg)" }, { transform: "rotateY(0)" }], 650, { easing: EASE.inout });
  A(c.el, [{ transform: `translateY(-20px) rotate(${(i - 1) * 2}deg) scale(1.04)` }, { offset: 0.5, transform: "translateY(-40px) scale(1.14)" }, { transform: `rotate(${(i - 1) * 2}deg)` }], 650, { easing: EASE.inout });
  A(c.burst, [{ transform: "scale(0) rotate(-30deg)" }, { transform: "scale(1) rotate(0)" }], 600, { delay: 300, easing: EASE.back });
  if (!(await wait(640))) return false;
  sfx.thwap(); sfx.fanfare(); shake(4);
  c.el.querySelectorAll(".winners .token").forEach((t, k) => A(t, [{ transform: "translateY(-30px) scale(1.4)", opacity: 0 }, { transform: "none", opacity: 1 }], 320, { delay: k * 120, easing: EASE.back }));
  return true;
}

/* ---------- 10. word web */
const WEB_WORDS = {
  VOLCANO: [190, 150, 1], DRAGON: [520, 150, 1], CASTLE: [850, 150, 1], GHOST: [1108, 150, 0],
  PIZZA: [172, 376, 1], ROBOT: [1108, 388, 0],
  DESERT: [190, 610, 1], OCEAN: [560, 610, 1], PIANO: [840, 610, 0], SNOWMAN: [1108, 610, 1],
};
const WEB_HUBS = [
  ["HOT", 380, 380, ["VOLCANO", "PIZZA", "DESERT", "DRAGON"]],
  ["FIRE", 355, 150, ["VOLCANO", "DRAGON"]],
  ["KNIGHT", 685, 150, ["DRAGON", "CASTLE"]],
  ["HAUNTED", 990, 262, ["CASTLE", "GHOST"]],
  ["ARMOR", 900, 386, ["CASTLE", "ROBOT"]],
  ["ITALY", 160, 262, ["VOLCANO", "PIZZA"]],
  ["SAND", 375, 610, ["DESERT", "OCEAN"]],
  ["COLD", 720, 500, ["OCEAN", "SNOWMAN"]],
  ["WHITE", 1108, 500, ["GHOST", "SNOWMAN"]],
  ["BEEP", 975, 520, ["ROBOT", "PIANO"]],
];
function web() {
  mat();
  table.append(h("div", { class: "tab paper webtitle" }, "The word web"));
  table.append(h("div", { class: "abs", style: "left:338px;top:44px;font-size:26px;font-weight:600;color:#a8d3cd" }, "10 words, linked by the clues you shared"));
  const svgNS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(svgNS, "svg");
  svg.setAttribute("viewBox", "0 0 1280 720");
  svg.setAttribute("style", "position:absolute;inset:0;width:1280px;height:720px;overflow:visible");
  table.append(svg);
  const strings = [];
  for (const [, hx, hy, words] of WEB_HUBS) {
    const list = [];
    for (const w of words) {
      const [x, y] = WEB_WORDS[w];
      const mx = (hx + x) / 2, my = (hy + y) / 2 + 26;
      const p = document.createElementNS(svgNS, "path");
      p.setAttribute("d", `M${hx} ${hy} Q${mx} ${my} ${x} ${y}`);
      p.setAttribute("fill", "none"); p.setAttribute("stroke", "#df4a3c"); p.setAttribute("stroke-width", "5"); p.setAttribute("stroke-linecap", "round");
      svg.append(p);
      const L = p.getTotalLength();
      p.style.strokeDasharray = String(L); p.style.strokeDashoffset = String(L);
      list.push([p, L]);
    }
    strings.push(list);
  }
  const cards = Object.entries(WEB_WORDS).map(([w, [x, y, won]], i) => {
    const c = h("div", { class: `wcard ${won ? "won" : "lost"}`, style: `left:${x}px;top:${y}px;transform:rotate(${((i * 7) % 5) - 2}deg)` }, w);
    table.append(c);
    A(c, [{ transform: "scale(1.5) rotate(-12deg)", opacity: 0 }, { transform: c.style.transform, opacity: 1 }], 300, { delay: 200 + i * 80, easing: EASE.in });
    setTimeout(sfx.thwap, 380 + i * 80);
    return c;
  });
  const tags = WEB_HUBS.map(([t, x, y]) => { const e = h("div", { class: "wtag", style: `left:${x}px;top:${y}px;opacity:0` }, h("i", { class: "pin" }), t); table.append(e); return e; });
  (async () => {
    if (!(await wait(1200 + cards.length * 40))) return;
    for (let i = 0; i < strings.length; i++) {
      A(tags[i], [{ transform: "translate(-50%,-50%) scale(1.8) rotate(-8deg)", opacity: 0 }, { transform: "translate(-50%,-50%) scale(1)", opacity: 1 }], 220, { easing: EASE.in });
      setTimeout(sfx.pop, 150);
      strings[i].forEach(([p, L], k) => A(p, [{ strokeDashoffset: L }, { strokeDashoffset: 0 }], 480, { delay: 120 + k * 90, easing: EASE.inout }));
      setTimeout(sfx.zip, 120);
      if (!(await wait(300))) return;
    }
  })();
}

/* =================================================================== the mockup's shell */

const PHASES = [
  { id: "lobby", name: "Lobby", enter: lobby, actions: [["Space", "next player joins", joinNext]] },
  { id: "writing", name: "Writing", enter: writing, actions: [["Space", "next clue in", () => submit()], ["L", "last 10 s", lastSeconds]] },
  { id: "jinx", name: "Jinx", enter: jinx, actions: [["J", "replay the jinx check", () => go(2)]] },
  { id: "reveal", name: "Reveal", enter: reveal, actions: [["Space", "next step", () => { gen++; revealStep(); }], ["F", "turn a card", () => { gen++; if (rv && rv.turned < PICKS.length) turnCard(PICKS[rv.turned]); }], ["T", "type the guess", () => { gen++; typeGuess(); }]] },
  { id: "verdict", name: "Verdict", enter: verdict, actions: [["V", "right", () => { wrongPath = false; go(4); }], ["W", "wrong", () => { wrongPath = true; go(4); }]] },
  { id: "replay", name: "Replay + MVP", enter: replay, actions: [["M", "MVP clue", () => { gen++; mvp(); }]] },
  { id: "scores", name: "Scores", enter: scores, actions: [] },
  { id: "result", name: "End: room + MVP", enter: result, actions: [["U", "MVP of the night", () => { gen++; nightMvp(); }]] },
  { id: "awards", name: "End: awards", enter: awards, actions: [["A", "flip next award", () => { gen++; flipAward(); }]] },
  { id: "web", name: "End: word web", enter: web, actions: [] },
];
let cur = 0;
function go(i) {
  cur = (i + PHASES.length) % PHASES.length;
  gen++;
  intervals.forEach(clearInterval); intervals = [];
  table.getAnimations().forEach((a) => a.cancel());
  table.innerHTML = ""; fx.innerHTML = "";
  wr = jx = rv = rp = res = aw = null;
  PHASES[cur].enter();
  renderControls();
  try { history.replaceState(null, "", `?phase=${cur + 1}${wrongPath ? "&wrong=1" : ""}${params.get("controls") === "0" ? "&controls=0" : ""}`); } catch {}
}
function renderControls() {
  const ph = document.getElementById("phases"), ac0 = document.getElementById("actions");
  ph.innerHTML = ""; ac0.innerHTML = "";
  PHASES.forEach((p, i) => {
    const b = h("button", { class: i === cur ? "on" : "" }, h("kbd", null, String((i + 1) % 10)), p.name);
    b.onclick = () => go(i);
    ph.append(b);
  });
  const replayB = h("button", null, h("kbd", null, "R"), "replay phase");
  replayB.onclick = () => go(cur);
  ac0.append(replayB);
  for (const [k, label, fn] of PHASES[cur].actions) {
    const b = h("button", null, h("kbd", null, k), label);
    b.onclick = fn;
    ac0.append(b);
  }
  const snd = h("button", { class: SND.on ? "on" : "" }, h("kbd", null, "S"), SND.on ? "sound on" : "sound off");
  snd.onclick = toggleSound;
  const safe = h("button", null, h("kbd", null, "G"), "safe area");
  safe.onclick = toggleSafe;
  ac0.append(snd, safe);
  fitStage();
}
function toggleSound() { SND.on = !SND.on; if (SND.on) { ac(); sfx.pop(); } renderControls(); }
function toggleSafe() { const s = document.getElementById("safe"); s.hidden = !s.hidden; }
document.addEventListener("keydown", (e) => {
  if (e.metaKey || e.ctrlKey || e.altKey) return;
  const k = e.key;
  if (k === "ArrowRight") return go(cur + 1);
  if (k === "ArrowLeft") return go(cur - 1);
  if (/^[0-9]$/.test(k)) return go((Number(k) + 9) % 10);
  if (k === "r" || k === "R") return go(cur);
  if (k === "s" || k === "S") return toggleSound();
  if (k === "g" || k === "G") return toggleSafe();
  if (k === "h" || k === "H") { document.body.classList.toggle("nocontrols"); return fitStage(); }
  const acts = PHASES[cur].actions;
  if (k === " ") { e.preventDefault(); if (acts.length) acts[0][2](); else go(cur); return; }
  const a = acts.find(([key]) => key.toLowerCase() === k.toLowerCase());
  if (a) a[2]();
});
function fitStage() {
  const vp = document.getElementById("viewport");
  scale = Math.min(vp.clientWidth / 1280, vp.clientHeight / 720);
  stage.style.transform = `translate(-50%,-50%) scale(${scale})`;
}
window.addEventListener("resize", fitStage);
if (params.get("controls") === "0") document.body.classList.add("nocontrols");
fitStage();
document.fonts.ready.then(() => go(Math.max(0, Math.min(PHASES.length - 1, (Number(params.get("phase")) || 1) - 1))));
