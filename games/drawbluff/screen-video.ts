/**
 * drawbluff's main screen as video frames (variant 1): the same screen
 * view the web screen shows (`{ roster, screen, join }`), drawn into a 2D
 * canvas that `@tvpipe/sdk`'s `stream()` sends to the TV.
 *
 * Uses @napi-rs/canvas, a dependency of this game only: the screen is
 * mostly text in several sizes (names, prompts, titles of any language)
 * and anti-aliased strokes, which a canvas does well and raw pixels with
 * a bitmap font would do poorly. It is prebuilt Skia with no install
 * scripts and no dependencies of its own. The fonts are tvpipe's (OFL).
 */

import { fileURLToPath } from "node:url";

import { drawQr } from "@tvpipe/party";

import { drawDrawing } from "./web/phone/strokes.js";

/** The parts of a 2D context the renderer uses (a browser's or @napi-rs/canvas's). */
export type Ctx = any;

const FONTS = new URL("./web/phone/fonts/", import.meta.url);

/** Registers the fonts with @napi-rs/canvas (once). */
export async function loadCanvas(): Promise<typeof import("@napi-rs/canvas")> {
  let mod: typeof import("@napi-rs/canvas");
  try {
    mod = await import("@napi-rs/canvas");
  } catch (err) {
    throw new Error(`the video screen needs @napi-rs/canvas (npm install in games/drawbluff): ${String(err)}`);
  }
  const f = (n: string) => fileURLToPath(new URL(n, FONTS));
  mod.GlobalFonts.registerFromPath(f("plex-sans.woff2"), "Plex Sans");
  mod.GlobalFonts.registerFromPath(f("michroma.woff2"), "Michroma");
  mod.GlobalFonts.registerFromPath(f("sixtyfour.woff2"), "Sixtyfour");
  return mod;
}

const C = {
  bg: "#2b1d5c",
  bg2: "#4a2f99",
  ink: "#fffdf6",
  accent: "#ffd60a",
  hot: "#ff5d8f",
  ok: "#2ec4b6",
  dark: "#1d1d1f",
  grey: "#4a4a55",
  paper: "#fffdf6",
};

const SANS = `"Plex Sans", sans-serif`;
const DISPLAY = `"Michroma", "Plex Sans", sans-serif`;
const LOGO = `"Sixtyfour", monospace`;

interface Frame {
  ctx: Ctx;
  w: number;
  h: number;
  /** 1% of the height (the web screen's `vh`). */
  vh: number;
  vw: number;
  /** Server time now (ms). */
  now: number;
}

/**
 * Draws one frame of the screen view. `now` is the party's clock
 * (`party.now()`), for countdowns; `t` the stream's time (animations).
 */
export function renderScreen(ctx: Ctx, view: Record<string, any>, t: number, now: number = Date.now()): void {
  const { width: w, height: h } = ctx.canvas as { width: number; height: number };
  const f: Frame = { ctx, w, h, vh: h / 100, vw: w / 100, now };
  const g = ctx.createRadialGradient(w * 0.15, h * 0.1, 0, w * 0.15, h * 0.1, w * 0.9);
  g.addColorStop(0, C.bg2);
  g.addColorStop(0.65, C.bg);
  g.addColorStop(1, C.bg);
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, w, h);
  const s = view["screen"] ?? {};
  switch (s.phase) {
    case "lobby":
      lobby(f, view, t);
      break;
    case "draw":
      draw(f, view);
      break;
    case "gallery":
      gallery(f, view);
      break;
    case "lie":
    case "vote":
      guess(f, view);
      break;
    case "reveal":
      reveal(f, view, t);
      break;
    case "scores":
    case "final":
      board(f, view);
      break;
    default:
      text(f, "drawbluff", w / 2, h / 2, { font: LOGO, size: 8 * f.vh, color: C.accent, align: "center", base: "middle" });
  }
  if (cornerShown(s.phase) && view["join"]?.qr) corner(f, view);
}

/** The corner's QR code (join or rejoin): not in the lobby, nor where the options fill the right side. */
export function cornerShown(phase: unknown): boolean {
  return typeof phase === "string" && phase !== "lobby" && phase !== "vote" && phase !== "reveal";
}

// ---------------------------------------------------------------------------
// Text

interface TextOpts {
  font?: string;
  size: number;
  color?: string;
  weight?: number;
  align?: "left" | "center" | "right";
  base?: "top" | "middle" | "alphabetic";
  max?: number;
}

