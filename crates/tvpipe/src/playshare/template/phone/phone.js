// The phone page: phones open the join link (?r=CODE#k=KEY) through the
// tunnel or the Wi-Fi. The client joins with a name, follows the view
// ({ roster, room, me }) and sends actions to the host page.
import { PartyClient } from "/_tvpipe/lib/client.js";

const $ = (id) => document.getElementById(id);
const party = PartyClient.fromLocation();
let round = 0;

party.on("view", (view) => {
  const room = view.room ?? {};
  $("join").hidden = true;
  $("game").hidden = false;
  $("hello").textContent = `Hi ${party.you?.name ?? ""}!`;
  if (room.round !== round) {
    round = room.round;
    $("result").textContent = "";
  }
  $("buzz").disabled = !room.open;
  if (!room.open) $("result").textContent = "Wait for the host to start a round.";
});
party.on("status", (s) => ($("status").textContent = s === "open" ? "" : s));
party.on("bye", (reason, message) => ($("status").textContent = message ?? `The game ended (${reason}).`));

$("buzz").onclick = async () => {
  try {
    const { place } = await party.act("buzz");
    $("result").textContent = place === 1 ? "First!" : `#${place}`;
  } catch (err) {
    $("result").textContent = err.message;
  }
};

$("join").onsubmit = async (e) => {
  e.preventDefault();
  try {
    await party.join($("name").value);
  } catch (err) {
    $("status").textContent = err.message;
  }
};

if (party.canResume) await party.connect(); // this phone played before
else $("join").hidden = false;
