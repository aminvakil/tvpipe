// @ts-check
/**
 * Jinx's phone page. It shows `view.room` (RoomView: the phase and the
 * people) and `view.me` (PlayerState: this phone's task) and sends the
 * actions of game/views.ts; the guesser's draft goes as `input({ draft })`.
 * The look is the TV's table: felt, cream paper, tomato card backs, chips.
 * Everything players typed is shown as text (`textContent`), never as markup.
 *
 * @typedef {import("../game/views.ts").RoomView} RoomView
 * @typedef {import("../game/views.ts").PlayerState} PlayerState
 * @typedef {import("../game/views.ts").Person} Person
 * @typedef {import("../game/views.ts").Card} Card
 * @typedef {import("../game/views.ts").Face} Face
 * @typedef {import("../game/views.ts").Reason} Reason
 * @typedef {import("../game/views.ts").Payout} Payout
 * @typedef {import("../game/views.ts").Award} Award
 * @typedef {import("../game/views.ts").RoomTitle} RoomTitle
 */
/**
 * @template {PlayerState["task"]} T
 * @typedef {Extract<PlayerState, { task: T }>} Task
 */
/**
 * What is on screen: the scrolling part, the part docked at the bottom
 * (fields stay right above the on-screen keyboard), and what to update on
 * every view while the screen stays.
 * @typedef {{ main: (Node | null)[], dock?: (Node | null)[], refresh?: (me: PlayerState, room: RoomView) => void, tone?: string }} Screen
 */

import { PartyClient } from "/_tvpipe/lib/client.js";
import { COLORS, FLAME, faceSvg, PRESETS } from "./faces.js";
import { Pad } from "./pad.js";

/** The longest clue and guess (the matcher's limit). */
const MAX_WORD = 24;
const NAME_KEY = "jinx:name";

// ---------------------------------------------------------------------------
// A game controller, not a document: no pinch or double-tap zoom, no text
// selection outside the fields (iOS ignores `user-scalable=no`).

for (const type of ["gesturestart", "gesturechange", "gestureend"]) {
  document.addEventListener(type, (e) => e.preventDefault(), { passive: false });
}
document.addEventListener(
  "touchmove",
  (e) => {
    if (e.touches.length > 1) e.preventDefault();
  },
  { passive: false },
);
document.addEventListener("dblclick", (e) => e.preventDefault(), { passive: false });
document.addEventListener("selectstart", (e) => {
  const t = e.target instanceof Element ? e.target : /** @type {Node} */ (e.target).parentElement;
  if (!t?.closest("input, textarea")) e.preventDefault();
});

// ---------------------------------------------------------------------------
// DOM helpers: text always as text.

/** @typedef {Node | string | number | null | undefined | false} Kid */

/**
 * @param {string} tag
 * @param {Record<string, any>} [attrs]
 * @param {...Kid} kids
 * @returns {HTMLElement}
 */
function h(tag, attrs = {}, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2), v);
    else if (k === "class") el.className = v;
    else if (k === "style") el.style.cssText = v;
    else if (v === true) el.setAttribute(k, "");
    else if (v !== false && v != null) el.setAttribute(k, String(v));
  }
  for (const kid of kids) if (kid !== null && kid !== undefined && kid !== false) el.append(typeof kid === "number" ? String(kid) : kid);
  return el;
}

/**
 * A button (type=button).
 * @param {string} cls
 * @param {() => void} onclick
 * @param {...Kid} kids
 */
function btn(cls, onclick, ...kids) {
  return /** @type {HTMLButtonElement} */ (h("button", { type: "button", class: cls, onclick }, ...kids));
}

/** @param {string} markup constant markup of this file only */
function icon(markup) {
  const s = h("span", { class: "icon" });
  s.innerHTML = markup;
  return s;
}

/** @param {number[]} pattern */
function buzz(pattern) {
  try {
    // Only after a tap on the page (browsers refuse it before, with a warning).
    if (navigator.userActivation && !navigator.userActivation.hasBeenActive) return;
    navigator.vibrate?.(pattern);
  } catch {}
}

// ---------------------------------------------------------------------------
// The shell

const app = h("div", { id: "app" });
const header = h("header", { id: "top" });
/** The wooden letter tiles. @param {boolean} [large] */
function logo(large = false) {
  return h("div", { class: `tiles${large ? " big" : ""}`, role: "img", "aria-label": "Jinx" }, ...["J", "I", "N", "X"].map((l, i) => h("b", { style: `--r:${[-4, 3, -2, 5][i]}deg` }, l, h("sub", {}, [8, 1, 1, 8][i]))));
}
const tiles = logo();
const timer = h("div", { id: "timer", class: "dial", hidden: true, "aria-live": "off" });
const meBox = h("div", { id: "me" });
header.append(tiles, timer, meBox);
const banner = h("div", { id: "banner", role: "status", hidden: true });
const main = h("main", { id: "main" });
const dock = h("footer", { id: "dock" });
app.append(header, banner, main, dock);
document.body.replaceChildren(app);

// The on-screen keyboard: the app is as tall as the visual viewport, so the
// docked fields sit right above the keyboard and the rest scrolls.
const root = document.documentElement;
function fitViewport() {
  const vv = window.visualViewport;
  const height = vv ? vv.height : window.innerHeight;
  root.style.setProperty("--vvh", `${Math.round(height)}px`);
  app.style.transform = vv && vv.offsetTop > 0 ? `translateY(${Math.round(vv.offsetTop)}px)` : "";
  root.classList.toggle("short", height < 560);
}
window.visualViewport?.addEventListener("resize", fitViewport);
window.visualViewport?.addEventListener("scroll", fitViewport);
window.addEventListener("resize", fitViewport);
fitViewport();
// A focused field scrolls its screen part into view once the keyboard is up.
document.addEventListener("focusin", (e) => {
  const t = e.target;
  if (t instanceof HTMLInputElement) setTimeout(() => t.scrollIntoView({ block: "nearest" }), 300);
});

const client = PartyClient.fromLocation();

// ---------------------------------------------------------------------------
// The view

/** @returns {RoomView} */
function roomView() {
  const r = client.view?.room;
  return r && typeof r === "object" && Array.isArray(r.people) ? r : { phase: "lobby", people: [], minPlayers: 3, maxPlayers: 8 };
}

