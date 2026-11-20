// @ts-check
/**
 * drawbluff's TV page: tvpipe casts it with its own browser, or anyone
 * opens the watch link (the host page's preview). It shows the screen
 * view (`{ roster, screen, join }`) in the room's language
 * (`screen.lang`).
 */

import { PartyClient, qrSvg } from "/_tvpipe/lib/client.js";
import { pickLang } from "../phone/i18n.js";
import { drawDrawing } from "../phone/strokes.js";
import { strings } from "./strings.js";

const client = PartyClient.fromLocation({ role: /** @type {any} */ (new URLSearchParams(location.hash.slice(1)).get("role") ?? "screen") });
const stage = /** @type {HTMLElement} */ (document.getElementById("stage"));
const corner = /** @type {HTMLElement} */ (document.getElementById("corner"));
const status = /** @type {HTMLElement} */ (document.getElementById("status"));
const joined = /** @type {HTMLElement} */ (document.getElementById("joined"));

/** The room's language (English until the first view says). */
let lang = pickLang(undefined);
let t = strings(lang);

/**
 * @param {string} tag
 * @param {Record<string, any>} [attrs]
 * @param {...(Node | string | null | undefined | false)} kids
 * @returns {HTMLElement}
 */
function h(tag, attrs = {}, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") el.className = v;
    else if (v !== false && v != null) el.setAttribute(k, String(v));
  }
  for (const kid of kids) if (kid) el.append(kid);
  return el;
}

/** The QR code: SVG made from the module rows (no text from players in it). */
function qr(/** @type {any} */ join) {
  const box = h("div", { class: "qr" });
  box.innerHTML = qrSvg(join?.qr, { quiet: 3 });
  return box;
}

// ---------------------------------------------------------------------------
// Sound: short synthesized cues (tvpipe's browser allows autoplay).

/** The host window's preview has `#mute=1`: the TV plays the sound. */
const MUTED = new URLSearchParams(location.hash.slice(1)).get("mute") === "1";
/** @type {AudioContext | null} */
let audio = null;
/**
 * @param {number[]} notes frequencies, played one after the other
 * @param {number} [len] seconds per note
 */
function beep(notes, len = 0.12) {
  if (MUTED) return;
  try {
    audio ??= new AudioContext();
    if (audio.state === "suspended") void audio.resume();
    let t = audio.currentTime + 0.02;
    for (const f of notes) {
      const o = audio.createOscillator();
      const g = audio.createGain();
      o.type = "triangle";
      o.frequency.value = f;
      g.gain.setValueAtTime(0.0001, t);
      g.gain.exponentialRampToValueAtTime(0.25, t + 0.01);
      g.gain.exponentialRampToValueAtTime(0.0001, t + len);
      o.connect(g).connect(audio.destination);
      o.start(t);
      o.stop(t + len + 0.02);
      t += len * 0.9;
    }
  } catch {}
}

// ---------------------------------------------------------------------------

let shown = "";
let lastPhase = "";
let lastStep = 0;
/** @type {(() => void) | null} */
let refresh = null;

client.on("view", (view) => render(view));
client.on("status", (s) => {
  status.hidden = s === "open";
  status.textContent = s === "reconnecting" || s === "connecting" ? t.reconnecting : s === "closed" ? t.over : "";
});

/** @param {Record<string, any>} view */
function render(view) {
  const s = view.screen ?? {};
  const l = pickLang(s.lang);
  if (l !== lang) {
    lang = l;
    t = strings(l);
    document.documentElement.lang = l;
  }
  const key = `${lang}:${s.phase}:${s.round ?? ""}:${s.deadline ?? ""}:${s.artist?.id ?? ""}:${s.page ?? ""}`;
  if (s.phase !== lastPhase) {
    if (s.phase === "draw" || s.phase === "lie" || s.phase === "vote" || s.phase === "gallery") beep([523, 659, 784]);
    if (s.phase === "final") beep([523, 659, 784, 1047], 0.18);
    lastPhase = s.phase;
    lastStep = 0;
  }
  if (key !== shown) {
    shown = key;
    refresh = null;
    build(view);
  }
  refresh?.();
  // Not in the lobby (the big code), nor where the options fill the
  // right side, nor when the game is closed to newcomers.
  const late = /** @type {"open" | "wait" | "closed"} */ (view.join?.lateJoin ?? "wait");
  corner.hidden = s.phase === "lobby" || s.phase === "vote" || s.phase === "reveal" || !s.phase || !view.join?.qr || late === "closed";
  if (!corner.hidden && corner.dataset.url !== view.join.url) {
    corner.dataset.url = view.join.url;
    /** @type {HTMLElement} */ (document.getElementById("corner-qr")).innerHTML = qrSvg(view.join.qr, { quiet: 2 });
  }
  if (!corner.hidden) /** @type {HTMLElement} */ (document.getElementById("corner-text")).textContent = t.corner[late] ?? "";
  // This round's newcomers, discreetly.
  const names = /** @type {string[]} */ (s.phase !== "lobby" && Array.isArray(s.joined) ? s.joined : []);
  const more = typeof s.joinedMore === "number" ? s.joinedMore : 0;
  joined.hidden = names.length === 0;
  joined.textContent = [...names.map((n) => `+ ${n}`), ...(more > 0 ? [t.joinedMore(more)] : [])].join(" · ");
}

