// @ts-check
/**
 * drawbluff's phone page: joining, the lobby, the drawing pad, lies,
 * votes, scores. Everything it shows comes from the party's view
 * (`{ roster, room, me }`); everything it does is an action. It speaks
 * the room's language (`room.lang`), or the one the player picked with
 * the switch in the header (kept on this phone).
 */

import { PartyClient } from "/_tvpipe/lib/client.js";
import { browserLang, pickLang } from "./i18n.js";
import { strings } from "./strings.js";
import { drawDrawing, drawStroke, GRID, PALETTE } from "./strokes.js";

const MAX_STROKES = 300;
const MAX_POINTS = 12000;
const WIDTHS = [8, 20, 48];
/** The longest lie the input takes (`LIE_SOFT` in game/rules.ts). */
const LIE_SOFT = 40;

// Phones are game controllers here, not documents: iOS Safari ignores
// `user-scalable=no`, so block its pinch and double-tap zoom gestures and
// text selection outside text fields by hand.
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

const client = PartyClient.fromLocation();
const app = /** @type {HTMLElement} */ (document.getElementById("app"));
const banner = /** @type {HTMLElement} */ (document.getElementById("banner"));
const timer = /** @type {HTMLElement} */ (document.getElementById("timer"));
const who = /** @type {HTMLElement} */ (document.getElementById("who"));
const langButton = /** @type {HTMLButtonElement} */ (document.getElementById("lang"));

// ---------------------------------------------------------------------------
// The language: the phone's own choice, else the room's, else the browser's.

const LANG_KEY = "drawbluff:lang";
/** @returns {import("./i18n.js").Lang | null} */
function ownLang() {
  try {
    const v = localStorage.getItem(LANG_KEY);
    return v ? pickLang(v) : null;
  } catch {
    return null;
  }
}
/** @returns {import("./i18n.js").Lang} */
function roomLang() {
  const l = client.view?.room?.lang;
  return typeof l === "string" ? pickLang(l) : browserLang();
}
let lang = ownLang() ?? roomLang();
let t = strings(lang);
document.documentElement.lang = lang;

/** Follows the language; true when it changed (the screen has to be built again). */
function useLang() {
  const l = ownLang() ?? roomLang();
  if (l === lang) return false;
  lang = l;
  t = strings(l);
  document.documentElement.lang = l;
  return true;
}

/** The switch: only where rebuilding loses nothing (joining, the lobby, the end). */
function showSwitch(on = false) {
  langButton.hidden = !on;
  langButton.textContent = t.otherLang;
}
langButton.addEventListener("click", () => {
  const next = lang === "de" ? "en" : "de";
  try {
    // Back to the room's language: follow the room again.
    if (next === roomLang()) localStorage.removeItem(LANG_KEY);
    else localStorage.setItem(LANG_KEY, next);
  } catch {}
  if (!useLang()) return;
  if (shown === "join") {
    const typed = /** @type {HTMLInputElement | null} */ (document.getElementById("name"))?.value;
    joinForm("", typed);
  } else if (client.view?.me) {
    shown = "";
    render(client.view);
  } else showSwitch(true);
});

/** The key of what is on screen: rebuilt only when it changes. */
let shown = "";
/** @type {(() => void) | null} called on every view while the screen stays */
let refresh = null;
/** @type {Pad | null} */
let pad = null;

// ---------------------------------------------------------------------------
// DOM helpers: text always as text.

/**
 * @param {string} tag
 * @param {Record<string, any>} [attrs]
 * @param {...(Node | string | null | undefined | false)} kids
 * @returns {HTMLElement}
 */
function h(tag, attrs = {}, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (k === "class") el.className = v;
    else if (v === true) el.setAttribute(k, "");
    else if (v !== false && v != null) el.setAttribute(k, String(v));
  }
  for (const kid of kids) if (kid) el.append(kid);
  return el;
}

/** @param {...Node} nodes */
function show(...nodes) {
  app.replaceChildren(...nodes);
}

