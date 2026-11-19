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
  const key = `${s.phase}:${s.round ?? ""}:${s.deadline ?? ""}:${s.artist?.id ?? ""}`;
  if (s.phase !== lastPhase) {
    if (s.phase === "draw" || s.phase === "lie" || s.phase === "vote") beep([523, 659, 784]);
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
  corner.hidden = s.phase === "lobby" || !view.join?.qr;
  if (!corner.hidden && !corner.dataset.url) {
    corner.dataset.url = view.join.url;
    /** @type {HTMLElement} */ (document.getElementById("corner-qr")).innerHTML = qrSvg(view.join.qr, { quiet: 2 });
    /** @type {HTMLElement} */ (document.getElementById("corner-text")).textContent = "Lost your phone's page? Scan to rejoin.";
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
    for (let i = roster.length; i < max; i++) cards.push(h("div", { class: "player empty" }, i < min ? "waiting…" : "room for one more"));
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

/**
 * Who has answered: a chip per player waited for.
 * @param {Record<string, any>} view
 */
function chips(view) {
  const box = h("div", { class: "chips" });
  const update = () => {
    const s = client.view.screen ?? {};
    const roster = client.view.roster ?? [];
    const done = new Set(s.done ?? []);
    box.replaceChildren(
      ...(s.waiting ?? []).map((/** @type {string} */ id) => {
        const r = roster.find((/** @type {any} */ x) => x.id === id);
        return h("span", { class: `chip${done.has(id) ? " done" : ""}` }, `${done.has(id) ? "✓ " : ""}${r?.name ?? "?"}`);
      }),
    );
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

/** @param {any} drawing */
function canvas(drawing) {
  const el = /** @type {HTMLCanvasElement} */ (h("canvas", { class: "drawing" }));
  requestAnimationFrame(() => {
    const r = el.getBoundingClientRect();
    el.width = Math.round(r.width);
    el.height = Math.round(r.height);
    drawDrawing(el.getContext("2d"), drawing, 0, 0, el.width);
  });
  return el;
}

/** @param {Record<string, any>} view */
function guess(view) {
  const s = view.screen;
  const t = timer();
  const c = chips(view);
  const right =
    s.phase === "lie"
      ? h(
          "div",
          { class: "col grow" },
          h("h2", {}, `${s.artist?.name ?? "Someone"} drew this`),
          h("h1", {}, "What is it called? Write a fake title on your phone."),
          t.el,
          c.box,
        )
      : h(
          "div",
          { class: "col grow" },
          h("h2", {}, "Which one is the real title?"),
          h("div", { class: "options" }, ...(s.options ?? []).map((/** @type {any} */ o) => h("div", { class: "option" }, o.text))),
          h("div", { style: "display:flex;gap:2vw;align-items:center;margin-top:3vh" }, t.el, c.box),
        );
  stage.replaceChildren(canvas(s.drawing), right);
  ticker = t.update;
  refresh = c.update;
}

/** @param {Record<string, any>} view */
function reveal(view) {
  const s = view.screen;
  const items = (s.items ?? []).map((/** @type {any} */ it) => {
    const who = it.truth
      ? it.voters.length
        ? `found by ${it.voters.join(", ")} · +${it.points} each, ${s.artist?.name} +${it.artistPoints}`
        : "nobody found it"
      : `${it.voters.join(", ")} fell for it${it.decoy ? " (our lie)" : ` · ${it.authors.join(" & ")} +${it.points}`}`;
    return h(
      "div",
      { class: `option hidden${it.truth ? " truth" : ""}` },
      h("span", { class: "tag" }, it.truth ? `THE TRUTH by ${s.artist?.name ?? "?"}` : it.decoy ? "A LIE" : `A LIE by ${it.authors.join(" & ")}`),
      h("span", {}, it.text),
      h("span", { class: "who" }, who),
    );
  });
  stage.replaceChildren(canvas(s.drawing), h("div", { class: "col grow reveal" }, h("h2", {}, "The answers"), h("div", { class: "options" }, ...items)));
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
  const lines = (s.scores ?? []).map((/** @type {any} */ l, i) =>
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
  stage.replaceChildren(
    h(
      "div",
      { class: "col grow center" },
      h("div", { class: "big-title" }, title),
      h("div", { class: "board" }, ...lines),
      final ? h("p", { class: "hint" }, "The VIP can start another game on their phone.") : null,
    ),
  );
  ticker = null;
}

client.connect().catch((e) => {
  stage.replaceChildren(h("div", { class: "col grow center" }, h("h1", {}, `Cannot show the game: ${e?.message ?? e}`)));
});