/** @param {Record<string, any>} view */
function build(view) {
  const s = view.screen ?? {};
  ticker = null;
  switch (s.phase) {
    case "lobby":
      return lobby(view);
    case "draw":
      return draw(view);
    case "gallery":
      return gallery(view);
    case "lie":
    case "vote":
      return guess(view);
    case "reveal":
      return reveal(view);
    case "scores":
    case "final":
      return board(view);
    default:
      stage.replaceChildren(h("div", { class: "col grow center" }, h("div", { class: "logo", style: "font-size:8vh" }, "drawbluff")));
  }
}

/** @param {Record<string, any>} view */
function lobby(view) {
  const players = h("div", { class: "players" });
  const hint = h("p", { class: "hint" });
  const lateLine = h("div", { class: "late" });
  const badges = h("div", { class: "badges" });
  stage.replaceChildren(
    h(
      "div",
      { class: "col join" },
      h("div", { class: "logo" }, "drawbluff"),
      h("h2", {}, t.scanToJoin),
      qr(view.join),
      h("div", { class: "url" }, view.join?.url ?? ""),
      h("div", { class: "code" }, view.join?.code ?? ""),
      lateLine,
    ),
    h("div", { class: "col grow" }, h("h1", {}, t.players), players, hint, badges),
  );
  refresh = () => {
    const v = client.view;
    const roster = v.roster ?? [];
    const late = /** @type {"open" | "wait" | "closed"} */ (v.join?.lateJoin ?? "wait");
    lateLine.textContent = t.late[late] ?? "";
    lateLine.classList.toggle("open", late === "open");
    const max = v.screen?.maxPlayers ?? 8;
    const min = v.screen?.minPlayers ?? 3;
    // Each player's colour (`screen.colors`, by id).
    const colors = v.screen?.colors ?? {};
    const cards = roster.map((/** @type {any} */ r) => {
      const c = colors[r.id];
      return h(
        "div",
        { class: `player${r.connected ? "" : " away"}` },
        typeof c === "string" && /^#[0-9a-f]{6}$/i.test(c) ? h("span", { class: "dot", style: `background:${c}` }) : null,
        h("span", { class: "n" }, r.name),
        r.vip ? h("span", { class: "star" }, "★ VIP") : null,
      );
    });
    // Empty seats up to 8; past 8 players a denser grid (up to 50 names).
    for (let i = roster.length; i < Math.min(max, 8); i++) cards.push(h("div", { class: "player empty" }, i < min ? t.seatWaiting : t.seatFree));
    players.className = `players${roster.length > 24 ? " many" : roster.length > 8 ? " more" : ""}`;
    players.replaceChildren(...cards);
    const ready = roster.filter((/** @type {any} */ r) => r.connected).length;
    hint.textContent = ready < min ? t.needed(min - ready) : t.vipStarts(roster.find((/** @type {any} */ r) => r.vip)?.name);
    // The rules: undo, colours, adult prompts.
    const s = v.screen ?? {};
    badges.replaceChildren(
      h("span", { class: "badge" }, s.undo ? t.undoOn : t.undoOff),
      h("span", { class: "badge" }, s.colorMode ? t.colorMode : t.shades),
      ...(s.adult ? [h("span", { class: "badge hot" }, t.adult)] : []),
    );
    if (roster.length > lobbyCount) beep([660, 880]);
    lobbyCount = roster.length;
  };
}
let lobbyCount = 0;

/** Chips up to this many players waited for; more: a count, a bar and the last few names. */
const CHIPS = 12;

