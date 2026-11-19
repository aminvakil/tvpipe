/**
 * drawbluff: a Drawful-style party game on tvpipe.
 *
 *   node main.ts --to "Living Room"            the TV shows a web page (tvpipe's own browser)
 *   node main.ts --to "Living Room" --video    the TV shows video frames drawn here
 *   node main.ts --to browser --open           a browser tab instead of a TV
 *   node main.ts                               no TV: open the printed screen link yourself (HDMI)
 *
 * Options: --tunnel=cloudflare|opentunnel|auto (default auto), --no-tunnel
 * (phones on this computer only), --bots N (fake players), --rounds N,
 * --fast (short timers), --size WxH (video, default 1280x720), -v.
 *
 * Run `npm run build` in npm/tvpipe and npm/party first (this imports the
 * packages as an application would).
 */

import { parseArgs } from "node:util";

import { createParty, type TunnelChoice } from "@tvpipe/party";

import { bots } from "./game/bots.ts";
import { Drawbluff } from "./game/game.ts";
import { loadCanvas, renderScreen } from "./screen-video.ts";

const { values: args } = parseArgs({
  options: {
    to: { type: "string", multiple: true },
    video: { type: "boolean", default: false },
    tunnel: { type: "string" },
    "no-tunnel": { type: "boolean", default: false },
    bots: { type: "string", default: "0" },
    rounds: { type: "string", default: "2" },
    fast: { type: "boolean", default: false },
    size: { type: "string", default: "1280x720" },
    open: { type: "boolean", default: false },
    verbose: { type: "boolean", short: "v", default: false },
    help: { type: "boolean", short: "h", default: false },
  },
});

if (args.help) {
  console.log(
    "usage: node main.ts [--to NAME]... [--video] [--tunnel=cloudflare|opentunnel|auto] [--no-tunnel] [--bots N] [--rounds N] [--fast] [--size WxH] [--open] [-v]",
  );
  process.exit(0);
}

const tunnel: TunnelChoice = args["no-tunnel"] ? false : ((args.tunnel as TunnelChoice | undefined) ?? "auto");
const here = new URL(".", import.meta.url);

const party = await createParty({
  phone: new URL("web/phone/", here),
  screen: new URL("web/screen/", here),
  tunnel,
  maxPlayers: 8,
  logLevel: args.verbose ? "info" : "warn",
});

const game = new Drawbluff(party, {
  rounds: Number(args.rounds),
  timings: args.fast ? { draw: 25_000, lie: 20_000, vote: 15_000, revealStep: 1500, revealEnd: 2500, scores: 4000 } : undefined,
});
game.open();
game.on("phase", (p) => args.verbose && console.error(`drawbluff: ${p}`));

console.log(`\n  drawbluff, room ${party.code}\n`);
console.log(`  phones join at  ${party.links.join}`);
if (party.links.watch) console.log(`  watch from afar ${party.links.watch}`);
console.log(`  the screen      ${party.links.screen}\n`);
const qr = party.joinInfo.qr;
if (qr && process.stdout.isTTY) console.log(terminalQr(qr.rows));

const to = args.to ?? [];
let tv: Awaited<ReturnType<typeof party.toTv>> | undefined;
if (to.length > 0) {
  const sdk = to.includes("browser") ? { player: { open: args.open } } : {};
  if (args.video) {
    const [w, h] = args.size.split("x").map(Number) as [number, number];
    const { createCanvas } = await loadCanvas();
    const canvas = createCanvas(w, h);
    tv = await party.toTv({
      mode: "video",
      to,
      canvas,
      render: (ctx, view, t) => renderScreen(ctx, view, t, party.now()),
      sdk,
    });
  } else {
    tv = await party.toTv({ mode: "web", to, sdk });
  }
  const url = (tv.session as { playerUrl?: string }).playerUrl;
  console.log(`  on the TV (${args.video ? "video" : "web page"})${url ? `: ${url}` : ""}`);
}

const n = Number(args.bots);
if (n > 0) {
  const url = party.links.localJoin.replace(/^http/, "ws").replace(/\/\?.*$/, "/party/ws");
  await bots(n, { url, room: party.code, key: party.joinKey, autoStart: true, think: () => 500 + Math.random() * 3000 });
  console.log(`  ${n} bot${n === 1 ? "" : "s"} joined`);
}

let stopping = false;
const stop = async () => {
  if (stopping) process.exit(130);
  stopping = true;
  game.close();
  await party.shutdown();
  process.exit(0);
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);

/** A QR code in the terminal: two module rows per line, half blocks, black on white. */
function terminalQr(rows: string[]): string {
  const quiet = 2;
  const size = rows.length + 2 * quiet;
  const dark = (x: number, y: number) => rows[y - quiet]?.[x - quiet] === "1";
  let out = "";
  for (let y = 0; y < size; y += 2) {
    out += "  \x1b[30;47m";
    for (let x = 0; x < size; x++) {
      const top = dark(x, y);
      const bottom = dark(x, y + 1);
      out += top && bottom ? "█" : top ? "▀" : bottom ? "▄" : " ";
    }
    out += "\x1b[0m\n";
  }
  return out;
}
