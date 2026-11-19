// @ts-check
/**
 * drawbluff's phone page: joining, the lobby, the drawing pad, lies,
 * votes, scores. Everything it shows comes from the party's view
 * (`{ roster, room, me }`); everything it does is an action.
 */

import { PartyClient } from "/party/client.js";
import { drawDrawing, drawStroke, GRID, PALETTE } from "/strokes.js";

const MAX_STROKES = 300;
const MAX_POINTS = 12000;
const WIDTHS = [8, 20, 48];
/** The longest lie the input takes (`LIE_SOFT` in game/rules.ts and src/rules.rs). */
const LIE_SOFT = 32;

const client = PartyClient.fromLocation();
const app = /** @type {HTMLElement} */ (document.getElementById("app"));
const banner = /** @type {HTMLElement} */ (document.getElementById("banner"));
const timer = /** @type {HTMLElement} */ (document.getElementById("timer"));
const who = /** @type {HTMLElement} */ (document.getElementById("who"));

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

function joinForm(error = "") {
  shown = "join";
  refresh = null;
  const tpl = /** @type {HTMLTemplateElement} */ (document.getElementById("t-join"));
  show(tpl.content.cloneNode(true));
  const input = /** @type {HTMLInputElement} */ (document.getElementById("name"));
  const err = /** @type {HTMLElement} */ (document.getElementById("join-error"));
  input.value = client.saved?.name ?? localStorage.getItem("drawbluff:name") ?? "";
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
      return "Someone already has that name.";
    case "name-invalid":
      return e.message || "Pick another name.";
    case "full":
      return "The game is full.";
    case "locked":
      return "A game is running. Wait for the next one.";
    case "bad-key":
      return "This link does not work any more. Scan the code on the TV again.";
    case "no-room":
      return "That game is over. Scan the code on the TV again.";
    default:
      return e?.message ?? String(e);
  }
}

client.on("status", (s) => {
  if (s === "reconnecting") setBanner("Reconnecting…");
  else if (s === "open") setBanner(null);
});

client.on("bye", (reason) => {
  const text =
    reason === "kicked"
      ? "You were removed from the game."
      : reason === "replaced"
        ? "You opened the game somewhere else."
        : reason === "room-closed"
          ? "The game is over. Thanks for playing!"
          : null;
  if (text) {
    shown = "bye";
    refresh = null;
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
  const rank = typeof me.rank === "number" && typeof me.of === "number" && me.of > 0 && me.task !== "lobby" ? ` · #${me.rank} of ${me.of}` : "";
  who.textContent = mine ? `${mine.vip ? "★ " : ""}${mine.name}${rank}` : "";
  const key = screenKey(me, room);
  if (key !== shown) {
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
      return me.done ? waiting("Nice! Waiting for the others to finish…") : draw(me.prompt);
    case "wait":
      return waiting("You're in! You join the game at the next round. Watch the TV meanwhile.");
    case "artist":
      return waiting("That's your drawing! Watch the others try to name it.");
    case "lie":
      return me.done
        ? waiting(`Your lie: “${me.lie ?? ""}”. ${me.early ? "That drawing is next on the TV." : "Waiting for the other liars…"}`)
        : lie(me);
    case "vote":
      return me.done ? waiting("Vote counted. Look at the TV!") : vote(me.options ?? [], room.drawing);
    case "watch":
      return watch(me, room);
    case "scores":
      return scoreCard(`Round ${room.round} done`, me);
    case "final":
      return final(view);
    default:
      return waiting("Look at the TV!");
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
      h("p", { class: "score" }, String(me.score ?? 0)),
      h("p", { class: "muted" }, `points · #${me.rank ?? "?"}${me.of ? ` of ${me.of}` : ""}`),
    ),
  );
}

/** @param {string[]} names */
function list(names) {
  return names.length <= 1 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} & ${names[names.length - 1]}`;
}

/**
 * Between tasks: what the TV shows, what happened to you.
 * @param {any} me
 * @param {any} room
 */
function watch(me, room) {
  if (me.note === "gallery") {
    return scoreCard("Look at the TV: the gallery!", me, h("p", { class: "big" }, me.mine ? `Your drawing is on page ${me.mine}.` : "The other drawings of this round."));
  }
  if (me.note === "rest") {
    return scoreCard(
      `${me.artist?.name ?? "Someone"}'s drawing`,
      me,
      h("p", { class: "big" }, "You're not lying on this one. The liars are writing; you vote next."),
    );
  }
  const r = me.result;
  if (me.note === "reveal" && r) {
    const lines = [];
    if (r.kind === "artist") lines.push(`${r.count} of ${r.seen} found your drawing.`);
    if (r.kind === "found") lines.push("You found the real title!");
    if (r.kind === "fooled") lines.push(r.by?.length ? `You fell for ${list(r.by)}'s lie.` : "You fell for our lie.");
    if (r.lie !== undefined) lines.push(`Your lie “${r.lie}” fooled ${r.fooledCount === 1 ? "1 player" : `${r.fooledCount} players`}.`);
    return scoreCard(
      r.points > 0 ? `+${r.points}` : "Look at the TV!",
      me,
      ...lines.map((l) => h("p", { class: "big" }, l)),
    );
  }
  return scoreCard("Look at the TV!", me);
}

