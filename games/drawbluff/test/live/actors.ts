/**
 * Players for the screenshots (`play.ts --shots`): like the bots
 * (`game/bots.ts`), but with people's names, drawings that look drawn and
 * titles a person would write, so the pictures on the site show a room of
 * friends, not a test.
 */

import { memoryStorage, PartyClient } from "@tvpipe/party/client";

/** Human names for the room. */
export const NAMES = ["Mia", "Jonas", "Lena", "Sam", "Noah", "Zoe", "Ben", "Ella", "Theo", "Ivy"];

const TITLES: Record<string, readonly string[]> = {
  en: [
    "a pigeon running for mayor",
    "grandma's secret recipe",
    "the last slice of pizza",
    "a knight afraid of horses",
    "a snail on holiday",
    "my neighbour's cactus",
    "a haunted toaster",
    "the world's tiniest dragon",
    "a very proud potato",
    "monday morning",
    "a cat who knows too much",
    "the lost sock's revenge",
    "a fish learning to ride a bike",
    "an awkward first date",
  ],
  de: [
    "eine Taube als Bürgermeister",
    "Omas Geheimrezept",
    "das letzte Stück Pizza",
    "ein Ritter mit Angst vor Pferden",
    "eine Schnecke im Urlaub",
    "der Kaktus vom Nachbarn",
    "ein Toaster, in dem es spukt",
    "der kleinste Drache der Welt",
    "eine sehr stolze Kartoffel",
    "Montagmorgen",
    "eine Katze, die zu viel weiß",
    "die Rache der verlorenen Socke",
  ],
};

type Pt = [number, number];

/** Pictures on the 1000×1000 grid, pen 0 (dark) and pen 1 (light); a little wobble makes them look drawn. */
function pictures(rng: () => number): number[][][] {
  const wob = (v: number) => Math.round(v + (rng() - 0.5) * 8);
  const stroke = (pen: number, width: number, pts: Pt[]) => [pen, width, ...pts.flatMap(([x, y]) => [wob(x), wob(y)])];
  const ellipse = (pen: number, width: number, cx: number, cy: number, rx: number, ry: number, from = 0, to = 1): number[] => {
    const pts: Pt[] = [];
    for (let i = 0; i <= 32; i++) {
      const a = (from + (to - from) * (i / 32)) * Math.PI * 2;
      pts.push([cx + Math.cos(a) * rx, cy + Math.sin(a) * ry]);
    }
    return stroke(pen, width, pts);
  };
  const dot = (pen: number, x: number, y: number, r = 14) => [pen, r * 2, x, y, x + 1, y + 1];
  const cat = [
    ellipse(0, 14, 500, 430, 200, 175),
    stroke(0, 14, [[340, 330], [330, 160], [440, 270]]),
    stroke(0, 14, [[560, 270], [670, 160], [660, 330]]),
    stroke(1, 10, [[360, 300], [355, 210], [415, 275]]),
    stroke(1, 10, [[585, 275], [645, 210], [640, 300]]),
    dot(0, 430, 400, 18),
    dot(0, 570, 400, 18),
    stroke(1, 12, [[480, 470], [520, 470], [500, 495], [480, 470]]),
    stroke(0, 8, [[500, 495], [500, 520], [470, 540]]),
    stroke(0, 8, [[500, 520], [530, 540]]),
    stroke(0, 6, [[460, 480], [300, 455]]),
    stroke(0, 6, [[460, 500], [300, 515]]),
    stroke(0, 6, [[540, 480], [700, 455]]),
    stroke(0, 6, [[540, 500], [700, 515]]),
    ellipse(0, 14, 500, 790, 190, 150),
    stroke(0, 12, [[430, 930], [430, 880]]),
    stroke(0, 12, [[570, 930], [570, 880]]),
    stroke(1, 14, [[690, 800], [800, 760], [840, 660], [800, 590]]),
  ];
  const rocket = [
    stroke(0, 14, [[500, 120], [600, 260], [610, 640], [390, 640], [400, 260], [500, 120]]),
    ellipse(0, 12, 500, 360, 55, 55),
    ellipse(1, 10, 500, 360, 30, 30),
    stroke(0, 14, [[400, 520], [300, 650], [300, 720], [395, 640]]),
    stroke(0, 14, [[600, 520], [700, 650], [700, 720], [605, 640]]),
    stroke(1, 16, [[430, 660], [450, 760], [480, 700], [500, 830], [520, 700], [550, 760], [570, 660]]),
    dot(1, 220, 200, 10),
    dot(1, 800, 330, 8),
    dot(1, 760, 160, 12),
    dot(1, 180, 520, 8),
    stroke(1, 6, [[820, 560], [860, 560]]),
    stroke(1, 6, [[840, 540], [840, 580]]),
  ];
  const fish = [
    ellipse(0, 14, 460, 480, 250, 160),
    stroke(0, 14, [[700, 470], [860, 360], [850, 600], [700, 490]]),
    dot(0, 330, 440, 20),
    stroke(0, 10, [[250, 520], [290, 530]]),
    stroke(1, 10, [[480, 360], [430, 480], [480, 600]]),
    stroke(1, 10, [[560, 380], [510, 480], [560, 580]]),
    ellipse(1, 8, 200, 300, 30, 30),
    ellipse(1, 8, 160, 200, 20, 20),
    ellipse(1, 8, 220, 120, 14, 14),
    stroke(1, 12, [[80, 800], [180, 760], [280, 800], [380, 760], [480, 800], [580, 760], [680, 800], [780, 760], [880, 800]]),
  ];
  const snowman = [
    ellipse(0, 14, 500, 740, 210, 170),
    ellipse(0, 14, 500, 450, 150, 130),
    ellipse(0, 14, 500, 230, 100, 90),
    stroke(0, 14, [[380, 150], [620, 150]]),
    stroke(0, 14, [[420, 150], [430, 40], [570, 40], [580, 150]]),
    dot(0, 465, 215, 12),
    dot(0, 535, 215, 12),
    stroke(1, 14, [[500, 245], [600, 265], [500, 270]]),
    dot(0, 500, 410, 12),
    dot(0, 500, 470, 12),
    stroke(0, 10, [[360, 430], [220, 330], [190, 340]]),
    stroke(0, 10, [[640, 430], [780, 330], [800, 300]]),
    stroke(1, 18, [[390, 330], [610, 330]]),
    stroke(1, 18, [[580, 330], [600, 420]]),
  ];
  const house = [
    stroke(0, 14, [[180, 520], [180, 860], [620, 860], [620, 520]]),
    stroke(0, 14, [[130, 540], [400, 290], [670, 540]]),
    stroke(0, 12, [[360, 860], [360, 690], [450, 690], [450, 860]]),
    stroke(0, 10, [[230, 600], [310, 600], [310, 670], [230, 670], [230, 600]]),
    stroke(1, 10, [[270, 600], [270, 670]]),
    ellipse(1, 12, 820, 170, 70, 70),
    stroke(1, 8, [[820, 60], [820, 80]]),
    stroke(1, 8, [[710, 170], [730, 170]]),
    stroke(1, 8, [[900, 250], [915, 265]]),
    stroke(0, 14, [[780, 860], [780, 620]]),
    ellipse(1, 14, 780, 540, 100, 110),
    stroke(1, 10, [[60, 880], [940, 880]]),
  ];
  return [cat, rocket, fish, snowman, house];
}