function setFont(f: Frame, o: TextOpts): void {
  f.ctx.font = `${o.weight ?? 400} ${Math.round(o.size)}px ${o.font ?? SANS}`;
  f.ctx.fillStyle = o.color ?? C.ink;
  f.ctx.textAlign = o.align ?? "left";
  f.ctx.textBaseline = o.base ?? "top";
}

/** One line, shortened with … to `max` px. */
function text(f: Frame, s: string, x: number, y: number, o: TextOpts): void {
  setFont(f, o);
  let str = s;
  if (o.max !== undefined && f.ctx.measureText(str).width > o.max) {
    while (str.length > 1 && f.ctx.measureText(`${str}…`).width > o.max) str = str.slice(0, -1);
    str = `${str.trimEnd()}…`;
  }
  f.ctx.fillText(str, x, y);
}

/** Wrapped text; returns the height used. */
function para(f: Frame, s: string, x: number, y: number, width: number, o: TextOpts & { lineHeight?: number; maxLines?: number }): number {
  setFont(f, o);
  const words = s.split(/\s+/).filter(Boolean);
  const lines: string[] = [];
  let line = "";
  for (const word of words) {
    const next = line ? `${line} ${word}` : word;
    if (f.ctx.measureText(next).width > width && line) {
      lines.push(line);
      line = word;
    } else line = next;
  }
  if (line) lines.push(line);
  const lh = o.size * (o.lineHeight ?? 1.3);
  const shown = lines.slice(0, o.maxLines ?? lines.length);
  shown.forEach((l, i) => text(f, l, x, y + i * lh, { ...o, max: width }));
  return shown.length * lh;
}

function round(f: Frame, x: number, y: number, w: number, h: number, r: number, color: string): void {
  const c = f.ctx;
  c.fillStyle = color;
  c.beginPath();
  c.roundRect(x, y, w, h, r);
  c.fill();
}

// ---------------------------------------------------------------------------
// Phases

function lobby(f: Frame, view: Record<string, any>, t: number): void {
  const { vh, vw } = f;
  const join = view["join"] ?? {};
  const s = view["screen"] ?? {};
  const roster: any[] = view["roster"] ?? [];
  const cx = 4 * vw + 21 * vw;
  text(f, "drawbluff", cx, 6 * vh, { font: LOGO, size: 6.5 * vh, color: C.accent, align: "center" });
  text(f, "SCAN TO JOIN WITH YOUR PHONE", cx, 17 * vh, { size: 2.4 * vh, weight: 600, align: "center", color: "rgba(255,253,246,0.75)" });
  const q = 46 * vh;
  // A slow pulse draws the eye to the code.
  const pulse = 1 + 0.012 * Math.sin(t / 400);
  const qs = q * pulse;
  round(f, cx - qs / 2 - vh, 22 * vh - vh + (q - qs) / 2, qs + 2 * vh, qs + 2 * vh, 2 * vh, "#fff");
  drawQr(f.ctx, join.qr, cx - qs / 2, 22 * vh + (q - qs) / 2, qs, { quiet: 2 });
  para(f, join.url ?? "", cx, 72 * vh, 40 * vw, { size: 2.1 * vh, align: "center", color: "rgba(255,253,246,0.85)", maxLines: 2, lineHeight: 1.25 });
  text(f, (join.code ?? "").split("").join(" "), cx, 80 * vh, { font: DISPLAY, size: 3 * vh, color: C.accent, align: "center" });

  const x0 = 50 * vw;
  const colW = 22 * vw;
  text(f, "Players", x0, 6 * vh, { font: DISPLAY, size: 4.2 * vh });
  const max = s.maxPlayers ?? 8;
  const min = s.minPlayers ?? 3;
  if (roster.length > 8) {
    // A denser grid, up to 50 names.
    const cols = roster.length > 24 ? 5 : 3;
    const gap = 0.8 * vw;
    const cw = (46 * vw - (cols - 1) * gap) / cols;
    const rows = Math.ceil(roster.length / cols);
    const ch = Math.min(8.6 * vh, (64 * vh - (rows - 1) * 0.8 * vh) / rows);
    const size = Math.min(3.4 * vh, ch * 0.55, cols === 5 ? 2.6 * vh : 3 * vh);
    roster.forEach((r, i) => {
      const x = x0 + (i % cols) * (cw + gap);
      const y = 16 * vh + Math.floor(i / cols) * (ch + 0.8 * vh);
      round(f, x, y, cw, ch, 1 * vh, r.connected ? "rgba(255,255,255,0.12)" : "rgba(255,255,255,0.06)");
      text(f, `${r.vip ? "★ " : ""}${r.name}`, x + 0.8 * vw, y + ch / 2, { size, weight: 600, base: "middle", max: cw - 1.6 * vw, color: r.connected ? C.ink : "rgba(255,253,246,0.45)" });
    });
  }
  for (let i = 0; i < Math.min(max, 8) && roster.length <= 8; i++) {
    const x = x0 + (i % 2) * (colW + 1.6 * vw);
    const y = 16 * vh + Math.floor(i / 2) * 10.5 * vh;
    const r = roster[i];
    round(f, x, y, colW, 8.6 * vh, 1.6 * vh, r ? (r.connected ? "rgba(255,255,255,0.12)" : "rgba(255,255,255,0.06)") : "rgba(255,255,255,0.04)");
    if (r) {
      text(f, r.name, x + 1.6 * vw, y + 2.4 * vh, { size: 3.4 * vh, weight: 600, max: colW - (r.vip ? 9 : 3) * vw, color: r.connected ? C.ink : "rgba(255,253,246,0.45)" });
      if (r.vip) text(f, "★ VIP", x + colW - 1.4 * vw, y + 3.1 * vh, { size: 2.4 * vh, color: C.accent, align: "right" });
    } else {
      text(f, i < min ? "waiting…" : "room for one more", x + 1.6 * vw, y + 2.8 * vh, { size: 2.6 * vh, color: "rgba(255,253,246,0.3)" });
    }
  }
  const ready = roster.filter((r) => r.connected).length;
  const vip = roster.find((r) => r.vip)?.name ?? "The VIP";
  const hint = ready < min ? `${min - ready} more player${min - ready === 1 ? "" : "s"} needed.` : `${vip} starts the game on their phone.`;
  para(f, hint, x0, roster.length > 8 ? 84 * vh : 61 * vh, 46 * vw, { size: 3 * vh });
}

