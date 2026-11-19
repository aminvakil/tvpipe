/**
 * drawbluff on a party, for 3 to 50 players (docs/drawbluff-scale.md):
 * the lobby, then per round everyone draws their prompt, a gallery shows
 * the drawings that do not get the stage, and per spotlight drawing its
 * liars write fake titles, the others vote, the truth is revealed;
 * scores; the final podium. The host is the only authority: phones and
 * screens only ever get views (docs/party-design.md §5).
 *
 * Views:
 * - `party.room` (every phone): `{ phase, round, rounds, players, spot?, spots?, deadline?, drawing?, artist?, minPlayers? }`
 * - `player.state` (one phone): `{ task, score, rank, of, … }`, `task` one of
 *   `lobby`, `wait` (a late joiner), `draw` (`prompt`, `deadline`, `done`),
 *   `lie` (`key`, `drawing`, `artist`, `liars`, `early`, `deadline`, `done`, `lie`),
 *   `artist` (`key`), `vote` (`key`, `options`, `deadline`, `done`, `choice`),
 *   `watch` (`note`, `result`), `scores`/`final`
 * - `party.screen` (the TV): `{ phase, … }` per phase, see each method.
 */

import { EventEmitter } from "node:events";

import { PartyError, type Party, type Player } from "@tvpipe/party";

import { PROMPTS } from "./prompts.ts";
import {
  buildOptions,
  cleanTitle,
  deal,
  PHONE_LIES,
  pickLiars,
  plan,
  POINTS,
  revealOrder,
  rotate,
  score,
  share,
  shuffle,
  titleKey,
  validateDrawing,
  type Drawing,
  type Option,
  type Plan,
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
  /** One page of the gallery. */
  gallery: number;
}

export const DEFAULT_TIMINGS: Timings = {
  draw: 90_000,
  lie: 45_000,
  vote: 25_000,
  revealStep: 2600,
  revealEnd: 4500,
  scores: 9000,
  gallery: 5000,
};

export const FAST_TIMINGS: Timings = {
  draw: 25_000,
  lie: 20_000,
  vote: 15_000,
  revealStep: 1500,
  revealEnd: 2500,
  scores: 4000,
  gallery: 3000,
};

/** Drawings per gallery page on the TV. */
export const GALLERY_PAGE = 8;
/** Lines of the score table on the TV. */
export const BOARD_LINES = 8;
/** Voter names a reveal line lists (more: a count). */
export const REVEAL_NAMES = 6;

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

export type Phase = "lobby" | "draw" | "gallery" | "lie" | "vote" | "reveal" | "scores" | "final";

/** One drawing of the round. */
interface Entry {
  artist: Player;
  prompt: string;
  drawing: Drawing;
}

/** A spotlight's liars at work (maybe while the TV shows something else). */
interface Lies {
  spot: Entry;
  /** `round.spot`, the phones' key for the task. */
  key: string;
  liars: Player[];
  deadline: number;
  answers: Map<Player, string>;
  finished: boolean;
  done: Promise<Map<Player, string>>;
  abort: AbortController;
}

/** What one player did this round (the highlights). */
interface Tally {
  fooled: number;
  found: number;
  votes: number;
}