/** @param {string | null} text */
function setBanner(text) {
  banner.hidden = !text;
  banner.textContent = text ?? "";
}

// ---------------------------------------------------------------------------
// Joining

/**
 * @param {string} [error]
 * @param {string} [typed] the name typed so far (the language switched)
 */
function joinForm(error = "", typed) {
  shown = "join";
  refresh = null;
  showSwitch(true);
  const tpl = /** @type {HTMLTemplateElement} */ (document.getElementById("t-join"));
  show(tpl.content.cloneNode(true));
  const input = /** @type {HTMLInputElement} */ (document.getElementById("name"));
  const err = /** @type {HTMLElement} */ (document.getElementById("join-error"));
  /** @type {HTMLElement} */ (app.querySelector("h1")).textContent = t.joinTitle;
  /** @type {HTMLElement} */ (app.querySelector("label")).textContent = t.yourName;
  /** @type {HTMLElement} */ (app.querySelector("#join-form button")).textContent = t.join;
  input.value = typed ?? client.saved?.name ?? localStorage.getItem("drawbluff:name") ?? "";
  err.textContent = error;
  /** @type {HTMLFormElement} */ (document.getElementById("join-form")).addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const name = input.value.trim();
    if (!name) return;
    try {
      localStorage.setItem("drawbluff:name", name);
    } catch {}
    err.textContent = "";
    try {
      await client.join(name);
    } catch (e) {
      joinForm(message(e));
    }
  });
  if (!input.value) input.focus();
}

/** @param {any} e */
function message(e) {
  const code = e?.code;
  switch (code) {
    case "name-taken":
      return t.nameTaken;
    case "name-invalid":
      return lang === "en" && e.message ? e.message : t.pickAnother;
    case "full":
      return t.full;
    case "locked":
    case "in-progress":
      return t.closed;
    case "bad-key":
      return t.badKey;
    case "no-room":
      return t.noRoom;
    default:
      return (typeof code === "string" ? t.errors[code] : undefined) ?? e?.message ?? String(e);
  }
}

client.on("status", (s) => {
  if (s === "reconnecting") setBanner(t.reconnecting);
  else if (s === "open") setBanner(null);
});

client.on("bye", (reason) => {
  const text = reason === "kicked" ? t.kicked : reason === "replaced" ? t.replaced : reason === "room-closed" ? t.roomClosed : null;
  if (text) {
    shown = "bye";
    refresh = null;
    showSwitch(false);
    setBanner(null);
    timer.hidden = true;
    show(h("section", { class: "card" }, h("p", { class: "big" }, text)));
  }
});

client.on("view", (view) => render(view));

// ---------------------------------------------------------------------------
// Views

/** @param {Record<string, any>} view */
function render(view) {
  const me = view.me ?? {};
  const room = view.room ?? {};
  const roster = view.roster ?? [];
  const mine = roster.find((/** @type {any} */ r) => r.id === client.you?.id);
  if (useLang()) shown = "";
  const rank = typeof me.rank === "number" && typeof me.of === "number" && me.of > 0 && me.task !== "lobby" ? ` · ${t.rankShort(me.rank, me.of)}` : "";
  who.textContent = mine ? `${mine.vip ? "★ " : ""}${mine.name}${rank}` : "";
  const key = screenKey(me, room);
  if (key !== shown) {
    showSwitch(me.task === "lobby" || me.task === "final");
    shown = key;
    refresh = null;
    pad = null;
    build(view);
  }
  refresh?.();
}

/**
 * What is on screen: a lie or a vote stays while the room moves on (the
 * next drawing's liars write during the current vote and reveal).
 * @param {any} me
 * @param {any} room
 */
function screenKey(me, room) {
  const done = me.done ? 1 : 0;
  switch (me.task) {
    case "lie":
    case "vote":
    case "artist":
      return `${me.task}:${me.key ?? ""}:${done}`;
    case "draw":
      return `draw:${room.round ?? ""}:${me.deadline ?? ""}:${done}`;
    default:
      return `${me.task}:${room.round ?? ""}:${room.spot ?? ""}:${room.phase ?? ""}:${me.note ?? ""}`;
  }
}