/** Chips up to this many players waited for; more: a count, a bar and the last few names. */
const CHIPS = 12;

function chips(
  f: Frame,
  view: Record<string, any>,
  x: number,
  y: number,
  width: number,
  align: "left" | "center" = "left",
  list = "waiting",
  verb = "done",
): void {
  const s = view["screen"] ?? {};
  const roster: any[] = view["roster"] ?? [];
  const done = new Set<string>(s.done ?? []);
  const { vh, vw } = f;
  const waiting: string[] = s[list] ?? [];
  if (waiting.length > CHIPS) {
    const n = waiting.filter((id) => done.has(id)).length;
    const left = waiting.filter((id) => !done.has(id)).map((id) => roster.find((r) => r.id === id)?.name ?? "?");
    const bw = Math.min(width, 50 * vw);
    const bx = align === "center" ? x + (width - bw) / 2 : x;
    text(f, `${n} of ${waiting.length} ${verb}`, align === "center" ? x + width / 2 : x, y, { font: DISPLAY, size: 3 * vh, align: align === "center" ? "center" : "left" });
    round(f, bx, y + 5 * vh, bw, 1.6 * vh, 0.8 * vh, "rgba(255,255,255,0.12)");
    if (n > 0) round(f, bx, y + 5 * vh, (bw * n) / waiting.length, 1.6 * vh, 0.8 * vh, C.ok);
    if (left.length > 0 && left.length <= 6) {
      text(f, `Waiting for ${left.join(", ")}`, align === "center" ? x + width / 2 : x, y + 8.4 * vh, { size: 2.3 * vh, align: align === "center" ? "center" : "left", max: width, color: "rgba(255,253,246,0.85)" });
    }
    return;
  }
  const items = waiting.map((id: string) => {
    const name = roster.find((r) => r.id === id)?.name ?? "?";
    return { label: `${done.has(id) ? "✓ " : ""}${name}`, done: done.has(id) };
  });
  setFont(f, { size: 2.4 * vh });
  const pad = 1.6 * vw;
  const gap = 1 * vh;
  const rows: { label: string; done: boolean; w: number }[][] = [[]];
  let lineW = 0;
  for (const it of items) {
    const w = Math.min(width, f.ctx.measureText(it.label).width + 2 * pad);
    if (lineW + w > width && rows.at(-1)!.length > 0) {
      rows.push([]);
      lineW = 0;
    }
    rows.at(-1)!.push({ ...it, w });
    lineW += w + gap;
  }
  rows.forEach((row, ri) => {
    const total = row.reduce((s, c) => s + c.w + gap, -gap);
    let cx = align === "center" ? x + (width - total) / 2 : x;
    for (const c of row) {
      const cy = y + ri * 5.6 * vh;
      round(f, cx, cy, c.w, 4.4 * vh, 2.2 * vh, c.done ? C.ok : "rgba(255,255,255,0.1)");
      text(f, c.label, cx + pad, cy + 0.9 * vh, { size: 2.4 * vh, color: c.done ? "#10302d" : C.ink, max: c.w - 2 * pad });
      cx += c.w + gap;
    }
  });
}