/** @returns {PlayerState | null} */
function meView() {
  const m = client.view?.me;
  return m && typeof m === "object" && typeof m.task === "string" ? m : null;
}

/** @param {string | undefined} id */
function person(id) {
  return roomView().people.find((p) => p.id === id);
}

function myself() {
  return person(client.you?.id);
}

/** @param {string | undefined} id */
function nameOf(id) {
  return person(id)?.name ?? "Someone";
}

/** @param {string} name */
function possessive(name) {
  return /s$/i.test(name) ? `${name}’` : `${name}’s`;
}

/** @param {string[]} names */
function listNames(names) {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** @param {number} n */
function signed(n) {
  return n > 0 ? `+${n}` : n < 0 ? `−${-n}` : "0";
}

// ---------------------------------------------------------------------------
// Pieces: chips, cards, stamps

/**
 * A poker-chip avatar.
 * @param {Person | undefined} p
 * @param {number} size px
 * @param {{ crown?: boolean, blindfold?: boolean, face?: Face, color?: number }} [o]
 */
function chip(p, size, o = {}) {
  const color = COLORS[o.color ?? p?.color ?? 0] ?? COLORS[0];
  const el = h("span", { class: `chip${p?.bot ? " bot" : ""}${p && !p.connected ? " away" : ""}`, style: `--s:${size}px;--c:${color}` });
  el.append(faceSvg(o.face ?? p?.face));
  if (o.crown) el.append(h("i", { class: "crown" }));
  if (o.blindfold) el.append(h("i", { class: "blindfold" }));
  return el;
}

function flame() {
  return h("span", { class: "flame" }, icon(FLAME));
}

/**
 * A clue card: the back (tomato, or ink with a flame when bold) until it is
 * turned, then the cream face with the word.
 * @param {Card} c
 * @param {{ tag?: string, onclick?: () => void, label?: string, words?: boolean, extra?: string }} [o]
 */
function cardEl(c, o = {}) {
  const author = person(c.author);
  const up = typeof c.word === "string" && (o.words !== false || c.turned);
  const el = h(o.tag ?? "div", {
    class: `card${up ? " up" : " down"}${c.bold ? " bold" : ""}${c.turned ? " turned" : ""}${o.extra ? ` ${o.extra}` : ""}`,
    type: o.tag === "button" ? "button" : null,
    "aria-label": o.label ?? null,
    onclick: o.onclick ?? null,
  });
  const face = h("span", { class: "paper" });
  if (up) face.append(h("span", { class: "word" }, c.word ?? ""));
  else face.append(h("span", { class: "emblem" }, h("span", {}, "J")));
  el.append(face, chip(author, 34), h("span", { class: "who" }, author?.name ?? "?"));
  if (c.bold) el.append(flame());
  if (c.turned) el.append(h("span", { class: "order" }, c.turned));
  return el;
}

/**
 * Fits words into their card (the TV's rule): smaller down to 60 %, then
 * condensed down to 74 %, then an ellipsis. Runs after layout.
 * @param {ParentNode} [scope]
 */
function fitWords(scope = document) {
  requestAnimationFrame(() => {
    for (const el of scope.querySelectorAll(".card .word, .index .word")) {
      const w = /** @type {HTMLElement} */ (el);
      w.style.fontSize = "";
      w.style.transform = "";
      w.style.maxWidth = "";
      const parent = w.parentElement;
      if (!parent) continue;
      const ps = getComputedStyle(parent);
      const own = getComputedStyle(w);
      const pad = parseFloat(own.paddingLeft) + parseFloat(own.paddingRight);
      const room = parent.clientWidth - parseFloat(ps.paddingLeft) - parseFloat(ps.paddingRight) - 12 - pad;
      if (room <= 0) continue;
      const text = () => w.scrollWidth - pad;
      if (text() <= room) continue;
      const size = parseFloat(getComputedStyle(w).fontSize);
      w.style.fontSize = `${Math.max(size * 0.6, (size * room) / text())}px`;
      const k = room / text();
      if (k < 1) {
        const x = Math.max(0.74, k);
        w.style.maxWidth = `${(room + pad) / x}px`;
        w.style.transform = `scaleX(${x.toFixed(3)})`;
      }
    }
  });
}
document.fonts?.ready.then(() => fitWords());
document.fonts?.addEventListener("loadingdone", () => fitWords());
window.addEventListener("resize", () => fitWords());

/**
 * @param {string} text
 * @param {string} [tone] tomato, leaf, gold, ink
 */
function stamp(text, tone = "tomato") {
  return h("div", { class: `stamp ${tone}` }, text);
}

/** @param {string} text */
function tape(text) {
  return h("div", { class: "tape" }, text);
}

/**
 * The secret word on an index card.
 * @param {string} word
 * @param {string} [label]
 */
function indexCard(word, label = "The word") {
  const w = h("div", { class: "word" }, word);
  return h("div", { class: "index" }, h("div", { class: "label" }, label), w);
}

/** @param {number} pot */
function potEl(pot) {
  const stack = h("div", { class: "stack" });
  for (let i = 0; i < Math.min(pot, 12); i++) stack.append(h("i"));
  return h("div", { class: "pot" }, stack, h("div", { class: "label" }, h("small", {}, "The pot"), h("b", {}, pot)));
}

/** @param {...Kid} kids */
function note(...kids) {
  return h("p", { class: "note" }, ...kids);
}

/** @param {...Kid} kids */
function big(...kids) {
  return h("p", { class: "big" }, ...kids);
}

const ENVELOPE = `<svg viewBox="0 0 200 130" aria-hidden="true"><rect x="4" y="4" width="192" height="122" rx="10" fill="#d9b98a" stroke="#1d1a2b" stroke-width="6"/><path d="M6 10 L100 78 L194 10" fill="none" stroke="#1d1a2b" stroke-width="6" stroke-linejoin="round"/><circle cx="100" cy="76" r="24" fill="#df4a3c" stroke="#1d1a2b" stroke-width="5"/><text x="100" y="88" text-anchor="middle" font-size="32" font-weight="700" fill="#fbf1dc" font-family="Plex Sans, sans-serif">?</text></svg>`;

// ---------------------------------------------------------------------------
// Errors

/** @param {any} e */
function message(e) {
  switch (e?.code) {
    case "name-taken":
      return "Someone here already has that name. Pick another.";
    case "full":
      return "The table is full.";
    case "locked":
    case "in-progress":
      return "This game is closed to new players right now.";
    case "bad-key":
    case "no-room":
      return "That game is over. Scan the code on the TV again.";
    case "closed":
      return "Not connected. Trying again…";
    default:
      return typeof e?.message === "string" && e.message ? e.message : "That didn't work. Try again.";
  }
}

/**
 * Sends an action; a refusal lands in `err` (and is thrown on).
 * @param {string} type
 * @param {unknown} data
 * @param {HTMLElement | null} [err]
 */
async function act(type, data, err) {
  if (err) err.textContent = "";
  try {
    return await client.act(type, data);
  } catch (e) {
    if (err) err.textContent = message(e);
    throw e;
  }
}

// ---------------------------------------------------------------------------
// Rendering

/** The key of what is on screen: rebuilt only when it changes. */
let shown = "";
/** @type {Screen["refresh"] | null} */
let refresh = null;
/** Called by the clock shortly before the deadline (the write screen sends what is typed). */
/** @type {(() => void) | null} */
let lastCall = null;

/** @param {Screen} s */
function mount(s) {
  main.replaceChildren(...s.main.filter((n) => n !== null));
  dock.replaceChildren(...(s.dock ?? []).filter((n) => n !== null));
  dock.hidden = !s.dock?.length;
  app.dataset.tone = s.tone ?? "";
  main.scrollTop = 0;
  refresh = s.refresh ?? null;
  fitWords();
  lastCall = null;
}

/**
 * A plain screen outside the game (connecting, kicked, …).
 * @param {string} key
 * @param {...Kid} kids
 */
function plain(key, ...kids) {
  shown = key;
  mount({ main: [h("section", { class: "center" }, ...kids)] });
  outside = true;
  timer.hidden = true;
}

/** A screen outside the game is up (no clock). */
let outside = false;

function renderHeader() {
  const p = myself();
  meBox.replaceChildren();
  if (!p) return;
  meBox.append(h("span", { class: "name" }, p.name), chip(p, 36));
  const task = meView()?.task;
  if (roomView().phase !== "lobby" && task !== "wait") meBox.prepend(h("span", { class: "score", "aria-label": `${p.score} points` }, p.score));
}

function render() {
  const me = meView();
  const room = roomView();
  renderHeader();
  if (!me) return;
  const key = screenKey(me, room);
  if (key !== shown) {
    shown = key;
    outside = false;
    mount(build(me, room));
  }
  refresh?.(me, room);
  tick();
}

/**
 * @param {PlayerState} me
 * @param {RoomView} room
 */
function screenKey(me, room) {
  if (held(me, room)) return `hold:${me.task}:${room.phase}:${room.round ?? ""}`;
  return me.task === "lobby" || me.task === "wait" || me.task === "final" ? me.task : `${me.task}:${room.round ?? ""}`;
}

/**
 * A task the room has moved past: the host keeps the old task a while so a
 * phone never beats the TV (a giver's fate comes near the end of the jinx
 * check, the result a few seconds into the verdict). Until then: nothing
 * new, "watch the TV".
 * @param {PlayerState} me
 * @param {RoomView} room
 */
function held(me, room) {
  switch (me.task) {
    case "write":
    case "blind":
      return room.phase !== "write";
    case "guess":
    case "watch":
      return room.phase !== "reveal";
    default:
      return false;
  }
}

/**
 * @param {PlayerState} me
 * @param {RoomView} room
 * @returns {Screen}
 */
function hold(me, room) {
  if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
  const jinx = room.phase === "jinx";
  const guesser = me.task === "blind" || me.task === "guess" || room.guesser === client.you?.id;
  return {
    tone: "spot",
    main: [
      h(
        "section",
        { class: "center" },
        jinx ? h("div", { class: "backs" }, ...[0, 1, 2].map(() => h("i", { class: "back" }, h("span", {}, "J")))) : h("div", { class: "envelope" }, icon(ENVELOPE)),
        h("h1", { class: "title" }, jinx ? "Jinx check" : room.phase === "verdict" ? "The verdict" : "Look at the TV"),
        big("Watch the TV…"),
        note(jinx ? (guesser ? "Which clues made it?" : "Did anyone write the same clue?") : "Here comes the word."),
      ),
    ],
  };
}

/**
 * @param {PlayerState} me
 * @param {RoomView} room
 * @returns {Screen}
 */
function build(me, room) {
  if (held(me, room)) return hold(me, room);
  switch (me.task) {
    case "tv":
      return hold(me, room);
    case "lobby":
      return lobby(me);
    case "wait":
      return waitLate();
    case "hotseat":
      return hotseat(me, room);
    case "write":
      return write(me, room);
    case "blind":
      return blind(me);
    case "check":
      return check(me);
    case "fate":
      return fate(me);
    case "guess":
      return guess(me);
    case "watch":
      return watch(me, room);
    case "result":
      return result(me, room);
    case "mvp":
      return mvp(me);
    case "scores":
      return scores(me, room);
    case "final":
      return final(me);
    default:
      return { main: [h("section", { class: "center" }, big("Look at the TV!"))] };
  }
}

// ---------------------------------------------------------------------------
// Joining

/** @param {string} [error] */
function joinForm(error = "") {
  shown = "join";
  let saved = "";
  try {
    saved = client.saved?.name ?? localStorage.getItem(NAME_KEY) ?? "";
  } catch {}
  const input = /** @type {HTMLInputElement} */ (
    h("input", { id: "name", maxlength: 16, autocapitalize: "words", autocomplete: "nickname", enterkeyhint: "go", spellcheck: "false", placeholder: "Your name", "aria-label": "Your name" })
  );
  input.value = saved;
  const err = h("p", { class: "line error", role: "alert" }, error);
  const go = /** @type {HTMLButtonElement} */ (h("button", { type: "submit", class: "primary" }, "Take a seat"));
  const form = h(
    "form",
    {
      class: "join",
      autocomplete: "off",
      onsubmit: async (/** @type {Event} */ e) => {
        e.preventDefault();
        const name = input.value.trim();
        if (!name) return input.focus();
        try {
          localStorage.setItem(NAME_KEY, name);
        } catch {}
        err.textContent = "";
        go.disabled = true;
        try {
          await client.join(name);
          wake();
        } catch (e) {
          go.disabled = false;
          err.textContent = message(e);
          buzz([60, 40, 60]);
        }
      },
    },
    err,
    h("div", { class: "row" }, input, go),
  );
  mount({
    main: [h("section", { class: "center hello" }, logo(true), big("One word each."), big("Don't think alike."), note("3 to 8 players · you draw your face next"))],
    dock: [form],
  });
}

// ---------------------------------------------------------------------------
// Lobby: draw your face; the VIP starts and removes players.

/**
 * The face pad: your chip, drawn on with one ink; Undo, Clear, Shuffle.
 * Every change goes to the host (the TV shows it at once).
 * @param {Face} face
 * @param {number} color
 * @param {HTMLElement} err
 */
function facePad(face, color, err) {
  const canvas = /** @type {HTMLCanvasElement} */ (h("canvas", { class: "pad", "aria-label": "Draw your face" }));
  const preset = h("div", { class: "preset" });
  const ring = h("div", { class: "padchip", style: `--c:${COLORS[color] ?? COLORS[0]}` }, preset, canvas);
  const hint = h("p", { class: "line" });
  /** The preset the chip falls back to (clear, or before the first stroke). */
  let base = "preset" in face ? face.preset : 0;
  /** @type {ReturnType<typeof setTimeout> | null} */
  let sendTimer = null;
  const showPreset = (/** @type {boolean} */ on) => {
    preset.replaceChildren(on ? faceSvg({ preset: base }) : "");
    preset.hidden = !on;
  };
  const sendFace = () => {
    if (sendTimer) clearTimeout(sendTimer);
    sendTimer = setTimeout(() => {
      sendTimer = null;
      const next = pad.strokes.length ? { strokes: pad.strokes.map((s) => s.slice()) } : { preset: base };
      act("face", next, err).catch(() => {});
    }, 500);
  };
  const pad = new Pad(canvas, {
    onStroke: () => {
      showPreset(false);
      hint.textContent = pad.full ? "That's plenty of ink!" : "Look at the TV: that's you.";
      sendFace();
    },
    onFull: () => {
      hint.textContent = "That's plenty of ink!";
      buzz([30]);
    },
  });
  // The pad starts blank over the preset; a doodle from before comes back.
  canvas.addEventListener("pointerdown", () => showPreset(false));
  if ("strokes" in face && face.strokes.length) {
    pad.load(face.strokes);
    showPreset(false);
    hint.textContent = "Look at the TV: that's you.";
  } else {
    showPreset(true);
    hint.textContent = "Draw over it with your finger, or shuffle.";
  }
  const tools = h(
    "div",
    { class: "tools" },
    btn("ghost", () => {
      pad.undo();
      if (!pad.strokes.length) showPreset(true);
      sendFace();
    }, "Undo"),
    btn("ghost", () => {
      pad.clear();
      showPreset(true);
      sendFace();
    }, "Clear"),
    btn("ghost", () => {
      base = (base + 1 + Math.floor(Math.random() * (PRESETS.length - 1))) % PRESETS.length;
      pad.clear();
      showPreset(true);
      hint.textContent = "Shuffled! Draw over it, or shuffle again.";
      sendFace();
    }, "Shuffle"),
  );
  return { ring, tools, hint };
}

/**
 * @param {Task<"lobby">} me
 * @returns {Screen}
 */
function lobby(me) {
  const err = h("p", { class: "line error", role: "alert" });
  const { ring, tools, hint } = facePad(me.face, me.color, err);
  const count = h("span", { class: "count" });
  const list = h("ul", { class: "people" });
  const start = btn("primary", () => {
    start.disabled = true;
    act("start", null, err).catch(() => (start.disabled = false));
  }, "Start");
  const slip = h("p", { class: "slip" });
  /** @type {string | null} */
  let confirmKick = null;
  let lastList = "";
  return {
    main: [
      h("h1", { class: "title" }, "Draw your face"),
      ring,
      tools,
      hint,
      h("h2", { class: "label" }, "At the table ", count),
      list,
    ],
    dock: [err, start, slip],
    refresh: (m, room) => {
      if (m.task !== "lobby") return;
      const vip = !!myself()?.vip;
      const here = room.people.filter((p) => p.connected && !p.left);
      count.textContent = `${room.people.length} / ${room.maxPlayers}`;
      const key = JSON.stringify([room.people.map((p) => [p.id, p.name, p.color, p.face, p.vip, p.connected, p.bot]), vip, confirmKick]);
      if (key !== lastList) {
        lastList = key;
        list.replaceChildren(
          ...room.people.map((p) => {
            const mine = p.id === client.you?.id;
            const kick =
              vip && !mine
                ? btn(confirmKick === p.id ? "kick sure" : "kick", () => {
                    if (confirmKick !== p.id) {
                      confirmKick = p.id;
                      render();
                      return;
                    }
                    confirmKick = null;
                    act("kick", { id: p.id }, err).catch(() => {});
                  }, confirmKick === p.id ? "Remove?" : "✕")
                : null;
            if (kick) kick.setAttribute("aria-label", `Remove ${p.name}`);
            return h(
              "li",
              { class: p.connected ? "" : "away" },
              chip(p, 40, { crown: p.vip }),
              h("span", { class: "name" }, p.name, mine ? h("small", {}, " (you)") : null),
              !p.connected ? h("small", { class: "gone" }, "away") : null,
              kick,
            );
          }),
        );
      }
      start.hidden = !vip;
      start.disabled = !m.canStart;
      start.textContent = m.canStart ? `Start with ${here.length} players` : `Need ${room.minPlayers} players (${Math.max(0, room.minPlayers - here.length)} more)`;
      const host = room.people.find((p) => p.vip);
      slip.hidden = vip;
      slip.textContent = host ? `${host.name} starts the game from their phone` : "The game starts soon";
    },
  };
}

/** @returns {Screen} */
function waitLate() {
  const p = myself();
  const err = h("p", { class: "line error", role: "alert" });
  const { ring, tools, hint } = facePad(p?.face ?? { preset: 0 }, p?.color ?? 0, err);
  return {
    main: [
      h("h1", { class: "title" }, "You're in!"),
      big("A round is on: you play from the next one. Draw your face in the meantime."),
      ring,
      tools,
      hint,
      err,
    ],
  };
}

// ---------------------------------------------------------------------------
// The round

/**
 * @param {Task<"hotseat">} me
 * @param {RoomView} room
 * @returns {Screen}
 */
function hotseat(me, room) {
  const g = person(room.guesser);
  const round = room.round && room.rounds ? tape(`Round ${room.round} of ${room.rounds}`) : null;
  if (me.guesser) {
    buzz([80, 60, 80]);
    return {
      tone: "spot",
      main: [
        h(
          "section",
          { class: "center" },
          round,
          chip(myself(), 132, { blindfold: true }),
          h("h1", { class: "title" }, "You're in the hot seat"),
          big("Don't look at anyone's phone."),
          note("Your friends write clues. You guess the word."),
        ),
      ],
    };
  }
  return {
    main: [
      h(
        "section",
        { class: "center" },
        round,
        chip(g, 120, { blindfold: true }),
        h("h1", { class: "title" }, `${g?.name ?? "Someone"} guesses`),
        big("Get ready: the word is coming."),
        note("One word each. Same clue? Both cards go up in smoke."),
      ),
    ],
  };
}

/**
 * The clue-giver writes (one slot, or two with 3 players).
 * @param {Task<"write">} me
 * @param {RoomView} room
 * @returns {Screen}
 */
function write(me, room) {
  const g = person(room.guesser);
  const allIn = h("p", { class: "line ok" });
  /** @type {{ refresh: (sent: { text: string, bold: boolean } | null) => void, lastCall: () => void }[]} */
  const slots = [];
  const box = h("div", { class: `slots n${me.each}` });
  for (let i = 0; i < me.each; i++) {
    const s = slot(i, me.each);
    slots.push(s);
    box.append(s.el);
  }
  const screen = {
    main: [
      indexCard(me.word),
      h("p", { class: "line who-guesses" }, chip(g, 26), ` ${g?.name ?? "Someone"} guesses · ${me.each === 2 ? "two clues each" : "one word each"}`),
      allIn,
      note(h("b", {}, "Bold"), " bets on your clue: +4 if it's turned, −2 if it's jinxed."),
    ],
    dock: [box],
    refresh: (/** @type {PlayerState} */ m) => {
      if (m.task !== "write") return;
      slots.forEach((s, i) => s.refresh(m.sent[i] ?? null));
      const done = m.sent.filter(Boolean).length;
      allIn.textContent = done >= m.each ? "All in! Watch the pile on the TV. You can still change it." : "";
    },
  };
  queueMicrotask(() => {
    lastCall = () => slots.forEach((s) => s.lastCall());
  });
  return screen;
}

/**
 * One clue slot: the field, the flame toggle, Send; once accepted, the sent
 * card with Change. A refusal shows the host's message and keeps the text.
 * @param {number} i
 * @param {number} each
 */
function slot(i, each) {
  const input = /** @type {HTMLInputElement} */ (
    h("input", {
      maxlength: MAX_WORD,
      placeholder: each === 2 ? `Clue ${i + 1}: one word` : "One word",
      "aria-label": each === 2 ? `Clue ${i + 1}` : "Your clue",
      autocapitalize: "none",
      autocomplete: "off",
      autocorrect: "off",
      spellcheck: "false",
      enterkeyhint: "send",
    })
  );
  const label = h("p", { class: "line", "aria-live": "polite" });
  const bold = btn("boldtoggle", () => {
    setBold(bold.getAttribute("aria-pressed") !== "true");
    input.focus({ preventScroll: true });
  }, icon(FLAME), h("span", {}, "Bold"));
  bold.setAttribute("aria-pressed", "false");
  bold.setAttribute("aria-label", "Bold");
  const send = btn("primary send", () => submit(), "Send");
  const editor = h(
    "form",
    {
      class: "editor",
      autocomplete: "off",
      onsubmit: (/** @type {Event} */ e) => {
        e.preventDefault();
        submit();
      },
    },
    input,
    bold,
    send,
  );
  const sentWord = h("span", { class: "word" });
  const sentCard = h("div", { class: "sent" }, h("span", { class: "mini-back" }, h("span", {}, "J")), h("span", { class: "text" }, "Sent: ", sentWord), btn("ghost small", () => edit(), "Change"));
  const el = h("div", { class: "slot" }, label, editor, sentCard);
  let pending = false;
  let editing = false;
  /** @type {{ text: string, bold: boolean } | null} */
  let sent = null;
  /** @type {string} */
  let error = "";

  /** @param {boolean} on */
  function setBold(on) {
    bold.setAttribute("aria-pressed", String(on));
    el.classList.toggle("is-bold", on);
  }
  function edit() {
    editing = true;
    if (sent) {
      input.value = sent.text;
      setBold(sent.bold);
    }
    update();
    input.focus();
  }
  async function submit() {
    const text = input.value.trim();
    if (pending) return;
    if (!text) {
      error = "Type a clue first.";
      update();
      input.focus();
      return;
    }
    pending = true;
    error = "";
    update();
    try {
      await client.act("clue", { text, bold: bold.getAttribute("aria-pressed") === "true", slot: i });
      editing = false;
      buzz([25]);
      if (document.activeElement === input) input.blur();
    } catch (e) {
      error = message(e);
      buzz([60, 40, 60]);
      input.focus();
    }
    pending = false;
    update();
  }
  function update() {
    const open = !sent || editing;
    editor.hidden = !open;
    sentCard.hidden = open;
    send.disabled = pending;
    send.textContent = pending ? "…" : sent && editing ? "Swap" : "Send";
    label.classList.toggle("error", !!error);
    label.textContent = error || (open ? (each === 2 ? `Clue ${i + 1}` : "Your clue") + (sent ? " · change it until the time's up" : "") : "");
    if (sent) {
      sentWord.textContent = sent.text;
      sentCard.classList.toggle("bold", sent.bold);
      sentWord.append(sent.bold ? " 🔥" : "");
    }
  }
  update();
  return {
    el,
    /** @param {{ text: string, bold: boolean } | null} s */
    refresh(s) {
      const changed = JSON.stringify(s) !== JSON.stringify(sent);
      sent = s;
      if (changed) update();
    },
    lastCall() {
      if (!pending && (!sent || editing) && input.value.trim() && input.value.trim() !== sent?.text) submit();
    },
  };
}

/**
 * @param {Task<"blind">} me
 * @returns {Screen}
 */
function blind(me) {
  const count = h("p", { class: "big" });
  const dots = h("div", { class: "dots" });
  return {
    tone: "spot",
    main: [
      h(
        "section",
        { class: "center" },
        h("div", { class: "envelope" }, icon(ENVELOPE)),
        h("h1", { class: "title" }, "Eyes on the TV"),
        big("Your friends are writing clues."),
        dots,
        count,
      ),
    ],
    refresh: (m) => {
      if (m.task !== "blind") return;
      count.textContent = `${m.done} of ${m.givers} done`;
      dots.replaceChildren(...Array.from({ length: m.givers }, (_, i) => h("i", { class: i < m.done ? "on" : "" })));
    },
  };
}

/**
 * @param {Task<"check">} me
 * @returns {Screen}
 */
function check(me) {
  const backs = h("div", { class: "backs" });
  for (let i = 0; i < me.survived; i++) backs.append(h("i", { class: "back" }, h("span", {}, "J")));
  for (let i = 0; i < me.jinxed; i++) backs.append(h("i", { class: "back broken" }, h("span", {}, "J")));
  return {
    tone: "spot",
    main: [
      h(
        "section",
        { class: "center" },
        h("h1", { class: "title" }, "Jinx check"),
        backs,
        big(
          me.survived === 1 ? "1 clue survived" : `${me.survived} clues survived`,
          h("br"),
          h("span", { class: "dim" }, me.jinxed === 1 ? "1 was jinxed" : `${me.jinxed} were jinxed`),
        ),
        note(me.survived ? "You pick which card to turn. Get ready." : "Every clue went up in smoke."),
      ),
    ],
  };
}

/**
 * @param {Task<"fate">} me
 * @returns {Screen}
 */
function fate(me) {
  const jinxed = me.cards.some((c) => c.fate === "jinxed");
  buzz(jinxed ? [220] : [30]);
  const list = h("div", { class: "fates" });
  for (const c of me.cards) {
    const names = c.with.map((w) => person(w)?.name ?? w);
    const why = c.reason ? reasonText(c.reason) : "";
    list.append(
      h(
        "div",
        { class: `fate ${c.fate}` },
        h("div", { class: `card up${c.bold ? " bold" : ""}${c.fate === "jinxed" ? " cracked" : ""}` }, h("span", { class: "paper" }, h("span", { class: "word" }, c.text)), c.bold ? flame() : null),
        c.fate === "alive"
          ? h("div", { class: "says" }, stamp("Survived", "leaf"), h("p", {}, c.bold ? "Your bold card is still in. Fingers crossed." : "Still in the game."))
          : h(
              "div",
              { class: "says" },
              stamp("Jinxed!", "tomato"),
              h("p", {}, names.length ? `with ${listNames(names)}` : "with your other clue"),
              why ? tape(why) : null,
              c.bold ? h("p", { class: "minus" }, "Bold: −2") : null,
            ),
      ),
    );
  }
  return {
    main: [indexCard(me.word), h("h1", { class: "title" }, me.cards.length > 1 ? "Your cards" : "Your card"), list, note("Don't say a word: the guesser hasn't guessed yet.")],
  };
}

/** @param {Reason} r */
function reasonText(r) {
  return r === "spelling" ? "other spelling" : r;
}

/**
 * The guesser: turn cards, type the one guess (live on the TV), or pass.
 * @param {Task<"guess">} me
 * @returns {Screen}
 */
function guess(me) {
  const grid = h("div", { class: "cards" });
  const potBox = h("div", { class: "potbox" });
  const info = h("p", { class: "line" });
  const err = h("p", { class: "line error", role: "alert" });
  const input = /** @type {HTMLInputElement} */ (
    h("input", {
      class: "tape-field",
      maxlength: MAX_WORD,
      placeholder: "Your guess (one shot)",
      "aria-label": "Your guess",
      autocapitalize: "none",
      autocomplete: "off",
      autocorrect: "off",
      spellcheck: "false",
      enterkeyhint: "done",
    })
  );
  const go = btn("primary", () => lockIn(), "Guess");
  let passArmed = false;
  /** @type {ReturnType<typeof setTimeout> | null} */
  let passTimer = null;
  const pass = btn("ghost pass", () => {
    if (!passArmed) {
      passArmed = true;
      pass.textContent = "Sure? Pass";
      pass.classList.add("armed");
      passTimer = setTimeout(() => {
        passArmed = false;
        pass.textContent = "Pass";
        pass.classList.remove("armed");
      }, 3000);
      return;
    }
    if (passTimer) clearTimeout(passTimer);
    busy = true;
    sync();
    act("pass", null, err).catch(() => {
      busy = false;
      sync();
    });
  }, "Pass");
  const form = h(
    "form",
    {
      class: "editor",
      autocomplete: "off",
      // Enter (the keyboard's "done") only closes the keyboard: the one
      // guess goes with the Guess button, never by accident.
      onsubmit: (/** @type {Event} */ e) => {
        e.preventDefault();
        input.blur();
      },
    },
    input,
    go,
  );
  let busy = false;
  let cards = me.cards;
  let lastCards = "";
  input.addEventListener("input", () => {
    client.input({ draft: input.value.slice(0, MAX_WORD) });
    sync();
  });
  function lockIn() {
    const text = input.value.trim();
    if (busy || !text || !cards.some((c) => c.turned)) return;
    busy = true;
    sync();
    input.blur();
    act("guess", { text }, err).catch(() => {
      busy = false;
      sync();
      buzz([60, 40, 60]);
    });
  }
  /** @param {string} id */
  function pick(id) {
    if (busy) return;
    busy = true;
    sync();
    buzz([20]);
    act("pick", { card: id }, err)
      .catch(() => {})
      .finally(() => {
        busy = false;
        sync();
      });
  }
  function sync() {
    const turned = cards.filter((c) => c.turned).length;
    const left = cards.length - turned;
    input.disabled = busy || turned === 0;
    input.placeholder = turned === 0 ? "Turn a card first" : "Your guess (one shot)";
    go.disabled = busy || turned === 0 || !input.value.trim();
    pass.disabled = busy || turned === 0;
    grid.querySelectorAll("button").forEach((b) => ((/** @type {HTMLButtonElement} */ (b)).disabled = busy));
    info.textContent =
      turned === 0 ? "Whose card do you trust? Tap it to turn it." : left === 0 ? "No cards left: guess or pass." : "Turn another (−2 chips) or guess now.";
  }
  return {
    tone: "hot",
    main: [h("div", { class: "head" }, potBox, h("div", { class: "info" }, h("h1", { class: "title" }, "Your turn"), h("p", { class: "dim jinxed" }))), grid],
    dock: [info, err, form, pass],
    refresh: (m) => {
      if (m.task !== "guess") return;
      cards = m.cards;
      const key = JSON.stringify([m.cards, m.pot]);
      if (key !== lastCards) {
        const before = lastCards;
        lastCards = key;
        potBox.replaceChildren(potEl(m.pot));
        const jinx = main.querySelector(".jinxed");
        if (jinx) jinx.textContent = `${m.cards.length} ${m.cards.length === 1 ? "card" : "cards"} survived · ${m.jinxed} jinxed`;
        fitWords(grid);
        grid.replaceChildren(
          ...m.cards.map((c) => {
            const name = nameOf(c.author);
            return c.turned
              ? cardEl(c)
              : cardEl(c, { tag: "button", onclick: () => pick(c.id), label: `Turn ${possessive(name)} card`, words: false });
          }),
        );
        if (before) buzz([15]);
      }
      sync();
    },
  };
}

/**
 * A clue-giver in the reveal: the word, the cards, quiet.
 * @param {Task<"watch">} me
 * @param {RoomView} room
 * @returns {Screen}
 */
function watch(me, room) {
  const g = person(room.guesser);
  const grid = h("div", { class: "cards small" });
  const potBox = h("div", { class: "potbox" });
  let last = "";
  return {
    main: [
      h("div", { class: "head" }, indexCard(me.word), potBox),
      h("div", { class: "shh" }, h("b", {}, "SHHH!"), h("span", {}, `No hints! ${g?.name ?? "The guesser"} is guessing.`)),
      grid,
    ],
    refresh: (m) => {
      if (m.task !== "watch") return;
      const key = JSON.stringify([m.cards, m.pot]);
      if (key === last) return;
      last = key;
      potBox.replaceChildren(potEl(m.pot));
      grid.replaceChildren(...m.cards.map((c) => cardEl(c)));
      fitWords(grid);
    },
  };
}

/** @type {Record<Payout["why"], string>} */
const WHY = {
  pot: "The pot is yours",
  wrong: "Wrong guess: −2",
  turned: "Your card was turned: +2",
  bold: "Your bold card was turned: +4",
  "bold-jinxed": "Your bold card was jinxed: −2",
  mvp: "MVP clue: +3",
};

/**
 * The verdict (and the replay for clue-givers).
 * @param {Task<"result">} me
 * @param {RoomView} room
 * @returns {Screen}
 */
function result(me, room) {
  const g = person(room.guesser);
  const replay = h("p", { class: "line replay" });
  /** @type {Kid} */
  let top;
  if (me.guesser) {
    top =
      me.result === "right"
        ? stamp("Right!", "leaf")
        : me.result === "wrong"
          ? stamp("Not it", "ink")
          : me.result === "pass"
            ? stamp("Pass", "ink")
            : stamp("All jinxed", "tomato");
    buzz(me.result === "right" ? [40, 40, 40, 40, 140] : me.result === "wrong" ? [300] : [60]);
  } else {
    const verdict = me.result === "right" ? `${g?.name ?? "The guesser"} got it!` : me.result === "wrong" ? `${g?.name ?? "The guesser"} missed it` : me.result === "pass" ? `${g?.name ?? "The guesser"} passed` : "Every clue was jinxed";
    top = h("h1", { class: "title" }, verdict);
    buzz(me.delta > 0 ? [40, 40, 80] : me.delta < 0 ? [200] : [30]);
  }
  const why = me.why.map((w) => h("li", {}, WHY[w] ?? w));
  return {
    tone: me.result === "right" ? "good" : "",
    main: [
      h(
        "section",
        { class: "center" },
        top,
        indexCard(me.word, "The word was"),
        h("p", { class: `points ${me.delta > 0 ? "plus" : me.delta < 0 ? "minus" : "zero"}` }, signed(me.delta)),
        why.length ? h("ul", { class: "why" }, ...why) : note(me.guesser ? (me.result === "pass" ? "No points, no loss." : "No points this round.") : "No points for you this round."),
        replay,
      ),
    ],
    refresh: (m, r) => {
      if (m.task !== "result") return;
      replay.textContent = r.phase === "replay" && !m.guesser && m.result === "right" ? `${g?.name ?? "The guesser"} is picking the MVP clue…` : r.phase === "replay" ? "Look at the TV: the replay." : "";
    },
  };
}

/**
 * The guesser picks the clue that helped most.
 * @param {Task<"mvp">} me
 * @returns {Screen}
 */
function mvp(me) {
  const err = h("p", { class: "line error", role: "alert" });
  const grid = h("div", { class: "cards" });
  /** @type {string | null} */
  let chosen = null;
  const done = h("p", { class: "line ok" });
  const draw = () => {
    grid.replaceChildren(
      ...me.cards.map((c) => {
        const b = cardEl(c, {
          tag: "button",
          extra: chosen === c.id ? "chosen" : chosen ? "dim" : "",
          label: `MVP: ${c.word ?? ""} by ${nameOf(c.author)}`,
          onclick: () => {
            if (chosen) return;
            chosen = c.id;
            buzz([30, 30, 60]);
            draw();
            act("mvp", { card: c.id }, err).then(
              () => (done.textContent = `${nameOf(c.author)} gets +3. Look at the TV!`),
              () => {
                chosen = null;
                draw();
              },
            );
          },
        });
        if (chosen === c.id) b.append(h("span", { class: "rosette" }, "MVP"));
        return b;
      }),
    );
  };
  draw();
  fitWords(grid);
  return {
    tone: "good",
    main: [h("h1", { class: "title" }, "Which clue helped most?"), note("The MVP clue's author gets +3."), grid, done, err],
  };
}

/**
 * @param {Task<"scores">} me
 * @param {RoomView} room
 * @returns {Screen}
 */
function scores(me, room) {
  if (me.next) buzz([80, 60, 80]);
  return {
    main: [
      h(
        "section",
        { class: "center" },
        room.round && room.rounds ? tape(`After round ${room.round} of ${room.rounds}`) : null,
        h("div", { class: "pad-note" }, h("p", { class: "rank" }, `#${me.rank}`, h("small", {}, ` of ${me.of}`)), h("p", { class: "total" }, `${me.score} ${me.score === 1 ? "point" : "points"}`)),
        me.next ? h("div", { class: "next" }, stamp("You're up next!", "gold"), note("You guess the next word. Don't peek at any phone.")) : note("Look at the TV for the scores."),
      ),
    ],
  };
}

/** @type {Record<RoomTitle, string>} */
const TITLES = {
  telepaths: "Telepaths!",
  "mind readers": "Mind readers!",
  "in sync": "In sync!",
  "mixed signals": "Mixed signals",
  "total jinx": "Total jinx",
};

/** @type {Record<Award["kind"], [string, string]>} */
const AWARDS = {
  "mind meld": ["Mind meld", "You and your twin brain jinxed each other the most."],
  "lone wolf": ["Lone wolf", "The most one-of-a-kind clues that got turned."],
  kamikaze: ["Kamikaze", "The most clues lost to jinxes. And you kept going."],
};

/**
 * @param {Task<"final">} me
 * @returns {Screen}
 */
function final(me) {
  const err = h("p", { class: "line error", role: "alert" });
  const again = btn("primary", () => {
    again.disabled = true;
    act("again", null, err).catch(() => (again.disabled = false));
  }, "Play again");
  const slip = h("p", { class: "slip" });
  const award = me.award ? AWARDS[me.award] : null;
  const words = h("div", { class: "tiles big count" }, ...[String(me.found), "of", String(me.total)].map((t, i) => h("b", { class: i === 1 ? "of" : "", style: `--r:${[-3, 2, 4][i]}deg` }, t)));
  return {
    main: [
      h(
        "section",
        { class: "center" },
        tape("Game over"),
        words,
        note("words found together"),
        stamp(TITLES[me.title] ?? me.title, me.found * 2 >= me.total ? "leaf" : "tomato"),
        h("div", { class: "pad-note" }, h("p", { class: "rank" }, `#${me.rank}`, h("small", {}, ` of ${me.of}`)), h("p", { class: "total" }, `${me.score} ${me.score === 1 ? "point" : "points"}`)),
        award ? h("div", { class: "award" }, h("small", {}, "Your award"), h("b", {}, award[0]), h("p", {}, award[1])) : null,
      ),
    ],
    dock: [err, again, slip],
    refresh: (m, room) => {
      if (m.task !== "final") return;
      again.hidden = !m.canRestart;
      const host = room.people.find((p) => p.vip);
      slip.hidden = m.canRestart;
      slip.textContent = host ? `${host.name} can start a new game from their phone` : "Thanks for playing!";
    },
  };
}

// ---------------------------------------------------------------------------
// The clock: the header's dial for the task's deadline.

let buzzedLow = "";
function tick() {
  const me = meView();
  const deadline = !outside && me && !held(me, roomView()) && "deadline" in me && typeof me.deadline === "number" ? me.deadline : null;
  timer.hidden = deadline === null;
  if (deadline === null || !me) return;
  const left = deadline - client.now();
  const s = Math.max(0, Math.ceil(left / 1000));
  timer.textContent = String(s);
  timer.classList.toggle("low", left < 10_000);
  if (left < 10_000 && left > 0 && buzzedLow !== shown && me.task !== "blind") {
    buzzedLow = shown;
    buzz([40]);
  }
  if (left < 900) lastCall?.();
}
setInterval(tick, 250);

// ---------------------------------------------------------------------------
// Connection

client.on("status", (/** @type {string} */ s) => {
  if (s === "reconnecting") {
    banner.hidden = false;
    banner.textContent = "Connection lost. Reconnecting…";
    app.classList.add("offline");
  } else if (s === "open") {
    banner.hidden = true;
    app.classList.remove("offline");
  } else if (s === "closed" && !client.byeReason && shown !== "join") {
    banner.hidden = true;
    app.classList.remove("offline");
    plain("closed", h("h1", { class: "title" }, "Disconnected"), big("The connection to the game is closed."), btn("primary", () => location.reload(), "Reconnect"));
  }
});

client.on("bye", (/** @type {string} */ reason) => {
  const text =
    reason === "kicked"
      ? "You were removed from the table."
      : reason === "replaced"
        ? "You opened the game on another device."
        : reason === "room-closed"
          ? "The game is over. Thanks for playing!"
          : null;
  if (!text) return;
  banner.hidden = true;
  app.classList.remove("offline");
  timer.hidden = true;
  meBox.replaceChildren();
  if (reason === "kicked") buzz([200]);
  plain(`bye:${reason}`, logo(true), big(text), reason !== "room-closed" ? note("Scan the code on the TV to join again.") : null);
});

client.on("view", () => {
  if (shown.startsWith("bye:")) return;
  render();
});

// Keep the screen on while playing.
async function wake() {
  try {
    await /** @type {any} */ (navigator).wakeLock?.request("screen");
  } catch {}
}
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") wake();
});

plain("connecting", logo(true), big("Connecting…"));
if (!client.room || !client.key) {
  plain("no-room", logo(true), big("Scan the code on the TV to join."));
} else if (client.canResume) {
  client.connect().then(wake, (/** @type {any} */ e) => joinForm(e?.code === "no-player" ? "" : message(e)));
} else {
  joinForm();
}

