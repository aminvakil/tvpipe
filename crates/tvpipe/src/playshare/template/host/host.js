// The host page: the game's rules. It runs in tvpipe's window and keeps
// the party (players, views, actions) with @tvpipe/party, which tvpipe
// serves at /_tvpipe/lib/. docs/games.md explains the pieces.
import { Tvpipe, PartyError } from "/_tvpipe/lib/index.js";
import { joinText, qrSvg } from "/_tvpipe/lib/client.js";

const $ = (id) => document.getElementById(id);

let tvpipe;
try {
  tvpipe = await Tvpipe.connect(); // tvpipe's API (the window's session)
} catch (err) {
  $("error").hidden = false;
  $("error").textContent = `${err.message}. Start the game with tvpipe game play.`;
  throw err;
}
const party = await tvpipe.party(); // the room, its join link and QR code

// The game: every round, the first phone to buzz scores a point.
const points = new Map(); // player id → points
let round = 0;
let buzzed = []; // names, in order

function publish() {
  const scores = party.players.map((p) => ({ name: p.name, points: points.get(p.id) ?? 0, connected: p.connected }));
  const state = { round, open: round > 0, buzzed, scores };
  party.room.set(state); // what every phone sees (view.room)
  party.screen.set(state); // what the TV sees (view.screen)
  render(scores);
}

function render(scores) {
  $("round").textContent = String(round);
  $("players").replaceChildren(
    ...scores.map((s) => {
      const li = document.createElement("li");
      li.textContent = `${s.name}: ${s.points}${s.connected ? "" : " (away)"}`;
      return li;
    }),
  );
}

party.handle("buzz", (_data, from) => {
  if (from.role !== "player") throw new PartyError("not-player", "Only players buzz");
  if (round === 0) throw new PartyError("not-open", "Wait for the round");
  if (!buzzed.includes(from.name)) {
    buzzed = [...buzzed, from.name];
    if (buzzed.length === 1) points.set(from.id, (points.get(from.id) ?? 0) + 1);
    publish();
  }
  return { place: buzzed.indexOf(from.name) + 1 };
});
for (const e of ["join", "leave", "away", "reconnect"]) party.on(e, publish);

$("next").onclick = () => {
  round += 1;
  buzzed = [];
  publish();
};
$("quit").onclick = () => void tvpipe.quit(); // ends tvpipe: TVs stop, phones are told
$("cast").onclick = () => void cast();

// The join link and its QR code (again when a tunnel comes up); the text is
// the short link (tvpipe.party/CODE) once tvpipe has one.
function showJoin() {
  $("link").textContent = joinText(party.joinInfo) ?? "this link";
  $("link").href = party.links.join;
  $("qr").innerHTML = qrSvg(party.joinInfo.qr);
}
party.on("screen-view", showJoin);
showJoin();

// The TV page on the TVs picked in the launcher; a preview here.
tvpipe.on("cast", (s) => ($("tv").textContent = `TV: ${s.state}${s.error ? ` (${s.error})` : ""}`));
async function cast() {
  try {
    await party.cast();
  } catch (err) {
    $("tv").textContent = `TV: ${err.message}`;
  }
}
if ((await tvpipe.selection()).length > 0) await cast();
$("preview").src = `${party.links.watch}&mute=1`;
const fit = () => ($("preview").style.transform = `scale(${$("preview").parentElement.clientWidth / 1280})`);
addEventListener("resize", fit);
fit();
publish();
