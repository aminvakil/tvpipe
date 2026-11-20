/**
 * Jinx's host page: the game runs here, in tvpipe's window
 * (docs/playshare-design.md). It makes the party on tvpipe's relay, puts
 * the TV page on the TVs the launcher chose, runs the rules (`game/`), and
 * shows the room: the join code and QR code, the TVs, the players (bots
 * to try it alone), the settings (rounds, writing time, words) and a
 * muted preview of the TV. The room's dealt words are kept in
 * `localStorage`, so no word comes twice until all were dealt.
 */

import { qrSvg, Tvpipe, type CastStatus, type HostedParty } from "@tvpipe/party";

import { bots, type Bot } from "../game/bots.ts";
import { Jinx, type Phase } from "../game/game.ts";
import { dictionary, parseWordList, type WordEntry } from "../game/match.ts";
import type { Difficulty, RoundsSetting } from "../game/rules.ts";

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

function showError(text: string | null): void {
  const e = $("error");
  e.hidden = !text;
  e.textContent = text ?? "";
}

let party: HostedParty;
let game: Jinx;
let tvpipe: Tvpipe;
let words: WordEntry[] = [];
let dictWords: string[] = [];
const robots: Bot[] = [];

const DEALT_KEY = "jinx:dealt";
const SETTINGS_KEY = "jinx:settings";

const PHASES: Record<Phase, string> = {
  lobby: "lobby",
  hotseat: "hot seat",
  write: "writing",
  jinx: "jinx check",
  reveal: "reveal",
  verdict: "verdict",
  replay: "replay",
  scores: "scores",
  end: "game over",
};

function loadJson<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v ? (JSON.parse(v) as T) : fallback;
  } catch {
    return fallback;
  }
}