/** @param {Record<string, any>} view */
function build(view) {
  const me = view.me ?? {};
  const room = view.room ?? {};
  switch (me.task) {
    case "lobby":
      return lobby(view);
    case "draw":
      return me.done ? waiting(t.drawDone) : draw(me.prompt);
    case "wait":
      return waiting(me.note === "drawing" ? t.waitDrawing : me.note === "game" ? t.waitGame : t.waitRound);
    case "artist":
      return waiting(t.artist);
    case "lie":
      return me.done ? waiting(t.lieDone(me.lie ?? "", !!me.early)) : lie(me);
    case "vote":
      return me.done ? waiting(t.voteDone) : vote(me.options ?? [], room.drawing);
    case "watch":
      return watch(me, room);
    case "scores":
      return scoreCard(t.roundDone(room.round ?? 0), me);
    case "final":
      return final(view);
    default:
      return waiting(t.lookAtTv);
  }
}

/** @param {string} text */
function waiting(text) {
  show(h("section", { class: "card" }, h("p", { class: "big" }, text)));
}

/**
 * @param {string} title
 * @param {any} me
 * @param {...(Node | string | null | undefined | false)} extra
 */
function scoreCard(title, me, ...extra) {
  show(
    h(
      "section",
      { class: "card" },
      h("h1", {}, title),
      ...extra,
      h("p", { class: "score" }, formatNumber(me.score ?? 0)),
      h("p", { class: "muted" }, `${t.points(me.score ?? 0)} · ${me.of ? t.rank(me.rank ?? 0, me.of) : t.rankOnly(me.rank ?? 0)}`),
    ),
  );
}

/** @param {number} n */
function formatNumber(n) {
  return new Intl.NumberFormat(lang).format(n);
}

/**
 * Between tasks: what the TV shows, what happened to you.
 * @param {any} me
 * @param {any} room
 */
function watch(me, room) {
  if (me.note === "gallery") {
    return scoreCard(t.galleryTitle, me, h("p", { class: "big" }, me.mine ? t.galleryMine(me.mine) : t.galleryOthers));
  }
  if (me.note === "rest") {
    return scoreCard(t.drawingOf(me.artist?.name), me, h("p", { class: "big" }, t.notLying));
  }
  const r = me.result;
  if (me.note === "reveal" && r) {
    const lines = [];
    if (r.kind === "artist") lines.push(t.artistResult(r.count ?? 0, r.seen ?? 0));
    if (r.kind === "found") lines.push(t.found);
    if (r.kind === "fooled") lines.push(r.by?.length ? t.fellFor(r.by) : t.fellForOurs);
    if (r.lie !== undefined) lines.push(t.lieFooled(r.lie, r.fooledCount ?? 0));
    return scoreCard(
      r.points > 0 ? `+${formatNumber(r.points)}` : t.lookAtTv,
      me,
      ...lines.map((l) => h("p", { class: "big" }, l)),
    );
  }
  return scoreCard(t.lookAtTv, me);
}

/** Who may join during a game: the VIP's choice (`late-join`). */
const LATE_JOIN = ["wait", "open", "closed"];

/**
 * Latecomers: three buttons for the VIP, a line for everybody else.
 * @param {HTMLElement} err
 */
function lateJoin(err) {
  const buttons = LATE_JOIN.map((mode) => /** @type {HTMLButtonElement} */ (h("button", { "data-mode": mode, onclick: () => act("late-join", { mode }, err).catch(() => {}) }, t.late[mode]?.[0])));
  const row = h("div", { class: "row seg" }, ...buttons);
  const line = h("p", { class: "muted" });
  const box = h("div", { class: "late" }, h("h2", {}, t.latecomers), row, line);
  const update = () => {
    const v = client.view;
    const mode = v.room?.lateJoin ?? "wait";
    const vip = (v.roster ?? []).find((/** @type {any} */ r) => r.id === client.you?.id)?.vip;
    row.hidden = !vip;
    for (const b of buttons) b.classList.toggle("chosen", b.dataset.mode === mode);
    line.textContent = t.late[mode]?.[1] ?? "";
  };
  return { box, update };
}

