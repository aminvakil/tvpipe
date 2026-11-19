/**
 * drawbluff on a party: the lobby, then per round everyone draws their
 * prompt, and per drawing the others lie, everyone votes, the truth is
 * revealed; scores; the final podium. The host is the only authority:
 * phones and screens only ever get views (docs/party-design.md §5).
 *
 * Views:
 * - `party.room` (every phone): `{ phase, round, rounds, deadline?, drawing?, minPlayers }`
 * - `player.state` (one phone): `{ task, … }`, `task` one of `lobby`,
 *   `draw` (`prompt`, `done`), `lie` (`done`, `lie`), `artist`, `vote`
 *   (`options`, `done`, `choice`), `watch`, `scores`/`final` (`score`, `rank`)
 * - `party.screen` (the TV): `{ phase, … }` per phase, see each setter.
 */

import { EventEmitter } from "node:events";

import { PartyError, type Party, type Player } from "@tvpipe/party";

import { PROMPTS } from "./prompts.ts";
import {
  buildOptions,
  cleanTitle,
  optionsFor,
  POINTS,
  revealOrder,
  score,
  shuffle,
  titleKey,
  validateDrawing,
  type Drawing,
  type Option,
} from "./rules.ts";

export interface Timings {
  draw: number;
  lie: number;
  vote: number;
  /** Between two reveal steps. */
  revealStep: number;
  /** After the truth. */
  revealEnd: number;
  scores: number;
}

export const DEFAULT_TIMINGS: Timings = {
  draw: 90_000,
  lie: 45_000,
  vote: 25_000,
  revealStep: 2600,
  revealEnd: 4500,
  scores: 9000,
};

export interface GameOptions {
  rounds?: number | undefined;
  minPlayers?: number | undefined;
  timings?: Partial<Timings> | undefined;
  rng?: (() => number) | undefined;
  prompts?: readonly string[] | undefined;
}

export interface ScoreLine {
  id: string;
  name: string;
  score: number;
  delta: number;
}

export interface GameEvents {
  phase: [phase: string];
  end: [scores: ScoreLine[]];
}

export type Phase = "lobby" | "draw" | "lie" | "vote" | "reveal" | "scores" | "final";

export class Drawbluff extends EventEmitter<GameEvents> {
  readonly party: Party;
  readonly rounds: number;
  readonly minPlayers: number;
  readonly timings: Timings;
  readonly #rng: () => number;
  readonly #prompts: readonly string[];
  /** Points per player id. */
  readonly scores = new Map<string, number>();
  #phase: Phase = "lobby";
  #running = false;
  #abort = new AbortController();
  #deck: string[] = [];

  constructor(party: Party, options: GameOptions = {}) {
    super();
    this.party = party;
    this.rounds = options.rounds ?? 2;
    this.minPlayers = options.minPlayers ?? 3;
    this.timings = { ...DEFAULT_TIMINGS, ...options.timings };
    this.#rng = options.rng ?? Math.random;
    this.#prompts = options.prompts ?? PROMPTS;
  }

  get phase(): Phase {
    return this.#phase;
  }

  get running(): boolean {
    return this.#running;
  }

