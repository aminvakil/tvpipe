// The buzzer's phone: join, then press when the button turns red.
import { PartyClient } from "/party/client.js";

const party = PartyClient.fromLocation();
const $ = (id) => document.getElementById(id);

function render(view) {
  const room = view.room ?? {};
  const me = party.you?.id;
  const vip = (view.roster ?? []).find((p) => p.vip)?.id === me;
  const winner = room.winner;
  $("status").textContent =
    room.phase === "wait" ? "Get ready…" :
    room.phase === "go" ? "NOW!" :
    winner ? `${winner.id === me ? "You" : winner.name} won in ${winner.ms} ms` :
    vip ? "Start when everyone is in" : "Waiting for the VIP";
  $("buzz").disabled = room.phase !== "wait" && room.phase !== "go";
  $("buzz").className = room.phase === "go" ? "go" : "";
  $("start").hidden = !vip || room.phase !== "lobby";
  $("players").replaceChildren(
    ...(view.roster ?? []).map((p) => {
      const li = document.createElement("li");
      li.textContent = `${p.name}${p.vip ? " ★" : ""}${p.connected ? "" : " (away)"} — ${room.wins?.[p.id] ?? 0}`;
      return li;
    }),
  );
}

party.on("view", render);
party.on("bye", (reason, message) => {
  $("status").textContent = message || reason;
});

async function enter(join) {
  try {
    await join;
    $("join").hidden = true;
    $("game").hidden = false;
  } catch (err) {
    alert(err.message);
  }
}

$("join").addEventListener("submit", (e) => {
  e.preventDefault();
  enter(party.join($("name").value));
});
$("buzz").addEventListener("pointerdown", async () => {
  // The view says who won; only refusals (a false start) need showing.
  try {
    await party.act("buzz", { at: Math.round(party.now()) });
  } catch (err) {
    $("status").textContent = err.message;
  }
});
$("start").addEventListener("click", () => party.act("start").catch((err) => alert(err.message)));

if (party.canResume) enter(party.connect());