function timer(f: Frame, deadline: number | undefined, cx: number, cy: number): void {
  if (typeof deadline !== "number") return;
  const left = Math.max(0, Math.ceil((deadline - f.now) / 1000));
  const r = 6.5 * f.vh;
  f.ctx.fillStyle = left <= 10 ? C.hot : "rgba(255,255,255,0.12)";
  f.ctx.beginPath();
  f.ctx.arc(cx, cy, r, 0, Math.PI * 2);
  f.ctx.fill();
  text(f, String(left), cx, cy, { font: DISPLAY, size: 5.4 * f.vh, align: "center", base: "middle" });
}

function draw(f: Frame, view: Record<string, any>): void {
  const s = view["screen"];
  const { w, vh, vw } = f;
  text(f, `ROUND ${s.round} OF ${s.rounds}`, w / 2, 16 * vh, { size: 2.4 * vh, weight: 600, align: "center", color: "rgba(255,253,246,0.7)" });
  para(f, "Draw your prompt on your phone!", w / 2, 24 * vh, 70 * vw, { font: DISPLAY, size: 6 * vh, align: "center" });
  timer(f, s.deadline, w / 2, 56 * vh);
  chips(f, view, 15 * vw, 68 * vh, 70 * vw, "center");
}

function drawingBox(f: Frame, drawing: unknown): number {
  const { vh, vw } = f;
  const size = 86 * vh;
  f.ctx.save();
  f.ctx.shadowColor = "rgba(0,0,0,0.35)";
  f.ctx.shadowBlur = 4 * vh;
  f.ctx.shadowOffsetY = 2 * vh;
  round(f, 4 * vw, 5 * vh, size, size, 2 * vh, C.paper);
  f.ctx.restore();
  f.ctx.save();
  f.ctx.beginPath();
  f.ctx.roundRect(4 * vw, 5 * vh, size, size, 2 * vh);
  f.ctx.clip();
  drawDrawing(f.ctx, drawing as never, 4 * vw, 5 * vh, size);
  f.ctx.restore();
  return 4 * vw + size + 4 * vw;
}

/** The gallery: the round's drawings that do not get the stage, 4×2 a page. */
function gallery(f: Frame, view: Record<string, any>): void {
  const s = view["screen"];
  const { vh, vw } = f;
  const pages = s.pages > 1 ? ` · ${s.page} OF ${s.pages}` : "";
  text(f, `ROUND ${s.round} · THE GALLERY${pages}`, 4 * vw, 5 * vh, { size: 2.4 * vh, weight: 600, color: "rgba(255,253,246,0.7)" });
  const items: any[] = s.items ?? [];
  const cw = (92 * vw - 3 * 2 * vw) / 4;
  const size = Math.min(cw, 29 * vh);
  items.slice(0, 8).forEach((it, i) => {
    const cx = 4 * vw + (i % 4) * (cw + 2 * vw) + cw / 2;
    const y = 10 * vh + Math.floor(i / 4) * (size + 9 * vh);
    round(f, cx - size / 2, y, size, size, 1.4 * vh, C.paper);
    f.ctx.save();
    f.ctx.beginPath();
    f.ctx.roundRect(cx - size / 2, y, size, size, 1.4 * vh);
    f.ctx.clip();
    drawDrawing(f.ctx, it.drawing, cx - size / 2, y, size);
    f.ctx.restore();
    text(f, it.prompt ?? "", cx, y + size + 1 * vh, { size: 2.2 * vh, weight: 600, color: C.accent, align: "center", max: cw });
    text(f, `by ${it.name ?? "?"}`, cx, y + size + 4 * vh, { size: 2 * vh, align: "center", max: cw, color: "rgba(255,253,246,0.8)" });
  });
  const liars = (s.liars ?? []).length;
  if (liars > 0) text(f, meanwhile(s), 4 * vw, 86 * vh, { size: 2.6 * vh, max: 70 * vw });
}

