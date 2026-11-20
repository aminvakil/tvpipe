// @ts-check
/**
 * Faces: the eight ready-made faces (the tabletop mockup's, in its player
 * order: Anna, Ben, Chiara, Dev, Emeka, Fatima, Gus, Hiroko) and doodles
 * (strokes on a 1000×1000 grid, one ink colour). The TV draws the same
 * faces; keep `PRESETS`, `presetMarkup` and `DOODLE_WIDTH` in step with it.
 *
 * @typedef {import("../game/views.ts").Face} Face
 */

export const INK = "#1d1a2b";
/** The chip colours, `Person.color` 0–7. */
export const COLORS = ["#ff7a5c", "#4aa8f5", "#8bd648", "#ffc63d", "#b48fff", "#ff8cc6", "#ff9845", "#3fd9c7"];
export const GRID = 1000;
/** A doodle's line width on the 1000 grid. */
export const DOODLE_WIDTH = 48;
export const MAX_STROKES = 40;
export const MAX_POINTS = 2000;

/** Eyes, mouth, extra. */
export const PRESETS = [
  ["dot", "smile", "blush"],
  ["dot", "flat", "glasses"],
  ["happy", "open", "tuft"],
  ["dot", "flat", "stache"],
  ["sleepy", "o", "brows"],
  ["wink", "smile", "bow"],
  ["wide", "grin", "brows"],
  ["happy", "smile", "swoop"],
];

const SVG = "http://www.w3.org/2000/svg";

/**
 * A preset's SVG content (constant markup, viewBox 0 0 100 100).
 * @param {number} n
 */
export function presetMarkup(n) {
  const f = PRESETS[((n % PRESETS.length) + PRESETS.length) % PRESETS.length] ?? PRESETS[0] ?? [];
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
  return s;
}

/**
 * One doodle stroke as path data (numbers only; anything else is dropped).
 * @param {unknown} stroke
 */
export function strokePath(stroke) {
  if (!Array.isArray(stroke)) return "";
  /** @type {number[]} */
  const p = [];
  for (const v of stroke) {
    const n = Number(v);
    if (!Number.isFinite(n)) return "";
    p.push(Math.round(Math.min(GRID, Math.max(0, n))));
  }
  if (p.length < 2) return "";
  let d = `M${p[0]} ${p[1]}`;
  if (p.length < 4) return `${d}l0 0`;
  for (let i = 2; i + 1 < p.length; i += 2) d += `L${p[i]} ${p[i + 1]}`;
  return d;
}

/**
 * A face as an `<svg>` (the cream centre of a chip is the svg's background).
 * @param {Face | null | undefined} face
 * @returns {SVGSVGElement}
 */
export function faceSvg(face) {
  const svg = /** @type {SVGSVGElement} */ (document.createElementNS(SVG, "svg"));
  svg.setAttribute("aria-hidden", "true");
  if (face && "strokes" in face && Array.isArray(face.strokes)) {
    svg.setAttribute("viewBox", `0 0 ${GRID} ${GRID}`);
    const path = document.createElementNS(SVG, "path");
    path.setAttribute("d", face.strokes.slice(0, MAX_STROKES).map(strokePath).join(""));
    path.setAttribute("fill", "none");
    path.setAttribute("stroke", INK);
    path.setAttribute("stroke-width", String(DOODLE_WIDTH));
    path.setAttribute("stroke-linecap", "round");
    path.setAttribute("stroke-linejoin", "round");
    svg.append(path);
  } else {
    svg.setAttribute("viewBox", "0 0 100 100");
    const n = face && "preset" in face && Number.isInteger(face.preset) ? face.preset : 0;
    svg.innerHTML = presetMarkup(n);
  }
  return svg;
}

/** The flame sticker's drawing (viewBox 0 0 40 40). */
export const FLAME = `<svg viewBox="0 0 40 40" aria-hidden="true"><path d="M20 3 C22 11 31 14 31 25 A11 11 0 0 1 9 25 C9 18 14 15 15 9 C17 13 18 15 20 17 C21 12 20 7 20 3 Z" fill="#df4a3c" stroke="${INK}" stroke-width="2.5" stroke-linejoin="round"/><path d="M20 21 C22 25 25 26 25 29.5 A5 5 0 0 1 15 29.5 C15 26.5 18 25 20 21 Z" fill="#ffe066"/></svg>`;