function renderJoin(): void {
  const j = party.joinInfo;
  $("code").textContent = party.code;
  $("link").textContent = j.url.replace(/#.*$/, "");
  $("qr").innerHTML = qrSvg(j.qr, { quiet: 2 });
  const tunnel = party.info.tunnel;
  $("link-note").textContent =
    j.mode === "tunnel" ? "" : j.mode === "lan" ? "On this Wi-Fi only" : tunnel.state === "starting" ? "Opening a public link…" : "This computer only (no public link)";
}

function renderPlayers(): void {
  const list = $("players");
  const running = game.running;
  const botIds = new Set(robots.map((b) => String(b.client.you?.["id"] ?? "")));
  list.replaceChildren(
    ...party.players.map((p) => {
      const li = document.createElement("li");
      if (!p.connected) li.className = "away";
      const who = document.createElement("span");
      who.className = "who";
      who.textContent = p.name;
      const tag = document.createElement("span");
      tag.className = "tag";
      tag.textContent = [botIds.has(p.id) ? "bot" : "", p.vip ? "VIP" : "", p.waiting ? "waiting" : "", p.connected ? "" : "away"].filter(Boolean).join(" · ");
      const end = document.createElement("span");
      end.className = "end";
      end.append(tag);
      if (!running) {
        const kick = document.createElement("button");
        kick.className = "kick";
        kick.title = `Remove ${p.name}`;
        kick.textContent = "✕";
        kick.onclick = () => party.kick(p);
        end.append(kick);
      }
      li.append(who, end);
      return li;
    }),
  );
  $("count").textContent = `${party.players.length} / ${party.maxPlayers}`;
  const ready = party.connectedPlayers.length;
  const start = $<HTMLButtonElement>("start");
  start.disabled = running || ready < game.minPlayers;
  start.textContent = running
    ? "Playing…"
    : ready < game.minPlayers
      ? `Waiting for ${game.minPlayers - ready} more`
      : game.phase === "end"
        ? "Play again"
        : "Start the game";
  for (const id of ["rounds", "write", "difficulty"]) $<HTMLSelectElement>(id).disabled = running;
  $<HTMLButtonElement>("bot1").disabled = party.players.length >= party.maxPlayers;
  $<HTMLButtonElement>("bot3").disabled = party.players.length + 3 > party.maxPlayers;
}

function renderCast(s: CastStatus): void {
  const list = $("tvs");
  const none = s.state === "stopped" && !s.error && !s.receivers?.length;
  const rows = (s.receivers ?? []).map((r) => {
    const li = document.createElement("li");
    const name = document.createElement("span");
    name.textContent = r.name === "browser" ? "A browser tab" : r.name;
    const tag = document.createElement("span");
    const ok = r.state === "connected" || r.state === "reconnected";
    tag.className = `tag ${ok ? "ok" : r.state === "failed" || r.state === "gave-up" ? "bad" : ""}`;
    tag.textContent = ok ? (s.state === "streaming" ? "showing" : "connected") : r.error ? `${r.state}: ${r.error}` : r.state;
    li.append(name, tag);
    return li;
  });
  if (rows.length === 0) {
    const li = document.createElement("li");
    li.className = "muted";
    li.textContent = none
      ? "No TV: open the preview on a screen yourself."
      : s.state === "starting"
        ? `Starting${s.to ? `: ${s.to.join(", ")}` : ""}…`
        : s.error
          ? `The TV failed: ${s.error}`
          : `The TV: ${s.state}`;
    rows.push(li);
  }
  list.replaceChildren(...rows);
  $("cast-again").hidden = none || !(s.state === "failed" || s.state === "stopped");
  const player = $<HTMLAnchorElement>("player");
  player.hidden = !s.player;
  if (s.player) player.href = s.player;
}

async function cast(): Promise<void> {
  showError(null);
  try {
    renderCast(await party.cast());
  } catch (err) {
    renderCast({ state: "failed", error: (err as Error).message });
  }
}

/** tvpipe's cast picker: other TVs; none stops casting. */
async function changeTvs(): Promise<void> {
  showError(null);
  let sel;
  try {
    sel = await tvpipe.showCastPicker();
  } catch (err) {
    showError((err as Error).message);
    return;
  }
  if (!sel) return;
  if (sel.devices.length > 0) {
    await cast();
    return;
  }
  await party.stopCast().catch(() => {});
  renderCast({ state: "stopped" });
}

function fitPreview(): void {
  const frame = $<HTMLIFrameElement>("preview");
  frame.style.transform = `scale(${frame.parentElement!.clientWidth / 1280})`;
}

async function addBots(n: number): Promise<void> {
  showError(null);
  const url = `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/_tvpipe/ws`;
  const names = ["Robo", "Pixel", "Gizmo", "Bolt", "Chip", "Sprocket", "Widget", "Servo"];
  try {
    const added = await bots(n, {
      url,
      room: party.code,
      key: party.joinKey,
      names: names.map((x) => `${x}${robots.length + 1}`),
      think: () => 800 + Math.random() * 3000,
      peek: () => game.secret,
      words,
      dict: dictWords,
    });
    for (const b of added) if (b.client.you) game.markBot(String(b.client.you["id"]));
    robots.push(...added);
    renderPlayers();
  } catch (err) {
    showError(`A bot could not join: ${(err as Error).message}`);
  }
}

async function main(): Promise<void> {
  try {
    tvpipe = await Tvpipe.connect();
  } catch (err) {
    showError(`${(err as Error).message}. Start the game with tvpipe game play.`);
    return;
  }
  const [wordsText, dictText] = await Promise.all(
    ["words.txt", "dict.txt"].map(async (f) => {
      const res = await fetch(new URL(`../data/${f}`, import.meta.url));
      if (!res.ok) throw new Error(`data/${f}: ${res.status}`);
      return res.text();
    }),
  );
  words = parseWordList(wordsText!);
  // The bots' random clues and wrong guesses come from the curated word list:
  // the dictionary has names, places and words not for a party's TV.
  dictWords = words.map((w) => w.word).filter((w) => /^[a-z]{4,8}$/.test(w));
  const settings = loadJson<{ rounds?: RoundsSetting; write?: number; difficulty?: Difficulty }>(SETTINGS_KEY, {});
  party = await tvpipe.party({ lateJoin: "wait" });
  game = new Jinx(party, {
    words,
    known: dictionary(dictText!),
    dealt: loadJson<string[]>(DEALT_KEY, []),
    rounds: settings.rounds ?? "auto",
    difficulty: settings.difficulty ?? "normal",
    timings: { write: (settings.write ?? 45) * 1000 },
  });
  game.on("dealt", (used) => {
    try {
      localStorage.setItem(DEALT_KEY, JSON.stringify(used));
    } catch {
      // Full or blocked: the deck still works for this session.
    }
  });
  game.open();
  renderJoin();
  renderPlayers();
  party.on("screen-view", () => {
    renderJoin();
    renderPlayers();
  });
  game.on("phase", (p) => {
    const round = Number(party.room.peek()["round"] ?? 0);
    $("phase").textContent = p === "lobby" || p === "end" ? PHASES[p] : `round ${round} · ${PHASES[p]}`;
    renderPlayers();
  });
  tvpipe.on("cast", renderCast);
  tvpipe.on("tunnel", () => setTimeout(renderJoin, 100));
  tvpipe.on("close", () => showError("tvpipe stopped: the game is over."));
  tvpipe.on("closing", () => {
    for (const b of robots) b.close();
  });

  const frame = $<HTMLIFrameElement>("preview");
  frame.src = `${party.links.watch}&mute=1`;
  fitPreview();
  addEventListener("resize", fitPreview);

  const rounds = $<HTMLSelectElement>("rounds");
  const write = $<HTMLSelectElement>("write");
  const difficulty = $<HTMLSelectElement>("difficulty");
  rounds.value = String(game.rounds);
  write.value = String(Math.round(game.timings.write / 1000));
  difficulty.value = game.difficulty;
  const save = () => {
    const s = { rounds: game.rounds, write: Math.round(game.timings.write / 1000), difficulty: game.difficulty };
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(s));
  };
  rounds.onchange = () => {
    const v = rounds.value;
    game.configure({ rounds: v === "1" ? 1 : v === "2" ? 2 : "auto" });
    save();
  };
  write.onchange = () => {
    game.configure({ write: Number(write.value) * 1000 });
    save();
  };
  difficulty.onchange = () => {
    game.configure({ difficulty: difficulty.value as Difficulty });
    save();
  };
  $("bot1").onclick = () => void addBots(1);
  $("bot3").onclick = () => void addBots(3);
  $("start").onclick = () => {
    if (game.running || party.connectedPlayers.length < game.minPlayers) return;
    void game.play();
    renderPlayers();
  };
  $("quit").onclick = () => void tvpipe.quit();
  $("cast-again").onclick = () => void cast();
  $("change-tv").onclick = () => void changeTvs();

  const chosen = await tvpipe.selection();
  if (chosen.length > 0) await cast();
  // For checks and tests (DevTools): the party and the game.
  Object.assign(globalThis, { jinx: { party, game, tvpipe, addBots } });
}

void main();
