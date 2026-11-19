// @ts-check
/**
 * drawbluff's main screen as a web page (variant 2): tvpipe casts it with
 * its own browser (`share URL`), or anyone opens the watch link. It shows
 * the screen view (`{ roster, screen, join }`); the video variant draws
 * the same view with screen-video.ts.
 */

import { PartyClient, qrSvg } from "/party/client.js";
import { drawDrawing } from "/strokes.js";

const client = PartyClient.fromLocation({ role: /** @type {any} */ (new URLSearchParams(location.hash.slice(1)).get("role") ?? "screen") });
const stage = /** @type {HTMLElement} */ (document.getElementById("stage"));
const corner = /** @type {HTMLElement} */ (document.getElementById("corner"));
const status = /** @type {HTMLElement} */ (document.getElementById("status"));

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

/** @type {AudioContext | null} */
let audio = null;
/**
 * @param {number[]} notes frequencies, played one after the other
 * @param {number} [len] seconds per note
 */
function beep(notes, len = 0.12) {
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
  status.textContent = s === "reconnecting" || s === "connecting" ? "Reconnecting to the game…" : s === "closed" ? "The game is over." : "";
});

/** @param {Record<string, any>} view */
function render(view) {
  const s = view.screen ?? {};
  const key = `${s.phase}:${s.round ?? ""}:${s.deadline ?? ""}:${s.artist?.id ?? ""}:${s.page ?? ""}`;
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
  // Not in the lobby (the big code), nor where the options fill the right side.
  corner.hidden = s.phase === "lobby" || s.phase === "vote" || s.phase === "reveal" || !s.phase || !view.join?.qr;
  if (!corner.hidden && !corner.dataset.url) {
    corner.dataset.url = view.join.url;
    /** @type {HTMLElement} */ (document.getElementById("corner-qr")).innerHTML = qrSvg(view.join.qr, { quiet: 2 });
    /** @type {HTMLElement} */ (document.getElementById("corner-text")).textContent = "Scan to join (from the next round) or to rejoin.";
  }
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
  stage.replaceChildren(
    h(
      "div",
      { class: "col join" },
      h("div", { class: "logo" }, "drawbluff"),
      h("h2", {}, "Scan to join with your phone"),
      qr(view.join),
      h("div", { class: "url" }, view.join?.url ?? ""),
      h("div", { class: "code" }, view.join?.code ?? ""),
    ),
    h("div", { class: "col grow" }, h("h1", {}, "Players"), players, hint),
  );
  refresh = () => {
    const v = client.view;
    const roster = v.roster ?? [];
    const max = v.screen?.maxPlayers ?? 8;
    const min = v.screen?.minPlayers ?? 3;
    const cards = roster.map((/** @type {any} */ r) =>
      h("div", { class: `player${r.connected ? "" : " away"}` }, h("span", { class: "n" }, r.name), r.vip ? h("span", { class: "star" }, "★ VIP") : null),
    );
    // Empty seats up to 8; past 8 players a denser grid (up to 50 names).
    for (let i = roster.length; i < Math.min(max, 8); i++) cards.push(h("div", { class: "player empty" }, i < min ? "waiting…" : "room for one more"));
    players.className = `players${roster.length > 24 ? " many" : roster.length > 8 ? " more" : ""}`;
    players.replaceChildren(...cards);
    const ready = roster.filter((/** @type {any} */ r) => r.connected).length;
    hint.textContent =
      ready < min
        ? `${min - ready} more player${min - ready === 1 ? "" : "s"} needed.`
        : `${roster.find((/** @type {any} */ r) => r.vip)?.name ?? "The VIP"} starts the game on their phone.`;
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
 * @param {string} [verb] what they do ("done", "voted")
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
    box.replaceChildren(h("div", { class: "count" }, `${n} of ${waiting.length} ${verb}`), bar);
    if (left.length > 0 && left.length <= 6) box.append(h("div", { class: "left" }, `Waiting for ${left.map(name).join(", ")}`));
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
  const t = timer();
  const c = chips(view);
  stage.replaceChildren(
    h(
      "div",
      { class: "col grow center" },
      h("h2", {}, `Round ${s.round} of ${s.rounds}`),
      h("div", { class: "big-title" }, "Draw your prompt on your phone!"),
      t.el,
      c.box,
    ),
  );
  ticker = t.update;
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
    h("div", { class: "cell" }, canvas(it.drawing, "thumb"), h("div", { class: "cap" }, h("b", {}, it.prompt), h("span", {}, `by ${it.name}`))),
  );
  const liars = (s.liars ?? []).length;
  stage.replaceChildren(
    h(
      "div",
      { class: "col grow" },
      h("h2", {}, `Round ${s.round} · the gallery${s.pages > 1 ? ` · ${s.page} of ${s.pages}` : ""}`),
      h("div", { class: "gallery" }, ...cells),
      liars > 0 ? h("div", { class: "meanwhile" }, line) : null,
    ),
  );
  refresh = () => {
    const v = client.view.screen ?? {};
    line.textContent = `Meanwhile ${liars === 1 ? "1 liar is" : `${liars} liars are`} writing for ${v.artist?.name ?? "the first"}'s drawing · ${(v.done ?? []).length} done`;
  };
}