/** A drawing for a player (`pens`: the pens the phone has). */
export function picture(rng: () => number, pens: number, pick?: number): { strokes: number[][] } {
  const all = pictures(rng);
  const strokes = all[(pick ?? Math.floor(rng() * all.length)) % all.length]!;
  return { strokes: strokes.map(([pen, ...rest]) => [Math.min(pen!, Math.max(0, pens - 1)), ...rest]) };
}

/** A player who joins, draws, writes titles and votes by itself. */
export class Actor {
  readonly client: PartyClient;
  readonly name: string;
  readonly #rng = Math.random;
  readonly #handled = new Set<string>();
  readonly #n: number;
  readonly #think: () => number;

  constructor(o: { url: string; room: string; key: string; name: string; n: number; think: () => number }) {
    this.name = o.name;
    this.#n = o.n;
    this.#think = o.think;
    this.client = new PartyClient({ url: o.url, room: o.room, key: o.key, storage: memoryStorage() });
    this.client.on("view", (v) => this.#onView(v));
  }

  #once(key: string, fn: () => Promise<unknown>): void {
    if (this.#handled.has(key)) return;
    this.#handled.add(key);
    setTimeout(() => void fn().catch(() => {}), this.#think());
  }

  #onView(view: Record<string, any>): void {
    const me = view["me"] ?? {};
    const room = view["room"] ?? {};
    const turn = `${room.round}:${me.key ?? me.deadline ?? ""}`;
    const c = this.client;
    if (me.task === "draw" && !me.done) {
      if (!this.#handled.has(`draw:${turn}`)) c.input({ ink: 80 + Math.floor(this.#rng() * 400) });
      const pens = Array.isArray(me.palette) ? me.palette.length : 2;
      this.#once(`draw:${turn}`, () => c.act("draw", picture(this.#rng, pens, 1 + this.#n + Number(room.round ?? 0))));
    } else if (me.task === "lie" && !me.done) {
      const titles = TITLES[room.lang] ?? TITLES["en"]!;
      if (!this.#handled.has(`lie:${turn}`)) c.input({ typing: true });
      // Another title when one is refused (the real one, or one already written).
      const lie = async () => {
        for (let i = 0; ; i++) {
          try {
            return await c.act("lie", { text: titles[Math.floor(this.#rng() * titles.length)]! });
          } catch (err) {
            if (i >= 3) throw err;
          }
        }
      };
      this.#once(`lie:${turn}`, lie);
    } else if (me.task === "vote" && !me.done && Array.isArray(me.options) && me.options.length > 0) {
      const opts = me.options as { id: string }[];
      this.#once(`vote:${turn}`, () => c.act("vote", { option: opts[Math.floor(this.#rng() * opts.length)]!.id }));
    } else if (me.task === "final") {
      this.#handled.clear();
    }
  }

  close(): void {
    this.client.close();
  }
}

/** `n` actors joined to a party, one after the other. */
export async function actors(n: number, o: { url: string; room: string; key: string; think: () => number }): Promise<Actor[]> {
  const out: Actor[] = [];
  for (let i = 0; i < n; i++) {
    const a = new Actor({ ...o, name: NAMES[i % NAMES.length]!, n: i });
    await a.client.join(a.name);
    out.push(a);
  }
  return out;
}