/** @param {Record<string, any>} view */
function lobby(view) {
  const list = h("ul", { class: "players" });
  const start = /** @type {HTMLButtonElement} */ (h("button", { class: "primary", id: "start", onclick: () => act("start", null, err).catch(() => {}) }, t.everybodyIn));
  const err = h("p", { class: "error" });
  const hint = h("p", { class: "muted" });
  const late = lateJoin(err);
  show(h("section", { class: "card lobby" }, h("h1", {}, t.youreIn), hint, h("h2", {}, t.players), list, start, err, late.box));
  refresh = () => {
    late.update();
    const v = client.view;
    const roster = v.roster ?? [];
    const meVip = roster.find((/** @type {any} */ r) => r.id === client.you?.id)?.vip;
    const min = v.room?.minPlayers ?? 3;
    const ready = roster.filter((/** @type {any} */ r) => r.connected).length;
    list.replaceChildren(
      ...roster.map((/** @type {any} */ r) =>
        h(
          "li",
          { class: r.connected ? "" : "away" },
          h("span", { class: "name" }, r.name),
          r.vip ? h("span", { class: "vip" }, t.vip) : null,
          meVip && r.id !== client.you?.id ? h("button", { onclick: () => act("kick", { id: r.id }, err).catch(() => {}) }, t.remove) : null,
        ),
      ),
    );
    start.hidden = !meVip;
    start.disabled = ready < min;
    hint.textContent = meVip ? (ready < min ? t.vipWait(min - ready) : t.vipReady) : t.waitVip;
  };
}

/** @param {Record<string, any>} view */
function final(view) {
  const me = view.me ?? {};
  const err = h("p", { class: "error" });
  const again = /** @type {HTMLButtonElement} */ (h("button", { class: "primary", id: "start", onclick: () => act("start", null, err).catch(() => {}) }, t.playAgain));
  const late = lateJoin(err);
  show(
    h(
      "section",
      { class: "card" },
      h("h1", {}, me.rank === 1 ? t.won : t.gameOver),
      h("p", { class: "score" }, formatNumber(me.score ?? 0)),
      h("p", { class: "muted" }, `${t.points(me.score ?? 0)} · ${t.rankOnly(me.rank ?? 0)}`),
      again,
      err,
      late.box,
    ),
  );
  refresh = () => {
    const vip = (client.view.roster ?? []).find((/** @type {any} */ r) => r.id === client.you?.id)?.vip;
    again.hidden = !vip;
    late.update();
    late.box.hidden = !vip;
  };
}

/**
 * @param {string} type
 * @param {any} data
 * @param {HTMLElement} err
 */
async function act(type, data, err) {
  err.textContent = "";
  try {
    return await client.act(type, data);
  } catch (e) {
    err.textContent = message(e);
    throw e;
  }
}

// ---------------------------------------------------------------------------
// Drawing

class Pad {
  /** @param {HTMLCanvasElement} canvas */
  constructor(canvas) {
    this.canvas = canvas;
    this.ctx = /** @type {CanvasRenderingContext2D} */ (canvas.getContext("2d"));
    /** @type {number[][]} */
    this.strokes = [];
    this.points = 0;
    this.color = 0;
    this.width = WIDTHS[1];
    /** @type {number[] | null} */
    this.current = null;
    this.size = 0;
    this.resize();
    new ResizeObserver(() => this.resize()).observe(canvas);
    canvas.addEventListener("pointerdown", (e) => this.down(e));
    canvas.addEventListener("pointermove", (e) => this.move(e));
    canvas.addEventListener("pointerup", () => this.up());
    canvas.addEventListener("pointercancel", () => this.up());
    // Pointer events alone don't stop iOS Safari from starting a text
    // selection, the magnifier or a scroll/zoom from a touch on the pad:
    // cancel the touch events themselves (non-passive).
    for (const type of ["touchstart", "touchmove", "touchend"]) {
      canvas.addEventListener(type, (e) => e.preventDefault(), { passive: false });
    }
  }