/**
 * Who has answered: a chip per player waited for, or the count.
 * @param {Record<string, any>} view
 * @param {string} [list] the screen field with the ids waited for
 * @param {"done" | "voted"} [verb] what they do
 */
function chips(view, list = "waiting", verb = "done") {
  const box = h("div", { class: "chips" });
  const update = () => {
    const s = client.view.screen ?? {};
    const roster = client.view.roster ?? [];
    const done = new Set(s.done ?? []);
    const waiting = /** @type {string[]} */ (s[list] ?? []);
    const name = (/** @type {string} */ id) => roster.find((/** @type {any} */ x) => x.id === id)?.name ?? "?";
    if (waiting.length <= CHIPS) {
      box.className = "chips";
      box.replaceChildren(...waiting.map((id) => h("span", { class: `chip${done.has(id) ? " done" : ""}` }, `${done.has(id) ? "✓ " : ""}${name(id)}`)));
      return;
    }
    const n = waiting.filter((id) => done.has(id)).length;
    const left = waiting.filter((id) => !done.has(id));
    const bar = h("div", { class: "bar" }, h("i", { style: `width:${Math.round((100 * n) / waiting.length)}%` }));
    box.className = "chips progress";
    // Not null in replaceChildren: it would show as "null".
    box.replaceChildren(h("div", { class: "count" }, t[verb](n, waiting.length)), bar);
    if (left.length > 0 && left.length <= 6) box.append(h("div", { class: "left" }, t.waitingFor(left.map(name))));
  };
  update();
  return { box, update };
}

function timer() {
  const el = h("div", { class: "timer" });
  const update = () => {
    const d = client.view.screen?.deadline;
    if (typeof d !== "number") return;
    const left = Math.max(0, Math.ceil((d - client.now()) / 1000));
    el.textContent = String(left);
    el.classList.toggle("low", left <= 10);
  };
  update();
  return { el, update };
}
setInterval(() => ticker?.(), 250);
/** @type {(() => void) | null} */
let ticker = null;

/** @param {Record<string, any>} view */
function draw(view) {
  const s = view.screen;
  const clock = timer();
  const c = chips(view);
  stage.replaceChildren(
    h(
      "div",
      { class: "col grow center" },
      h("h2", {}, t.roundOf(s.round, s.rounds)),
      h("div", { class: "big-title" }, t.drawPrompt),
      clock.el,
      c.box,
    ),
  );
  ticker = clock.update;
  refresh = c.update;
}

/**
 * @param {any} drawing
 * @param {string} [cls]
 */
function canvas(drawing, cls = "drawing") {
  const el = /** @type {HTMLCanvasElement} */ (h("canvas", { class: cls }));
  requestAnimationFrame(() => {
    const r = el.getBoundingClientRect();
    el.width = Math.round(r.width);
    el.height = Math.round(r.height);
    drawDrawing(el.getContext("2d"), drawing, 0, 0, el.width);
  });
  return el;
}

/**
 * The gallery: the round's drawings that do not get the stage.
 * @param {Record<string, any>} view
 */
function gallery(view) {
  const s = view.screen;
  const line = h("span", {});
  const cells = (s.items ?? []).map((/** @type {any} */ it) =>
    h("div", { class: "cell" }, canvas(it.drawing, "thumb"), h("div", { class: "cap" }, h("b", {}, it.prompt), h("span", {}, t.by(it.name)))),
  );
  const liars = (s.liars ?? []).length;
  stage.replaceChildren(
    h(
      "div",
      { class: "col grow" },
      h("h2", {}, t.gallery(s.round, s.page, s.pages)),
      h("div", { class: "gallery" }, ...cells),
      liars > 0 ? h("div", { class: "meanwhile" }, line) : null,
    ),
  );
  refresh = () => {
    const v = client.view.screen ?? {};
    line.textContent = t.meanwhile(liars, v.artist?.name, (v.done ?? []).length);
  };
}

