// @ts-check
/**
 * The lobby: the wooden JINX tiles, the QR code taped to the table, and
 * the seats. A newcomer's chip drops onto its seat, the place card flips
 * up and the host greets them.
 *
 * @typedef {import("../game/views.ts").LobbyScreen} LobbyScreen
 * @typedef {import("./tv.js").Ctx} Ctx
 */

import { A, EASE, fitText, h, short } from "./dom.js";
import { dust } from "./fx.js";
import { chip } from "./pieces.js";
import { sfx } from "./sound.js";

/** @param {number} i */
const seatPos = (i) => ({ x: 588 + (i % 4) * 157, y: 158 + Math.floor(i / 4) * 214 });

const GREET = [
  (/** @type {string} */ n) => `Welcome, *${n}*. Mind your words.`,
  (/** @type {string} */ n) => `*${n}* is in. The plot thickens.`,
  (/** @type {string} */ n) => `Here comes *${n}*!`,
  (/** @type {string} */ n) => `*${n}* pulls up a chair.`,
];

/**
 * @param {Ctx} ctx
 * @param {LobbyScreen} v
 * @param {import("../game/views.ts").ScreenView | null} prev
 */
export function lobby(ctx, v, prev) {
  const L = ctx.layer;
  const anim = !!prev;
  ctx.head.set({}, false);
  ctx.host.clear();
  ctx.pot.show(false);
  ctx.pool.off(anim);

  const logo = h("div", { class: "logo", style: "left:64px;top:50px" });
  /** @type {[string, number, number][]} */
  const tiles = [["J", 8, -4], ["I", 1, 3], ["N", 1, -2], ["X", 8, 4]];
  for (const [l, n, r] of tiles) logo.append(h("div", { class: "tile", style: `transform:rotate(${r}deg)` }, l, h("sub", null, n)));
  L.append(logo);
  if (anim || prev === null) {
    [...logo.children].forEach((t, i) => A(t, [{ transform: "translateY(-200px) rotate(30deg)", opacity: 0 }, { transform: /** @type {HTMLElement} */ (t).style.transform, opacity: 1 }], 420, { delay: 100 + i * 110, easing: EASE.back }));
    [0, 1, 2, 3].forEach((i) => ctx.later(240 + i * 110, sfx.thunk));
  }
  L.append(h("div", { class: "tagline", style: "left:66px;top:174px" }, "One word each. ", h("b", null, "Don't think alike.")));

  const qrBox = h("div", { class: "qrbox" });
  const code = h("div", { class: "code" });
  L.append(h("div", { class: "qrcard paper", style: "left:96px;top:236px;transform:rotate(-2deg)" }, qrBox, h("div", { class: "roomlbl" }, "ROOM CODE"), code));
  L.append(h("div", { class: "tapebit", style: "left:70px;top:226px;transform:rotate(-38deg)" }));
  L.append(h("div", { class: "tapebit", style: "left:342px;top:226px;transform:rotate(34deg)" }));
  const url = h("div", { class: "url", style: "left:64px;top:640px;width:460px" });
  L.append(url);

  const count = h("span", { class: "count" });
  L.append(h("div", { class: "abs", style: "left:588px;top:52px;font-size:32px;font-weight:700;letter-spacing:.12em" }, "AT THE TABLE ", count));
  const sub = h("div", { class: "abs", style: "left:588px;top:94px;font-size:24px;color:#a8d3cd;font-weight:600" });
  L.append(sub);
  const seats = h("div", { class: "abs", style: "inset:0" });
  L.append(seats);
  const slip = h("div", { class: "banner paper", style: "left:900px;top:604px;transform:translateX(-50%) rotate(-1deg)" });
  L.append(slip);

  /** @type {string[]} */
  let shown = [];
  let joinKey = "";
  let greetUntil = 0;
  let greetN = 0;
  let slipText = "";

  /** @param {string} s */
  const setSlip = (s, pop = false) => {
    if (s === slipText) return;
    slipText = s;
    slip.replaceChildren(...s.split(/(\*[^*]+\*)/).filter(Boolean).map((p) => (p.startsWith("*") ? h("b", null, p.slice(1, -1)) : p)));
    if (pop) A(slip, [{ transform: "translateX(-50%) rotate(-1deg) scale(1.15)" }, { transform: "translateX(-50%) rotate(-1deg) scale(1)" }], 260, { easing: EASE.back });
  };

  /** @param {LobbyScreen} v @param {boolean} anim */
  const render = (v, anim) => {
    const j = ctx.join;
    const jk = `${j?.code}|${j?.url}`;
    if (jk !== joinKey) {
      joinKey = jk;
      qrBox.innerHTML = ctx.qr(2);
      code.textContent = j?.code ?? "";
      url.textContent = (j?.url ?? "").replace(/^https?:\/\//, "");
      fitText(url, 26, 24, 460, 0.6);
    }
    const max = Math.min(8, v.maxPlayers || 8);
    const people = v.people.filter((p) => !p.left).slice(0, 8);
    count.textContent = `${people.length} / ${max}`;
    sub.textContent = `${v.minPlayers} to ${max} players · draw your face on your phone`;
    const ids = people.map((p) => `${p.id}:${p.name}:${JSON.stringify(p.face)}:${p.vip}:${p.connected}:${p.color}:${!!p.bot}`);
    if (ids.join("|") !== shown.join("|")) {
      const before = new Set(shown.map((s) => s.split(":")[0]));
      const faceBefore = new Map(shown.map((s) => [s.split(":")[0], s]));
      seats.replaceChildren();
      let newcomer = "";
      people.forEach((p, i) => {
        const pos = seatPos(i);
        const t = chip(p, 100, { style: "left:25px;top:0", crown: p.vip });
        const pc = h("div", { class: "placecard paper" }, short(p.name, 10));
        seats.append(h("div", { class: "seat", style: `left:${pos.x}px;top:${pos.y}px` }, t, pc));
        if (!anim) return;
        if (!before.has(p.id)) {
          newcomer = p.name;
          A(t, [
            { transform: "translateY(-120px) scale(1.6)", opacity: 0 },
            { offset: 0.6, transform: "translateY(0) scale(1)", opacity: 1, easing: EASE.out },
            { offset: 0.78, transform: "translateY(0) scale(1.1,.88)" },
            { transform: "translateY(0) scale(1)", opacity: 1 },
          ], 440, { easing: EASE.in });
          A(pc, [{ transform: "perspective(300px) rotateX(-90deg)", opacity: 0 }, { transform: "perspective(300px) rotateX(0)", opacity: 1 }], 320, { delay: 300, easing: EASE.back });
          ctx.later(260, () => {
            sfx.thunk();
            dust(pos.x + 75, pos.y + 96, 6, 70);
          });
          ctx.later(380, sfx.flipTick);
          A(count, [{ transform: "scale(1.4)" }, { transform: "scale(1)" }], 300, { delay: 260 });
        } else if (faceBefore.get(p.id) !== ids[i]) {
          // a new face or colour: a little flip
          A(t, [{ transform: "rotateY(90deg)" }, { transform: "none" }], 260, { easing: EASE.back });
        }
      });
      for (let i = people.length; i < max; i++) {
        const pos = seatPos(i);
        seats.append(h("div", { class: "seat empty", style: `left:${pos.x}px;top:${pos.y}px` }, h("div", { class: "token ghost", style: "--s:100px;left:25px;top:0" }), h("div", { class: "placecard" }, "open seat")));
      }
      shown = ids;
      if (newcomer) {
        setSlip((GREET[greetN++ % GREET.length] ?? GREET[0])?.(short(newcomer, 12)) ?? "", true);
        greetUntil = Date.now() + 2600;
        ctx.later(2700, () => render(/** @type {any} */ (ctx.tv.prev ?? v), true));
      }
    }
    if (Date.now() < greetUntil) return;
    const ready = people.filter((p) => p.connected).length;
    const vip = people.find((p) => p.vip);
    if (people.length === 0) setSlip("Phones out. Scan in. *No word is safe.*");
    else if (ready < v.minPlayers) setSlip(`Waiting for *${v.minPlayers - ready} more* to play`);
    else setSlip(vip ? `*${short(vip.name, 12)}* starts the game from their phone` : "Start the game from a phone");
  };

  render(v, anim);
  return {
    /** @param {LobbyScreen} nv */
    update(nv) {
      render(nv, true);
    },
  };
}