  /** Shows the lobby and waits for the VIP to start. */
  open(): void {
    const party = this.party;
    party.handle("start", (_d, from) => {
      if (from.role !== "player" || !from.vip) throw new PartyError("not-vip", "Only the VIP can start");
      if (this.#running) throw new PartyError("running", "The game is on");
      const ready = party.connectedPlayers.length;
      if (ready < this.minPlayers) {
        throw new PartyError("too-few", `Wait for ${this.minPlayers - ready} more player${this.minPlayers - ready === 1 ? "" : "s"}`);
      }
      void this.play();
      return null;
    });
    party.handle("kick", (d, from) => {
      if (from.role !== "player" || !from.vip) throw new PartyError("not-vip", "Only the VIP can remove players");
      if (this.#running) throw new PartyError("running", "Not during a game");
      const id = typeof d === "object" && d !== null && "id" in d ? (d as { id: unknown }).id : undefined;
      const p = typeof id === "string" ? party.player(id) : undefined;
      if (!p || p === from) throw new PartyError("no-player", "No such player");
      party.kick(p);
      return null;
    });
    // Late joiners in the lobby (or after the game) see the lobby.
    party.on("join", (p) => {
      if (!this.#running) p.state.replace(this.#lobbyState());
    });
    this.#lobby();
  }

  /** Ends the game and its timers (the party stays). */
  close(): void {
    this.#abort.abort();
  }

  #setPhase(phase: Phase): void {
    this.#phase = phase;
    this.emit("phase", phase);
  }

  #lobbyState(): Record<string, unknown> {
    return { task: this.#phase === "final" ? "final" : "lobby", ...this.#rank(undefined) };
  }

  #lobby(): void {
    const p = this.party;
    this.#setPhase("lobby");
    p.room.replace({ phase: "lobby", minPlayers: this.minPlayers, maxPlayers: p.maxPlayers, rounds: this.rounds });
    p.screen.replace({ phase: "lobby", minPlayers: this.minPlayers, maxPlayers: p.maxPlayers });
    for (const pl of p.players) pl.state.replace({ task: "lobby" });
  }

  /** Plays a whole game with the players who are there. */
  async play(): Promise<ScoreLine[]> {
    const party = this.party;
    this.#running = true;
    party.lock();
    this.scores.clear();
    for (const p of party.players) this.scores.set(p.id, 0);
    this.#deck = shuffle(this.#prompts, this.#rng);
    try {
      for (let round = 1; round <= this.rounds; round++) {
        const before = new Map(this.scores);
        const players = party.players.filter((p) => p.connected || this.scores.has(p.id));
        const drawings = await this.#draw(round, players);
        for (const { artist, prompt, drawing } of shuffle(drawings, this.#rng)) {
          if (this.#abort.signal.aborted) break;
          if (!party.players.includes(artist)) continue; // left the game
          await this.#turn(round, artist, prompt, drawing);
        }
        if (this.#abort.signal.aborted) break;
        await this.#scores(round, before);
      }
      return this.#final();
    } finally {
      this.#running = false;
      party.unlock();
    }
  }

  // -------------------------------------------------------------------------

  #deadline(ms: number): number {
    return this.party.now() + ms;
  }

  async #draw(round: number, players: Player[]): Promise<{ artist: Player; prompt: string; drawing: Drawing }[]> {
    const party = this.party;
    this.#setPhase("draw");
    const prompts = new Map(players.map((p) => [p, this.#deck.pop() ?? "anything you like"]));
    const deadline = this.#deadline(this.timings.draw);
    party.room.replace({ phase: "draw", round, rounds: this.rounds, deadline });
    party.screen.replace({ phase: "draw", round, rounds: this.rounds, deadline, waiting: players.map((p) => p.id), done: [] });
    for (const [p, prompt] of prompts) p.state.replace({ task: "draw", prompt, done: false });
    for (const p of party.players) if (!prompts.has(p)) p.state.replace({ task: "watch" });
    const got = await party.collect<Drawing>("draw", {
      from: players,
      until: deadline,
      validate: validateDrawing,
      signal: this.#abort.signal,
      onAnswer: (p, _v, answers) => {
        p.state.set({ done: true });
        party.screen.set({ done: [...answers.keys()].map((x) => x.id) });
      },
    });
    return [...got].map(([artist, drawing]) => ({ artist, prompt: prompts.get(artist)!, drawing }));
  }

  async #turn(round: number, artist: Player, prompt: string, drawing: Drawing): Promise<void> {
    const party = this.party;
    const others = party.players.filter((p) => p !== artist && this.scores.has(p.id));
    const art = { id: artist.id, name: artist.name };

    // Lies.
    this.#setPhase("lie");
    let deadline = this.#deadline(this.timings.lie);
    party.room.replace({ phase: "lie", round, rounds: this.rounds, deadline, drawing, artist: art });
    party.screen.replace({ phase: "lie", round, deadline, drawing, artist: art, waiting: others.map((p) => p.id), done: [] });
    artist.state.replace({ task: "artist" });
    for (const p of others) p.state.replace({ task: "lie", done: false });
    const spares = () => this.#deck.filter((t) => titleKey(t) !== titleKey(prompt));
    const lies = await party.collect<string>("lie", {
      from: others,
      until: deadline,
      signal: this.#abort.signal,
      validate: (d, p) => {
        const auto = typeof d === "object" && d !== null && (d as { auto?: unknown }).auto === true;
        if (auto) {
          // "Lie for me": a prompt nobody drew.
          const pool = spares();
          const pick = pool[Math.floor(this.#rng() * pool.length)];
          if (!pick) throw new PartyError("no-lies", "Out of ideas: write one");
          this.#deck.splice(this.#deck.indexOf(pick), 1);
          return pick;
        }
        const text = cleanTitle(typeof d === "object" && d !== null ? (d as { text?: unknown }).text : d);
        if (!text) throw new PartyError("empty", "Write a title");
        if (titleKey(text) === titleKey(prompt)) throw new PartyError("truth", "That's the real title! Write a lie instead.");
        if (!titleKey(text)) throw new PartyError("empty", "Write a title with letters");
        const ok = party.moderate(text, "lie", p);
        if (ok === false) throw new PartyError("refused", "Try another one");
        return cleanTitle(ok);
      },
      reply: (text) => ({ text }),
      onAnswer: (p, text, answers) => {
        p.state.set({ done: true, lie: text });
        party.screen.set({ done: [...answers.keys()].map((x) => x.id) });
      },
    });
    const options = buildOptions(
      prompt,
      new Map([...lies].map(([p, t]) => [p.id, t])),
      shuffle(spares(), this.#rng),
      this.#rng,
      Math.min(4, others.length + 1),
    );

    // Votes.
    this.#setPhase("vote");
    deadline = this.#deadline(this.timings.vote);
    const voters = others.filter((p) => party.players.includes(p));
    party.room.replace({ phase: "vote", round, rounds: this.rounds, deadline, drawing, artist: art });
    party.screen.replace({
      phase: "vote",
      round,
      deadline,
      drawing,
      artist: art,
      options: options.map((o) => ({ id: o.id, text: o.text })),
      waiting: voters.map((p) => p.id),
      done: [],
    });
    for (const p of voters) p.state.replace({ task: "vote", options: optionsFor(p.id, options), done: false });
    const votes = await party.collect<string>("vote", {
      from: voters,
      until: deadline,
      signal: this.#abort.signal,
      validate: (d, p) => {
        const id = typeof d === "object" && d !== null ? (d as { option?: unknown }).option : d;
        if (typeof id !== "string" || !optionsFor(p.id, options).some((o) => o.id === id)) {
          throw new PartyError("bad-vote", "Pick one of the titles");
        }
        return id;
      },
      onAnswer: (p, id, answers) => {
        p.state.set({ done: true, choice: id });
        party.screen.set({ done: [...answers.keys()].map((x) => x.id) });
      },
    });

    // Points and the reveal.
    const scoring = score(
      artist.id,
      options,
      new Map([...votes].map(([p, o]) => [p.id, o])),
      round,
    );
    for (const [id, n] of scoring.points) this.scores.set(id, (this.scores.get(id) ?? 0) + n);
    await this.#reveal(round, artist, prompt, drawing, options, scoring.voters);
  }

  async #reveal(
    round: number,
    artist: Player,
    prompt: string,
    drawing: Drawing,
    options: Option[],
    voters: ReadonlyMap<string, string[]>,
  ): Promise<void> {
    const party = this.party;
    this.#setPhase("reveal");
    const name = (id: string) => party.player(id)?.name ?? "someone";
    const items = revealOrder(options, voters).map((o) => {
      const who = voters.get(o.id) ?? [];
      return {
        id: o.id,
        text: o.text,
        truth: o.truth,
        decoy: o.decoy,
        authors: o.authors.map(name),
        voters: who.map(name),
        points: o.truth ? POINTS.guess * round : POINTS.fooled * round * who.length,
        artistPoints: o.truth ? POINTS.artist * round * who.length : 0,
      };
    });
    const art = { id: artist.id, name: artist.name };
    party.room.replace({ phase: "reveal", round, rounds: this.rounds, drawing, artist: art });
    party.screen.replace({ phase: "reveal", round, drawing, artist: art, prompt, items, step: 0 });
    for (const p of party.players) p.state.replace({ task: "watch", ...this.#rank(p) });
    for (let step = 1; step <= items.length; step++) {
      await this.#sleep(this.timings.revealStep);
      party.screen.set({ step });
    }
    await this.#sleep(this.timings.revealEnd);
  }

  #table(before?: ReadonlyMap<string, number>): ScoreLine[] {
    return this.party.players
      .filter((p) => this.scores.has(p.id))
      .map((p) => {
        const s = this.scores.get(p.id) ?? 0;
        return { id: p.id, name: p.name, score: s, delta: s - (before?.get(p.id) ?? 0) };
      })
      .sort((a, b) => b.score - a.score || a.name.localeCompare(b.name));
  }

  #rank(p: Player | undefined): { score: number; rank: number } {
    const table = this.#table();
    const s = p ? (this.scores.get(p.id) ?? 0) : 0;
    return { score: s, rank: 1 + table.filter((l) => l.score > s).length };
  }

  async #scores(round: number, before: ReadonlyMap<string, number>): Promise<void> {
    const party = this.party;
    this.#setPhase("scores");
    const table = this.#table(before);
    party.room.replace({ phase: "scores", round, rounds: this.rounds });
    party.screen.replace({ phase: "scores", round, rounds: this.rounds, scores: table, last: round === this.rounds });
    for (const p of party.players) p.state.replace({ task: "scores", ...this.#rank(p) });
    await this.#sleep(this.timings.scores);
  }

  #final(): ScoreLine[] {
    const party = this.party;
    this.#setPhase("final");
    const table = this.#table();
    party.room.replace({ phase: "final", minPlayers: this.minPlayers });
    party.screen.replace({ phase: "final", scores: table });
    for (const p of party.players) p.state.replace({ task: "final", ...this.#rank(p) });
    this.emit("end", table);
    return table;
  }

  #sleep(ms: number): Promise<void> {
    const signal = this.#abort.signal;
    return new Promise((resolve) => {
      if (signal.aborted) return resolve();
      const t = setTimeout(resolve, ms);
      signal.addEventListener(
        "abort",
        () => {
          clearTimeout(t);
          resolve();
        },
        { once: true },
      );
    });
  }
}