/** @param {Record<string, any>} view */
function lobby(view) {
  const list = h("ul", { class: "players" });
  const start = /** @type {HTMLButtonElement} */ (h("button", { class: "primary", onclick: () => act("start", null, err).catch(() => {}) }, "Everybody's in!"));
  const err = h("p", { class: "error" });
  const hint = h("p", { class: "muted" });
  show(h("section", { class: "card" }, h("h1", {}, "You're in!"), hint, h("h2", {}, "Players"), list, start, err));
  refresh = () => {
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
          r.vip ? h("span", { class: "vip" }, "★ VIP") : null,
          meVip && r.id !== client.you?.id ? h("button", { onclick: () => act("kick", { id: r.id }, err).catch(() => {}) }, "remove") : null,
        ),
      ),
    );
    start.hidden = !meVip;
    start.disabled = ready < min;
    hint.textContent = meVip
      ? ready < min
        ? `You're the VIP. Start when ${min - ready} more joined.`
        : "You're the VIP: start when everybody's in."
      : "Waiting for the VIP to start the game.";
  };
}

/** @param {Record<string, any>} view */
function final(view) {
  const me = view.me ?? {};
  const err = h("p", { class: "error" });
  const again = /** @type {HTMLButtonElement} */ (h("button", { class: "primary", onclick: () => act("start", null, err).catch(() => {}) }, "Play again"));
  show(
    h(
      "section",
      { class: "card" },
      h("h1", {}, me.rank === 1 ? "You won!" : "Game over"),
      h("p", { class: "score" }, String(me.score ?? 0)),
      h("p", { class: "muted" }, `points · #${me.rank ?? "?"}`),
      again,
      err,
    ),
  );
  refresh = () => {
    const vip = (client.view.roster ?? []).find((/** @type {any} */ r) => r.id === client.you?.id)?.vip;
    again.hidden = !vip;
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
    err.textContent = /** @type {any} */ (e).message;
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

  undo() {
    const s = this.strokes.pop();
    if (s) this.points -= (s.length - 2) / 2;
    this.redraw();
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
  const done = /** @type {HTMLButtonElement} */ (h("button", { class: "primary", onclick: () => submit() }, "Done!"));
  show(h("section", { class: "card" }, h("h2", {}, "Draw"), h("p", { class: "prompt" }, prompt), h("div", { class: "pad-wrap" }, canvas), palette, tools, done, err));
  const p = new Pad(canvas);
  pad = p;
  const swatches = PALETTE.map((c, i) => {
    const b = h("button", { style: `background:${c}`, "aria-label": `colour ${i + 1}`, onclick: () => pick(i) });
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
    const b = h("button", { onclick: () => size(i) }, ["thin", "medium", "thick"][i] ?? "");
    tools.append(b);
    return b;
  });
  /** @param {number} i */
  const size = (i) => {
    p.width = WIDTHS[i];
    sizes.forEach((b, j) => b.classList.toggle("chosen", j === i));
  };
  size(1);
  tools.append(h("button", { onclick: () => p.undo() }, "undo"), h("button", { onclick: () => p.clear() }, "clear"));
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
    h("input", { maxlength: LIE_SOFT, placeholder: "a convincing title", enterkeyhint: "send", autocapitalize: "none", autocomplete: "off", spellcheck: "true" })
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
    h("div", { class: "row" }, h("button", { type: "button", onclick: () => send({ auto: true }) }, "Lie for me"), h("button", { type: "submit", class: "primary", style: "margin-top:0" }, "Send")),
    err,
  );
  const role = `You're a liar for ${me.artist?.name ?? "this"}'s drawing${me.liars > 1 ? ` (one of ${me.liars})` : ""}.`;
  show(
    h(
      "section",
      { class: "card" },
      h("h2", {}, "Fake a title"),
      h("p", { class: "role" }, role),
      me.early ? h("p", { class: "muted" }, "It's next on the TV: you sit this vote out.") : null,
      mini(drawing),
      form,
    ),
  );
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
  show(h("section", { class: "card" }, h("h2", {}, "Which is the real title?"), mini(drawing), list, err));
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

if (!client.room || !client.key) {
  show(h("section", { class: "card" }, h("p", { class: "big" }, "Scan the code on the TV to join.")));
} else if (client.canResume) {
  client.connect().then(wake, (e) => joinForm(e?.code === "no-player" ? "" : message(e)));
} else {
  joinForm();
}