  resize() {
    const r = this.canvas.getBoundingClientRect();
    const dpr = Math.min(3, window.devicePixelRatio || 1);
    this.size = r.width;
    this.canvas.width = Math.round(r.width * dpr);
    this.canvas.height = Math.round(r.width * dpr);
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    this.redraw();
  }

  redraw() {
    drawDrawing(this.ctx, { strokes: this.strokes }, 0, 0, this.size);
  }

  /** @param {PointerEvent} e */
  at(e) {
    const r = this.canvas.getBoundingClientRect();
    const k = GRID / r.width;
    return [Math.round(Math.min(GRID, Math.max(0, (e.clientX - r.left) * k))), Math.round(Math.min(GRID, Math.max(0, (e.clientY - r.top) * k)))];
  }

  /** @param {PointerEvent} e */
  down(e) {
    if (this.strokes.length >= MAX_STROKES || this.points >= MAX_POINTS) return;
    e.preventDefault();
    this.canvas.setPointerCapture(e.pointerId);
    const [x, y] = this.at(e);
    this.current = [this.color, this.width, x, y];
    this.strokes.push(this.current);
    this.points++;
    drawStroke(this.ctx, this.current, 0, 0, this.size);
  }

  /** @param {PointerEvent} e */
  move(e) {
    const s = this.current;
    if (!s) return;
    e.preventDefault();
    for (const ev of e.getCoalescedEvents?.() ?? [e]) {
      if (this.points >= MAX_POINTS) return this.up();
      const [x, y] = this.at(ev);
      const lx = s[s.length - 2];
      const ly = s[s.length - 1];
      if (Math.hypot(x - lx, y - ly) < 4) continue;
      s.push(x, y);
      this.points++;
      drawStroke(this.ctx, [s[0], s[1], lx, ly, x, y], 0, 0, this.size);
    }
  }

  up() {
    this.current = null;
  }

  clear() {
    this.strokes = [];
    this.points = 0;
    this.redraw();
  }
}

/** @param {string} prompt */
function draw(prompt) {
  const canvas = /** @type {HTMLCanvasElement} */ (h("canvas", { class: "pad" }));
  const err = h("p", { class: "error" });
  const palette = h("div", { class: "palette" });
  const tools = h("div", { class: "tools" });
  const done = /** @type {HTMLButtonElement} */ (h("button", { class: "primary", id: "done", onclick: () => submit() }, t.done));
  show(h("section", { class: "card" }, h("h2", {}, t.draw), h("p", { class: "prompt" }, prompt), h("div", { class: "pad-wrap" }, canvas), palette, tools, done, err));
  const p = new Pad(canvas);
  pad = p;
  const swatches = PALETTE.map((c, i) => {
    const b = h("button", { style: `background:${c}`, "aria-label": t.colour(i + 1), onclick: () => pick(i) });
    palette.append(b);
    return b;
  });
  /** @param {number} i */
  const pick = (i) => {
    p.color = i;
    swatches.forEach((b, j) => b.classList.toggle("chosen", j === i));
  };
  pick(0);
  const sizes = WIDTHS.map((w, i) => {
    const b = h("button", { onclick: () => size(i) }, t.sizes[i] ?? "");
    tools.append(b);
    return b;
  });
  /** @param {number} i */
  const size = (i) => {
    p.width = WIDTHS[i];
    sizes.forEach((b, j) => b.classList.toggle("chosen", j === i));
  };
  size(1);
  tools.append(h("button", { onclick: () => p.clear() }, t.clear));
  let sent = false;
  const submit = async () => {
    if (sent) return;
    sent = true;
    done.disabled = true;
    try {
      await act("draw", { strokes: p.strokes }, err);
    } catch {
      sent = false;
      done.disabled = false;
    }
  };
  // Time's up: send what is there.
  refresh = () => {};
  autoSubmit = () => {
    if (!sent && pad === p) submit();
  };
}

