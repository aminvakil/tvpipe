// @ts-check
/**
 * Where the cards lie. The card area is x 64–980 (the pot is on the
 * right). A player's two cards (3 players) lie side by side under one
 * name tab.
 *
 * @typedef {import("../game/views.ts").Card} Card
 * @typedef {{ x: number, y: number, w: number, h: number, who: boolean, whoLeft?: number }} Slot
 */

export const AREA = { x: 64, w: 916 };

/**
 * Cards grouped by author when someone has two (3 players), in first-seen order.
 * @param {Card[]} cards
 * @returns {Card[][]}
 */
export function units(cards) {
  /** @type {Map<string, Card[]>} */
  const by = new Map();
  for (const c of cards) {
    const u = by.get(c.author);
    if (u) u.push(c);
    else by.set(c.author, [c]);
  }
  return [...by.values()];
}

/**
 * Rows of cards: up to 4 per row, two rows for 5 or more; pairs together.
 * @param {Card[]} cards
 * @param {{ big?: boolean, top?: number }} [o] big: few cards larger (the reveal)
 * @returns {Map<string, Slot>}
 */
export function grid(cards, { big = false, top } = {}) {
  /** @type {Map<string, Slot>} */
  const out = new Map();
  const n = cards.length;
  const us = units(cards);
  const paired = us.some((u) => u.length > 1);
  let w = 210;
  let hh = 150;
  if (big && n <= 3) {
    w = 240;
    hh = 165;
  } else if (n === 4 && paired) {
    w = 200;
    hh = 144;
  }
  const gap = big && n <= 3 ? 50 : 26;
  const pairGap = 34;
  /** @type {Card[][][]} rows of units */
  const rows = [];
  if (n <= 4) rows.push(us);
  else {
    // fill the first row with up to 4 cards (pairs never split)
    const first = [];
    let k = 0;
    const rest = [];
    for (const u of us) {
      if (rest.length === 0 && k + u.length <= 4) {
        first.push(u);
        k += u.length;
      } else rest.push(u);
    }
    rows.push(first, rest);
  }
  const ys = rows.length === 1 ? [top ?? (big ? 200 : 236)] : [top ?? 176, (top ?? 176) + 216];
  rows.forEach((row, ri) => {
    const unitGap = paired ? Math.max(gap, 44) : gap;
    const total = row.reduce((s, u) => s + u.length * w + (u.length - 1) * pairGap, 0) + (row.length - 1) * unitGap;
    let x = AREA.x + (AREA.w - total) / 2;
    for (const u of row) {
      u.forEach((c, i) => {
        /** @type {Slot} */
        const s = { x: Math.round(x), y: ys[ri] ?? 176, w, h: hh, who: i === 0 };
        if (u.length > 1 && i === 0) s.whoLeft = w + pairGap / 2;
        out.set(c.id, s);
        x += w + (i < u.length - 1 ? pairGap : 0);
      });
      x += unitGap;
    }
  });
  return out;
}