/** @param {Record<string, any>} view */
function guess(view) {
  const s = view.screen;
  const t = timer();
  const spot = s.spots > 1 ? ` · drawing ${s.spot} of ${s.spots}` : "";
  const liars = (s.waiting ?? []).length;
  const c = s.phase === "lie" ? chips(view) : chips(view, "waiting", "voted");
  const right =
    s.phase === "lie"
      ? h(
          "div",
          { class: "col grow" },
          h("h2", {}, `${s.artist?.name ?? "Someone"} drew this${spot}`),
          h("h1", {}, liars === 1 ? "What is it called? The liar is writing a fake title." : `What is it called? ${liars} liars are writing fake titles.`),
          t.el,
          c.box,
        )
      : h(
          "div",
          { class: "col grow" },
          h("h2", {}, `Which one is the real title?${spot}`),
          h("div", { class: "options" }, ...(s.options ?? []).map((/** @type {any} */ o) => h("div", { class: "option" }, o.text))),
          h("div", { style: "display:flex;gap:2vw;align-items:center;margin-top:3vh" }, t.el, c.box),
          s.writing > 0 ? h("p", { class: "hint small" }, `Meanwhile ${s.writing} liars write for the next drawing.`) : null,
        );
  stage.replaceChildren(canvas(s.drawing), right);
  ticker = t.update;
  refresh = c.update;
}

/** @param {Record<string, any>} view */
function reveal(view) {
  const s = view.screen;
  const items = (s.items ?? []).map((/** @type {any} */ it) => {
    const count = it.count ?? it.voters.length;
    // Names while they fit, else how many of those who could.
    const names = count <= it.voters.length ? it.voters.join(", ") : `${count} of ${it.seen}`;
    const who = it.truth
      ? count
        ? `found by ${names} · +${it.points} each, ${s.artist?.name} +${it.artistPoints}`
        : "nobody found it"
      : `${count <= it.voters.length ? `${names} fell for it` : `fooled ${names}`}${it.decoy ? " (our lie)" : ` · ${it.authors.join(" & ")} +${it.points}${it.same ? " (same idea!)" : ""}`}`;
    return h(
      "div",
      { class: `option hidden${it.truth ? " truth" : ""}` },
      h("span", { class: "tag" }, it.truth ? `THE TRUTH by ${s.artist?.name ?? "?"}` : it.decoy ? "A LIE" : `A LIE by ${it.authors.join(" & ")}`),
      h("span", {}, it.text),
      h("span", { class: "who" }, who),
    );
  });
  stage.replaceChildren(canvas(s.drawing), h("div", { class: `col grow reveal${items.length > 5 ? " many" : ""}` }, h("h2", {}, "The answers"), h("div", { class: "options" }, ...items)));
  refresh = () => {
    const step = client.view.screen?.step ?? 0;
    items.forEach((el, i) => el.classList.toggle("hidden", i >= step));
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
      h("span", { class: "rank" }, `${i + 1}.`),
      h("span", { class: "n" }, l.name),
      !final && l.delta ? h("span", { class: "delta" }, `+${l.delta}`) : null,
      h("span", { class: "pts" }, String(l.score)),
    ),
  );
  const title = final ? `${s.scores?.[0]?.name ?? "Nobody"} wins!` : s.last ? "Final scores" : `Scores after round ${s.round}`;
  const highlights = (s.highlights ?? []).map((/** @type {any} */ x) =>
    h(
      "div",
      { class: "highlight" },
      h("span", { class: "tag" }, x.kind === "liar" ? "Best liar" : "Sharpest eye"),
      h("span", { class: "n" }, x.name),
      h("span", {}, x.kind === "liar" ? `fooled ${x.count}` : `found ${x.count} of ${x.of}`),
    ),
  );
  stage.replaceChildren(
    h(
      "div",
      { class: "col grow center" },
      h("div", { class: "big-title" }, title),
      h("div", { class: "board-row" }, h("div", { class: "board" }, ...lines), highlights.length ? h("div", { class: "highlights" }, ...highlights) : null),
      total > top.length ? h("p", { class: "hint" }, `${total} players · your rank is on your phone`) : null,
      final ? h("p", { class: "hint" }, "The VIP can start another game on their phone.") : null,
    ),
  );
  ticker = null;
}

client.connect().catch((e) => {
  stage.replaceChildren(h("div", { class: "col grow center" }, h("h1", {}, `Cannot show the game: ${e?.message ?? e}`)));
});