/** @type {(() => void) | null} */
let autoSubmit = null;

// ---------------------------------------------------------------------------
// Lies and votes

/** @param {any} drawing */
function mini(drawing) {
  const c = /** @type {HTMLCanvasElement} */ (h("canvas", { class: "mini" }));
  requestAnimationFrame(() => {
    const r = c.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    c.width = Math.round(r.width * dpr);
    c.height = Math.round(r.width * dpr);
    const ctx = /** @type {CanvasRenderingContext2D} */ (c.getContext("2d"));
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    drawDrawing(ctx, drawing, 0, 0, r.width);
  });
  return c;
}

/** @param {any} me */
function lie(me) {
  const drawing = me.drawing ?? client.view.room?.drawing;
  // As long as the prompts at most (LIE_SOFT in game/rules.ts), no capitals
  // (titles are shown in lowercase anyway); autocorrect stays on.
  const input = /** @type {HTMLInputElement} */ (
    h("input", { maxlength: LIE_SOFT, placeholder: t.liePlaceholder, enterkeyhint: "send", autocapitalize: "none", autocomplete: "off", spellcheck: "true", lang })
  );
  const err = h("p", { class: "error" });
  const send = async (/** @type {any} */ data) => {
    try {
      await act("lie", data, err);
    } catch {}
  };
  const form = h(
    "form",
    {
      autocomplete: "off",
      onsubmit: (/** @type {Event} */ e) => {
        e.preventDefault();
        if (input.value.trim()) send({ text: input.value });
      },
    },
    input,
    h("button", { type: "submit", class: "primary" }, t.send),
    err,
  );
  const role = t.liarFor(me.artist?.name, me.liars ?? 0);
  // Early: the first drawing's liars write during the gallery, the next
  // drawing's during the current vote (which they sit out).
  const hint = h("p", { class: "muted" });
  show(h("section", { class: "card" }, h("h2", {}, t.fakeTitle), h("p", { class: "role" }, role), me.early ? hint : null, mini(drawing), form));
  refresh = () => {
    const phase = client.view.room?.phase;
    hint.textContent = phase === "draw" || phase === "gallery" ? t.firstAfterGallery : t.nextSitOut;
  };
}

/**
 * @param {{ id: string, text: string }[]} options
 * @param {any} drawing
 */
function vote(options, drawing) {
  const err = h("p", { class: "error" });
  const list = h("div", { class: "options" });
  for (const o of options) {
    const b = h(
      "button",
      {
        onclick: async () => {
          list.querySelectorAll("button").forEach((x) => x.classList.toggle("chosen", x === b));
          try {
            await act("vote", { option: o.id }, err);
          } catch {}
        },
      },
      o.text,
    );
    list.append(b);
  }
  show(h("section", { class: "card" }, h("h2", {}, t.whichReal), mini(drawing), list, err));
}

// ---------------------------------------------------------------------------
// The clock

setInterval(() => {
  const deadline = client.view.me?.deadline ?? client.view.room?.deadline;
  const task = client.view.me?.task;
  const active = typeof deadline === "number" && ["draw", "lie", "vote"].includes(task) && !client.view.me?.done;
  timer.hidden = !active;
  if (!active) return;
  const left = deadline - client.now();
  timer.textContent = String(Math.max(0, Math.ceil(left / 1000)));
  timer.classList.toggle("low", left < 10_000);
  if (task === "draw" && left < 800) autoSubmit?.();
}, 200);

// Keep the screen on while playing.
async function wake() {
  try {
    await /** @type {any} */ (navigator).wakeLock?.request("screen");
  } catch {}
}
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") wake();
});

// ---------------------------------------------------------------------------

/** @type {HTMLElement} */ (app.querySelector(".big")).textContent = t.connecting;
if (!client.room || !client.key) {
  show(h("section", { class: "card" }, h("p", { class: "big" }, t.scanToJoin)));
} else if (client.canResume) {
  client.connect().then(wake, (e) => joinForm(e?.code === "no-player" ? "" : message(e)));
} else {
  joinForm();
}