/** The gallery's line about the first drawing's liars. */
export function meanwhile(s: any): string {
  const liars = (s.liars ?? []).length;
  const done = (s.done ?? []).length;
  return `Meanwhile ${liars === 1 ? "1 liar is" : `${liars} liars are`} writing for ${s.artist?.name ?? "the first"}'s drawing · ${done} done`;
}

function guess(f: Frame, view: Record<string, any>): void {
  const s = view["screen"];
  const { w, vh, vw } = f;
  const x = drawingBox(f, s.drawing);
  const width = w - x - 4 * vw;
  const spot = s.spots > 1 ? ` · DRAWING ${s.spot} OF ${s.spots}` : "";
  if (s.phase === "lie") {
    const liars = (s.waiting ?? []).length;
    text(f, `${(s.artist?.name ?? "Someone").toUpperCase()} DREW THIS${spot}`, x, 6 * vh, { size: 2.4 * vh, weight: 600, color: "rgba(255,253,246,0.7)", max: width });
    const what = liars === 1 ? "What is it called? The liar is writing a fake title." : `What is it called? ${liars} liars are writing fake titles.`;
    const used = para(f, what, x, 11 * vh, width, { font: DISPLAY, size: 4.2 * vh, lineHeight: 1.35 });
    timer(f, s.deadline, x + 6.5 * vh, 11 * vh + used + 9 * vh);
    chips(f, view, x, 11 * vh + used + 19 * vh, width);
    return;
  }
  text(f, `WHICH ONE IS THE REAL TITLE?${spot}`, x, 6 * vh, { size: 2.4 * vh, weight: 600, color: "rgba(255,253,246,0.7)", max: width });
  const opts: { text: string }[] = s.options ?? [];
  const oh = Math.min(9 * vh, (60 * vh) / Math.max(1, opts.length));
  opts.forEach((o, i) => {
    const y = 11 * vh + i * (oh + 1.4 * vh);
    round(f, x, y, width, oh, 1.6 * vh, C.paper);
    text(f, o.text, x + 1.6 * vw, y + oh / 2, { size: Math.min(3.4 * vh, oh * 0.45), weight: 600, color: C.dark, base: "middle", max: width - 3.2 * vw });
  });
  const y = 11 * vh + opts.length * (oh + 1.4 * vh) + 7 * vh;
  timer(f, s.deadline, x + 6.5 * vh, Math.min(y, 87 * vh));
  chips(f, view, x + 15 * vh, Math.min(y, 87 * vh) - 2.2 * vh, width - 15 * vh, "left", "waiting", "voted");
  if (s.writing > 0) text(f, `Meanwhile ${s.writing} liars write for the next drawing.`, x, 95 * vh, { size: 2.2 * vh, max: width - 18 * vw, color: "rgba(255,253,246,0.8)", base: "alphabetic" });
}

function reveal(f: Frame, view: Record<string, any>, t: number): void {
  const s = view["screen"];
  const { w, vh, vw } = f;
  const x = drawingBox(f, s.drawing);
  const width = w - x - 4 * vw;
  text(f, "THE ANSWERS", x, 6 * vh, { size: 2.4 * vh, weight: 600, color: "rgba(255,253,246,0.7)" });
  const items: any[] = s.items ?? [];
  const step: number = s.step ?? 0;
  const ih = Math.min(13 * vh, (78 * vh) / Math.max(1, items.length) - 1.4 * vh);
  items.slice(0, step).forEach((it, i) => {
    const y = 11 * vh + i * (ih + 1.4 * vh);
    round(f, x, y, width, ih, 1.6 * vh, it.truth ? C.accent : C.paper);
    const tag = it.truth ? `THE TRUTH by ${s.artist?.name ?? "?"}` : it.decoy ? "A LIE" : `A LIE by ${it.authors.join(" & ")}`;
    text(f, tag, x + 1.6 * vw, y + 1.2 * vh, { font: DISPLAY, size: 1.9 * vh, color: it.truth ? C.dark : C.hot, max: width - 3.2 * vw });
    text(f, it.text, x + 1.6 * vw, y + 4 * vh, { size: Math.min(3.4 * vh, ih * 0.3), weight: 600, color: C.dark, max: width - 3.2 * vw });
    const who = revealLine(it, s.artist?.name);
    text(f, who, x + 1.6 * vw, y + ih - 3.6 * vh, { size: 2.2 * vh, color: C.grey, max: width - 3.2 * vw });
  });
  void t;
}