/** @param {Record<string, any>} view */
function guess(view) {
  const s = view.screen;
  const clock = timer();
  const spot = s.spots > 1 ? t.spotOf(s.spot, s.spots) : "";
  const liars = (s.waiting ?? []).length;
  const c = s.phase === "lie" ? chips(view) : chips(view, "waiting", "voted");
  const right =
    s.phase === "lie"
      ? h(
          "div",
          { class: "col grow" },
          h("h2", {}, `${t.drewThis(s.artist?.name)}${spot}`),
          h("h1", {}, t.liarsWriting(liars)),
          clock.el,
          c.box,
        )
      : h(
          "div",
          { class: "col grow" },
          h("h2", {}, `${t.whichReal}${spot}`),
          h("div", { class: `options${(s.options ?? []).length >= 6 ? " many" : ""}` }, ...(s.options ?? []).map((/** @type {any} */ o) => h("div", { class: "option" }, o.text))),
          h("div", { style: "display:flex;gap:2vw;align-items:center;margin-top:3vh" }, clock.el, c.box),
          s.writing > 0 ? h("p", { class: "hint small" }, t.writingNext(s.writing)) : null,
        );
  stage.replaceChildren(canvas(s.drawing), right);
  ticker = clock.update;
  refresh = c.update;
}

/** @param {Record<string, any>} view */
function reveal(view) {
  const s = view.screen;
  const items = (s.items ?? []).map((/** @type {any} */ it) => {
    const count = it.count ?? it.voters.length;
    // Names while they fit, else how many of those who could.
    const all = count <= it.voters.length;
    const who = it.truth
      ? count
        ? t.foundBy(all ? it.voters.join(", ") : t.foundOf(count, it.seen), it.points, s.artist?.name, it.artistPoints)
        : t.nobodyFound
      : `${all ? t.fellForIt(it.voters) : t.fooledOf(count, it.seen)}${it.decoy ? t.ourLie : `${t.liarsGet(it.authors, it.points)}${it.same ? t.sameIdea : ""}`}`;
    return h(
      "div",
      { class: `option hidden${it.truth ? " truth" : ""}` },
      h("span", { class: "tag" }, it.truth ? t.truthBy(s.artist?.name) : it.decoy ? t.aLie : t.aLieBy(it.authors)),
      h("span", {}, it.text),
      h("span", { class: "who" }, who),
    );
  });
  stage.replaceChildren(canvas(s.drawing), h("div", { class: `col grow reveal${items.length > 5 ? " many" : ""}` }, h("h2", {}, t.answers), h("div", { class: "options" }, ...items)));
  refresh = () => {
    const step = client.view.screen?.step ?? 0;
    items.forEach((/** @type {HTMLElement} */ el, /** @type {number} */ i) => el.classList.toggle("hidden", i >= step));
    if (step > lastStep) {
      const it = s.items[step - 1];
      beep(it?.truth ? [784, 988, 1175] : [392, 330]);
      lastStep = step;
    }
  };
  ticker = null;
}

/** @param {Record<string, any>} view */
function board(view) {
  const s = view.screen;
  const final = s.phase === "final";
  // All players up to 8, else the top 5 (each phone shows its own rank).
  const total = s.total ?? (s.scores ?? []).length;
  const top = total > 8 ? (s.scores ?? []).slice(0, 5) : (s.scores ?? []);
  const lines = top.map((/** @type {any} */ l, /** @type {number} */ i) =>
    h(
      "div",
      { class: `line${i === 0 ? " first" : ""}` },
      h("span", { class: "rank" }, t.rank(i + 1)),
      h("span", { class: "n" }, l.name),
      !final && l.delta ? h("span", { class: "delta" }, `+${t.n(l.delta)}`) : null,
      h("span", { class: "pts" }, t.n(l.score)),
    ),
  );
  const title = final ? t.wins(s.scores?.[0]?.name) : s.last ? t.finalScores : t.scoresAfter(s.round);
  const highlights = (s.highlights ?? []).map((/** @type {any} */ x) =>
    h(
      "div",
      { class: "highlight" },
      h("span", { class: "tag" }, x.kind === "liar" ? t.bestLiar : t.sharpestEye),
      h("span", { class: "n" }, x.name),
      h("span", {}, x.kind === "liar" ? t.fooledN(x.count) : t.foundN(x.count, x.of)),
    ),
  );
  stage.replaceChildren(
    h(
      "div",
      { class: "col grow center" },
      h("div", { class: "big-title" }, title),
      h("div", { class: "board-row" }, h("div", { class: "board" }, ...lines), highlights.length ? h("div", { class: "highlights" }, ...highlights) : null),
      total > top.length ? h("p", { class: "hint" }, t.rankOnPhone(total)) : null,
      final ? h("p", { class: "hint" }, t.anotherGame) : null,
    ),
  );
  ticker = null;
}

client.connect().catch((e) => {
  stage.replaceChildren(h("div", { class: "col grow center" }, h("h1", {}, t.cannotShow(e?.message ?? String(e)))));
});