export class Drawbluff extends EventEmitter<GameEvents> {
  readonly party: Party;
  readonly rounds: number;
  readonly minPlayers: number;
  readonly timings: Timings;
  readonly #rng: () => number;
  readonly #prompts: readonly string[];
  /** Points per player id (players in the game; late joiners from their first round). */
  readonly scores = new Map<string, number>();
  /** Lies written so far this game, per player id (rotation). */
  readonly #lied = new Map<string, number>();
  /** Spotlights so far this game, per player id (rotation). */
  readonly #spotted = new Map<string, number>();
  #tally = new Map<string, Tally>();
  #phase: Phase = "lobby";
  #running = false;
  #abort = new AbortController();
  #deck: string[] = [];
  #round = 0;
  /** The lies whose progress the TV shows (`liars`/`done` on the screen). */
  #onScreen: Lies | undefined;

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
    // Joiners in the lobby (or after the game) see the lobby; during a
    // game they wait for the next round.
    party.on("join", (p) => {
      if (!this.#running) p.state.replace(this.#lobbyState(p));
      else p.state.replace({ task: "wait", ...this.#rank(p) });
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

  #lobbyState(p: Player): Record<string, unknown> {
    return { task: this.#phase === "final" ? "final" : "lobby", ...this.#rank(this.scores.has(p.id) ? p : undefined) };
  }

  #lobby(): void {
    const p = this.party;
    this.#setPhase("lobby");
    p.room.replace({ phase: "lobby", minPlayers: this.minPlayers, maxPlayers: p.maxPlayers, rounds: this.rounds });
    p.screen.replace({ phase: "lobby", minPlayers: this.minPlayers, maxPlayers: p.maxPlayers });
    for (const pl of p.players) pl.state.replace({ task: "lobby" });
  }

  /** Plays a whole game with the players who are there (and those who join). */
  async play(): Promise<ScoreLine[]> {
    const party = this.party;
    this.#running = true;
    this.scores.clear();
    this.#lied.clear();
    this.#spotted.clear();
    this.#deck = shuffle(this.#prompts, this.#rng);
    try {
      for (let round = 1; round <= this.rounds; round++) {
        this.#round = round;
        // Everybody in the room plays this round, late joiners too.
        for (const p of party.players) if (!this.scores.has(p.id)) this.scores.set(p.id, 0);
        const before = new Map(this.scores);
        this.#tally = new Map();
        const players = this.#inGame();
        const entries = await this.#draw(round, players);
        if (this.#abort.signal.aborted) break;
        await this.#spotlights(round, players.length, entries);
        if (this.#abort.signal.aborted) break;
        await this.#scores(round, before);
      }
      return this.#final();
    } finally {
      this.#running = false;
    }
  }

  // -------------------------------------------------------------------------

  /** Players of this game still in the room, in join order. */
  #inGame(): Player[] {
    return this.party.players.filter((p) => this.scores.has(p.id));
  }

  #deadline(ms: number): number {
    return this.party.now() + ms;
  }

  #room(fields: Record<string, unknown>): void {
    this.party.room.replace({ round: this.#round, rounds: this.rounds, players: this.#inGame().length, ...fields });
  }

  async #draw(round: number, players: Player[]): Promise<Entry[]> {
    const party = this.party;
    this.#setPhase("draw");
    const prompts = new Map(players.map((p) => [p, this.#deck.pop() ?? "anything you like"]));
    const deadline = this.#deadline(this.timings.draw);
    this.#room({ phase: "draw", deadline });
    party.screen.replace({ phase: "draw", round, rounds: this.rounds, deadline, waiting: players.map((p) => p.id), done: [] });
    for (const [p, prompt] of prompts) p.state.replace({ task: "draw", prompt, deadline, done: false, ...this.#rank(p) });
    for (const p of party.players) if (!prompts.has(p)) p.state.replace({ task: "wait", ...this.#rank(p) });
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

  /** The round's spotlights (and the gallery of the other drawings). */
  async #spotlights(round: number, n: number, entries: Entry[]): Promise<void> {
    const party = this.party;
    const p = plan(n);
    const live = entries.filter((e) => party.players.includes(e.artist));
    const ids = live.map((e) => e.artist.id);
    const byId = new Map(live.map((e) => [e.artist.id, e]));
    // The fewest spotlights first, then connected artists, then drawings with strokes.
    const key = (id: string) => {
      const e = byId.get(id)!;
      return 4 * (this.#spotted.get(id) ?? 0) + (e.artist.connected ? 0 : 2) + (e.drawing.strokes.length === 0 ? 1 : 0);
    };
    const chosen = rotate(ids, key, p.spotlights, this.#rng);
    const spots = chosen.map((id) => byId.get(id)!);
    for (const s of spots) this.#spotted.set(s.artist.id, (this.#spotted.get(s.artist.id) ?? 0) + 1);
    const gallery = live.filter((e) => !chosen.includes(e.artist.id));
    // The first spotlight's liars start now (during the gallery, if any).
    let next = spots[0] ? this.#startLies(round, 0, spots[0], p, [], undefined, gallery.length > 0) : undefined;
    if (gallery.length > 0) await this.#gallery(round, gallery, next);
    for (let i = 0; i < spots.length; i++) {
      if (this.#abort.signal.aborted) return;
      const spot = spots[i]!;
      if (!party.players.includes(spot.artist)) {
        next?.abort.abort();
        next = undefined;
        continue; // left the game
      }
      const lies = next && next.spot === spot ? next : this.#startLies(round, i, spot, p, [], undefined, false);
      next = undefined;
      await this.#liePhase(round, i, spots.length, lies);
      if (this.#abort.signal.aborted) return;
      const following = spots[i + 1];
      if (p.pipelined && following && party.players.includes(following.artist)) {
        next = this.#startLies(round, i + 1, following, p, lies.liars, spot.artist, true);
      }
      await this.#turn(round, i, spots.length, lies, next);
    }
  }

  /**
   * Gives a spotlight drawing to its liars: `busy` may not lie (the
   * current liars), `first` lies first if they can (the current artist);
   * `early`: the TV shows something else meanwhile.
   */
  #startLies(round: number, i: number, spot: Entry, p: Plan, busy: readonly Player[], first: Player | undefined, early: boolean): Lies {
    const party = this.party;
    const pool = this.#inGame()
      .filter((x) => x !== spot.artist && !busy.includes(x))
      .map((x) => x.id);
    // Away players last.
    const lied = new Map(pool.map((id) => [id, (this.#lied.get(id) ?? 0) + (party.player(id)!.connected ? 0 : 1000)]));
    const firstId = first && first.connected && this.scores.has(first.id) ? first.id : undefined;
    const liars = pickLiars(pool, lied, p.liars, this.#rng, firstId).map((id) => party.player(id)!);
    for (const l of liars) this.#lied.set(l.id, (this.#lied.get(l.id) ?? 0) + 1);
    const key = `${round}.${i + 1}`;
    const deadline = this.#deadline(this.timings.lie);
    const art = { id: spot.artist.id, name: spot.artist.name };
    for (const l of liars) {
      l.state.replace({ task: "lie", key, drawing: spot.drawing, artist: art, liars: liars.length, early, deadline, done: false, ...this.#rank(l) });
    }
    const abort = new AbortController();
    const onAbort = () => abort.abort();
    this.#abort.signal.addEventListener("abort", onAbort, { once: true });
    const answers = new Map<Player, string>();
    const lies: Lies = { spot, key, liars, deadline, answers, finished: false, done: Promise.resolve(answers), abort };
    const spares = () => this.#deck.filter((t) => titleKey(t) !== titleKey(spot.prompt));
    lies.done = party
      .collect<string>("lie", {
        from: liars,
        until: deadline,
        signal: abort.signal,
        validate: (d, pl) => {
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
          if (titleKey(text) === titleKey(spot.prompt)) throw new PartyError("truth", "That's the real title! Write a lie instead.");
          if (!titleKey(text)) throw new PartyError("empty", "Write a title with letters");
          const ok = party.moderate(text, "lie", pl);
          if (ok === false) throw new PartyError("refused", "Try another one");
          return cleanTitle(ok);
        },
        reply: (text) => ({ text }),
        onAnswer: (pl, text, got) => {
          answers.set(pl, text);
          pl.state.set({ done: true, lie: text });
          if (this.#onScreen === lies) party.screen.set({ done: [...got.keys()].map((x) => x.id) });
        },
      })
      .then((got) => {
        lies.finished = true;
        this.#abort.signal.removeEventListener("abort", onAbort);
        return got;
      });
    return lies;
  }

  /** The gallery: the round's other drawings, a page at a time, while the first liars write. */
  async #gallery(round: number, gallery: Entry[], lies: Lies | undefined): Promise<void> {
    const party = this.party;
    this.#setPhase("gallery");
    this.#onScreen = lies;
    const pages = Math.ceil(gallery.length / GALLERY_PAGE);
    const page = (n: number) =>
      gallery.slice(n * GALLERY_PAGE, (n + 1) * GALLERY_PAGE).map((e) => ({ id: e.artist.id, name: e.artist.name, prompt: e.prompt, drawing: e.drawing }));
    const busy = new Set(lies?.liars ?? []);
    this.#room({ phase: "gallery", count: gallery.length });
    party.screen.replace({
      phase: "gallery",
      round,
      rounds: this.rounds,
      page: 1,
      pages,
      count: gallery.length,
      items: page(0),
      liars: lies?.liars.map((l) => l.id) ?? [],
      done: lies ? [...lies.answers.keys()].map((l) => l.id) : [],
      artist: lies ? { id: lies.spot.artist.id, name: lies.spot.artist.name } : null,
    });
    for (const p of party.players) {
      if (busy.has(p)) continue;
      const mine = gallery.findIndex((e) => e.artist === p);
      p.state.replace({ task: this.scores.has(p.id) ? "watch" : "wait", note: "gallery", mine: mine >= 0 ? Math.floor(mine / GALLERY_PAGE) + 1 : null, ...this.#rank(p) });
    }
    for (let n = 1; n <= pages; n++) {
      if (n > 1) party.screen.set({ page: n, items: page(n - 1) });
      await this.#sleep(this.timings.gallery);
      if (this.#abort.signal.aborted) return;
    }
  }

  /** The TV's lie moment: the drawing, while its liars are still writing. */
  async #liePhase(round: number, i: number, spots: number, lies: Lies): Promise<void> {
    if (lies.finished) return;
    const party = this.party;
    const spot = lies.spot;
    this.#setPhase("lie");
    this.#onScreen = lies;
    const art = { id: spot.artist.id, name: spot.artist.name };
    const deadline = lies.deadline;
    this.#room({ phase: "lie", spot: i + 1, spots, deadline, artist: art });
    party.screen.replace({
      phase: "lie",
      round,
      spot: i + 1,
      spots,
      deadline,
      drawing: spot.drawing,
      artist: art,
      waiting: lies.liars.map((p) => p.id),
      done: [...lies.answers.keys()].map((p) => p.id),
    });
    const busy = new Set(lies.liars);
    for (const p of party.players) {
      if (busy.has(p)) continue;
      if (p === spot.artist) p.state.replace({ task: "artist", key: lies.key, ...this.#rank(p) });
      else p.state.replace({ task: this.scores.has(p.id) ? "watch" : "wait", note: "rest", artist: art, ...this.#rank(p) });
    }
    await lies.done;
  }

  /** A spotlight's vote and reveal (`next`: the next drawing's liars, writing meanwhile). */
  async #turn(round: number, i: number, spots: number, lies: Lies, next: Lies | undefined): Promise<void> {
    const party = this.party;
    const spot = lies.spot;
    const artist = spot.artist;
    const art = { id: artist.id, name: artist.name };
    const options = buildOptions(
      spot.prompt,
      new Map([...(await lies.done)].map(([p, t]) => [p.id, t])),
      shuffle(this.#deck.filter((t) => titleKey(t) !== titleKey(spot.prompt)), this.#rng),
      this.#rng,
      Math.min(4, lies.liars.length + 1),
    );
    const writing = new Set(next?.liars ?? []);
    const voters = this.#inGame().filter((p) => p !== artist && !writing.has(p));
    const shown = deal(options, voters.map((p) => p.id), PHONE_LIES, this.#rng);
    const text = new Map(options.map((o) => [o.id, o.text]));
    const mine = (p: Player) => (shown.get(p.id) ?? []).map((id) => ({ id, text: text.get(id)! }));

    // Votes.
    this.#setPhase("vote");
    this.#onScreen = undefined;
    const deadline = this.#deadline(this.timings.vote);
    const key = lies.key;
    this.#room({ phase: "vote", spot: i + 1, spots, deadline, drawing: spot.drawing, artist: art });
    party.screen.replace({
      phase: "vote",
      round,
      spot: i + 1,
      spots,
      deadline,
      drawing: spot.drawing,
      artist: art,
      options: options.map((o) => ({ id: o.id, text: o.text })),
      waiting: voters.map((p) => p.id),
      done: [],
      writing: writing.size,
    });
    for (const p of voters) p.state.replace({ task: "vote", key, options: mine(p), deadline, done: false, ...this.#rank(p) });
    if (!writing.has(artist)) artist.state.replace({ task: "artist", key, ...this.#rank(artist) });
    for (const p of party.players) {
      if (!this.scores.has(p.id) && !writing.has(p)) p.state.replace({ task: "wait", ...this.#rank(p) });
    }
    const votes = await party.collect<string>("vote", {
      from: voters,
      until: deadline,
      signal: this.#abort.signal,
      validate: (d, p) => {
        const id = typeof d === "object" && d !== null ? (d as { option?: unknown }).option : d;
        if (typeof id !== "string" || !(shown.get(p.id) ?? []).includes(id)) throw new PartyError("bad-vote", "Pick one of the titles");
        return id;
      },
      onAnswer: (p, id, answers) => {
        p.state.set({ done: true, choice: id });
        party.screen.set({ done: [...answers.keys()].map((x) => x.id) });
      },
    });

    // Points and the reveal.
    const byVoter = new Map([...votes].map(([p, o]) => [p.id, o]));
    const scoring = score(artist.id, options, shown, byVoter, round);
    for (const [id, n] of scoring.points) this.scores.set(id, (this.scores.get(id) ?? 0) + n);
    for (const [id, o] of byVoter) {
      const opt = options.find((x) => x.id === o);
      if (!opt || !scoring.voters.get(o)!.includes(id)) continue;
      this.#count(id).votes++;
      if (opt.truth) this.#count(id).found++;
      else for (const a of opt.authors) this.#count(a).fooled++;
    }
    await this.#reveal(round, i, spots, spot, options, scoring, byVoter, next);
  }

  #count(id: string): Tally {
    let t = this.#tally.get(id);
    if (!t) this.#tally.set(id, (t = { fooled: 0, found: 0, votes: 0 }));
    return t;
  }

  async #reveal(
    round: number,
    i: number,
    spots: number,
    spot: Entry,
    options: Option[],
    scoring: ReturnType<typeof score>,
    votes: ReadonlyMap<string, string>,
    next: Lies | undefined,
  ): Promise<void> {
    const party = this.party;
    this.#setPhase("reveal");
    const name = (id: string) => party.player(id)?.name ?? "someone";
    const items = revealOrder(options, scoring.voters).map((o) => {
      const who = scoring.voters.get(o.id) ?? [];
      const seen = scoring.seen.get(o.id) ?? 0;
      return {
        id: o.id,
        text: o.text,
        truth: o.truth,
        decoy: o.decoy,
        authors: o.authors.map(name),
        voters: who.slice(0, REVEAL_NAMES).map(name),
        count: who.length,
        seen,
        points: (o.truth ? POINTS.guess : share(who.length, POINTS.fooled, seen)) * round,
        artistPoints: o.truth ? share(who.length, POINTS.artist, seen) * round : 0,
        same: o.authors.length > 1,
      };
    });
    const art = { id: spot.artist.id, name: spot.artist.name };
    this.#room({ phase: "reveal", spot: i + 1, spots, drawing: spot.drawing, artist: art });
    party.screen.replace({ phase: "reveal", round, spot: i + 1, spots, drawing: spot.drawing, artist: art, prompt: spot.prompt, items, step: 0 });
    const writing = new Set(next && !next.finished ? next.liars.filter((p) => !next.answers.has(p)) : []);
    for (const p of party.players) {
      if (writing.has(p)) continue;
      if (!this.scores.has(p.id)) {
        p.state.replace({ task: "wait", ...this.#rank(p) });
        continue;
      }
      p.state.replace({ task: "watch", note: "reveal", result: this.#result(p, spot, options, scoring, votes), ...this.#rank(p) });
    }
    for (let step = 1; step <= items.length; step++) {
      await this.#sleep(this.timings.revealStep);
      party.screen.set({ step });
    }
    await this.#sleep(this.timings.revealEnd);
  }

  /** What one phone says after a vote: what you found, who fooled you, what your lie did. */
  #result(p: Player, spot: Entry, options: Option[], scoring: ReturnType<typeof score>, votes: ReadonlyMap<string, string>): Record<string, unknown> {
    const points = scoring.points.get(p.id) ?? 0;
    if (p === spot.artist) {
      const truth = options.find((o) => o.truth)!;
      return { kind: "artist", points, count: scoring.voters.get(truth.id)?.length ?? 0, seen: scoring.seen.get(truth.id) ?? 0 };
    }
    const lie = options.find((o) => o.authors.includes(p.id));
    const fooled = lie ? (scoring.voters.get(lie.id)?.length ?? 0) : 0;
    const choice = votes.get(p.id);
    const picked = options.find((o) => o.id === choice && (scoring.voters.get(o.id) ?? []).includes(p.id));
    const out: Record<string, unknown> = { kind: picked ? (picked.truth ? "found" : "fooled") : "none", points };
    if (picked && !picked.truth) out["by"] = picked.decoy ? [] : picked.authors.map((a) => this.party.player(a)?.name ?? "someone");
    if (lie) {
      out["lie"] = lie.text;
      out["fooledCount"] = fooled;
    }
    return out;
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

  #rank(p: Player | undefined): { score: number; rank: number; of: number } {
    const scores = [...this.scores].filter(([id]) => this.party.player(id)).map(([, s]) => s);
    const s = p ? (this.scores.get(p.id) ?? 0) : 0;
    return { score: s, rank: 1 + scores.filter((x) => x > s).length, of: scores.length };
  }

  /** The round's highlights: the best liar, the sharpest eye. */
  #highlights(): Record<string, unknown>[] {
    const party = this.party;
    const lines = [...this.#tally]
      .map(([id, t]) => ({ id, name: party.player(id)?.name, ...t }))
      .filter((l): l is { id: string; name: string } & Tally => typeof l.name === "string");
    const best = (key: (l: Tally) => number) =>
      lines.filter((l) => key(l) > 0).sort((a, b) => key(b) - key(a) || a.name.localeCompare(b.name))[0];
    const out: Record<string, unknown>[] = [];
    const liar = best((l) => l.fooled);
    if (liar) out.push({ kind: "liar", id: liar.id, name: liar.name, count: liar.fooled });
    const eye = best((l) => l.found);
    if (eye) out.push({ kind: "eye", id: eye.id, name: eye.name, count: eye.found, of: eye.votes });
    return out;
  }

  async #scores(round: number, before: ReadonlyMap<string, number>): Promise<void> {
    const party = this.party;
    this.#setPhase("scores");
    const table = this.#table(before);
    this.#room({ phase: "scores" });
    party.screen.replace({
      phase: "scores",
      round,
      rounds: this.rounds,
      scores: table.slice(0, BOARD_LINES),
      total: table.length,
      highlights: this.#highlights(),
      last: round === this.rounds,
    });
    for (const p of party.players) p.state.replace({ task: this.scores.has(p.id) ? "scores" : "wait", ...this.#rank(p) });
    await this.#sleep(this.timings.scores);
  }

  #final(): ScoreLine[] {
    const party = this.party;
    this.#setPhase("final");
    const table = this.#table();
    party.room.replace({ phase: "final", minPlayers: this.minPlayers });
    party.screen.replace({ phase: "final", scores: table.slice(0, BOARD_LINES), total: table.length });
    for (const p of party.players) p.state.replace({ task: "final", ...this.#rank(this.scores.has(p.id) ? p : undefined) });
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