/** A reveal line's last row: who fell for it (names while they fit, else how many of those who saw it). */
export function revealLine(it: any, artist: string | undefined): string {
  const count: number = it.count ?? it.voters.length;
  const fits = count <= it.voters.length;
  const names = fits ? it.voters.join(", ") : `${count} of ${it.seen}`;
  if (it.truth) return count ? `found by ${names} · +${it.points} each, ${artist ?? "?"} +${it.artistPoints}` : "nobody found it";
  return `${fits ? `${names} fell for it` : `fooled ${names}`}${it.decoy ? " (our lie)" : ` · +${it.points}${it.same ? " (same idea!)" : ""}`}`;
}

function board(f: Frame, view: Record<string, any>): void {
  const s = view["screen"];
  const { w, vh, vw } = f;
  const final = s.phase === "final";
  const all: any[] = s.scores ?? [];
  // All players up to 8, else the top 5 (each phone shows its own rank).
  const total: number = s.total ?? all.length;
  const scores = total > 8 ? all.slice(0, 5) : all;
  const highlights: any[] = s.highlights ?? [];
  const title = final ? `${scores[0]?.name ?? "Nobody"} wins!` : s.last ? "Final scores" : `Scores after round ${s.round}`;
  text(f, title, w / 2, 8 * vh, { font: DISPLAY, size: 6 * vh, align: "center", max: 90 * vw });
  const lw = highlights.length ? 60 * vw : 70 * vw;
  const x = highlights.length ? 4 * vw : (w - lw) / 2;
  highlights.forEach((hl, i) => {
    const hx = x + lw + 3 * vw;
    const hy = 20 * vh + i * 15 * vh;
    const hw = w - hx - 4 * vw;
    round(f, hx, hy, hw, 13 * vh, 1.6 * vh, "rgba(255,255,255,0.1)");
    text(f, hl.kind === "liar" ? "BEST LIAR" : "SHARPEST EYE", hx + 1.6 * vw, hy + 1.6 * vh, { font: DISPLAY, size: 1.9 * vh, color: C.accent });
    text(f, hl.name, hx + 1.6 * vw, hy + 4.6 * vh, { size: 3.4 * vh, weight: 600, max: hw - 3.2 * vw });
    text(f, hl.kind === "liar" ? `fooled ${hl.count}` : `found ${hl.count} of ${hl.of}`, hx + 1.6 * vw, hy + 9 * vh, { size: 2.4 * vh, max: hw - 3.2 * vw });
  });
  if (total > scores.length) text(f, `${total} players · your rank is on your phone`, w / 2, 84 * vh, { size: 2.6 * vh, align: "center" });
  const lh = Math.min(8.4 * vh, (66 * vh) / Math.max(1, scores.length));
  scores.forEach((l, i) => {
    const y = 20 * vh + i * (lh + 1.2 * vh);
    const first = i === 0;
    round(f, x, y, lw, lh, 1.6 * vh, first ? C.accent : "rgba(255,255,255,0.08)");
    const color = first ? C.dark : C.ink;
    const mid = y + lh / 2;
    text(f, `${i + 1}.`, x + 2 * vw, mid, { font: DISPLAY, size: 3 * vh, color, base: "middle" });
    text(f, l.name, x + 7 * vw, mid, { size: 3.6 * vh, weight: 600, color, base: "middle", max: lw - 30 * vw });
    if (!final && l.delta) text(f, `+${l.delta}`, x + lw - 14 * vw, mid, { size: 2.6 * vh, color: first ? C.dark : C.ok, base: "middle", align: "right" });
    text(f, String(l.score), x + lw - 2 * vw, mid, { font: DISPLAY, size: 3.4 * vh, color, base: "middle", align: "right" });
  });
  if (final) text(f, "The VIP can start another game on their phone.", w / 2, 92 * vh, { size: 3 * vh, align: "center" });
}

function corner(f: Frame, view: Record<string, any>): void {
  const { w, h, vh, vw } = f;
  const size = 12 * vh;
  const bw = size + 16 * vw;
  const x = w - 2 * vw - bw;
  const y = h - 3 * vh - size - 2 * vh;
  round(f, x, y, bw, size + 2 * vh, 1.4 * vh, "rgba(0,0,0,0.35)");
  drawQr(f.ctx, view["join"].qr, x + vh, y + vh, size, { quiet: 2 });
  para(f, "Scan to join (from the next round) or to rejoin.", x + size + 2 * vh, y + 2 * vh, bw - size - 3 * vh, { size: 1.8 * vh, lineHeight: 1.25, color: "rgba(255,253,246,0.85)" });
}
