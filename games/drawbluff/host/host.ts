/**
 * drawbluff's host page: the game runs here, in tvpipe playshare's
 * window (docs/playshare-design.md). It makes the party on tvpipe's
 * relay, puts the TV page on the TVs the launcher chose, runs the rules
 * (`game/`), and shows the room: the join code and QR code, the TVs,
 * the players (bots to try it alone), the settings and a preview of the
 * TV.
 */

import { qrSvg, Tvpipe, type CastStatus, type HostedParty, type LateJoin } from "@tvpipe/party";

import { bots, type Bot } from "../game/bots.ts";
import { DEFAULT_TIMINGS, Drawbluff, FAST_TIMINGS } from "../game/game.ts";
import { parsePrompts } from "../game/rules.ts";

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

function showError(text: string | null): void {
  const e = $("error");
  e.hidden = !text;
  e.textContent = text ?? "";
}

let party: HostedParty;
let game: Drawbluff;
let tvpipe: Tvpipe;
const robots: Bot[] = [];

function renderJoin(): void {
  const j = party.joinInfo;
  $("code").textContent = party.code;
  $("link").textContent = j.url.replace(/#.*$/, "");
  $("qr").innerHTML = qrSvg(j.qr, { quiet: 2 });
  const t = party.info.tunnel;
  $("link-note").textContent =
    j.mode === "tunnel"
      ? ""
      : j.mode === "lan"
        ? "On this Wi-Fi only"
        : t.state === "starting"
          ? "Opening a public link…"
          : "This computer only (no public link)";
}

function renderPlayers(): void {
  const list = $("players");
  const running = game.running;
  list.replaceChildren(
    ...party.players.map((p) => {
      const li = document.createElement("li");
      if (!p.connected) li.className = "away";
      const who = document.createElement("span");
      who.className = "who";
      who.textContent = p.name;
      const tag = document.createElement("span");
      tag.className = "tag";
      tag.textContent = [p.vip ? "VIP" : "", p.waiting ? "waiting" : "", p.connected ? "" : "away"].filter(Boolean).join(" · ");
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
  ($("start") as HTMLButtonElement).disabled = running || party.connectedPlayers.length < game.minPlayers;
  $("start").textContent = running ? "Playing…" : party.connectedPlayers.length < game.minPlayers ? `Waiting for ${game.minPlayers - party.connectedPlayers.length} more` : "Start the game";
  for (const id of ["rounds", "timers", "late"]) ($(id) as HTMLSelectElement).disabled = running;
}

function renderCast(s: CastStatus): void {
  const list = $("tvs");
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
    li.textContent = s.state === "starting" ? `Starting${s.to ? `: ${s.to.join(", ")}` : ""}…` : s.error ? `The TV failed: ${s.error}` : `The TV: ${s.state}`;
    rows.push(li);
  }
  list.replaceChildren(...rows);
  $("cast-again").hidden = !(s.state === "failed" || s.state === "stopped");
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

function fitPreview(): void {
  const frame = $<HTMLIFrameElement>("preview");
  const box = frame.parentElement!;
  frame.style.transform = `scale(${box.clientWidth / 1280})`;
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
      think: () => 800 + Math.random() * 4000,
    });
    robots.push(...added);
  } catch (err) {
    showError(`A bot could not join: ${(err as Error).message}`);
  }
}

async function main(): Promise<void> {
  try {
    tvpipe = await Tvpipe.connect();
  } catch (err) {
    showError(`${(err as Error).message}. Start the game with tvpipe playshare.`);
    return;
  }
  const prompts = parsePrompts(await (await fetch(new URL("../data/prompts.txt", import.meta.url))).text());
  party = await tvpipe.party({ lateJoin: "wait" });
  game = new Drawbluff(party, { prompts, rounds: 2 });
  game.open();
  renderJoin();
  renderPlayers();
  party.on("screen-view", () => {
    renderJoin();
    renderPlayers();
  });
  game.on("phase", (p) => {
    $("phase").textContent = p === "lobby" ? "lobby" : p === "final" ? "game over" : `round ${String(party.room.peek()["round"] ?? "")} · ${p}`;
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

  ($("rounds") as HTMLSelectElement).onchange = (e) => game.configure({ rounds: Number((e.target as HTMLSelectElement).value) });
  ($("timers") as HTMLSelectElement).onchange = (e) =>
    game.configure({ timings: (e.target as HTMLSelectElement).value === "fast" ? FAST_TIMINGS : DEFAULT_TIMINGS });
  ($("late") as HTMLSelectElement).onchange = (e) => {
    party.lateJoin = (e.target as HTMLSelectElement).value as LateJoin;
    party.room.set({ lateJoin: party.lateJoin });
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

  const chosen = await tvpipe.selection();
  if (chosen.length > 0) await cast();
  // For checks and tests (DevTools): the party and the game.
  Object.assign(globalThis, { drawbluff: { party, game, tvpipe, addBots } });
}

void main();
