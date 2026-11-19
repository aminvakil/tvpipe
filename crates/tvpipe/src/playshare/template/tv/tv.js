// The TV page: tvpipe renders it in a headless browser and casts it. It
// connects as the screen (its link has #r=…&k=…&role=screen) and draws
// view.screen (what the host page sets) and view.join (the QR code).
import { PartyClient, qrSvg } from "/_tvpipe/lib/client.js";

const $ = (id) => document.getElementById(id);
const client = PartyClient.fromLocation();

client.on("view", (view) => {
  const s = view.screen ?? {};
  $("qr").innerHTML = qrSvg(view.join?.qr);
  $("code").textContent = view.join?.code ?? "";
  $("status").textContent = !s.round ? "Scan to join, then the host starts a round" : `Round ${s.round}: buzz!`;
  $("buzzed").replaceChildren(...(s.buzzed ?? []).map((name) => item(name)));
  $("scores").replaceChildren(...(s.scores ?? []).map((p) => item(`${p.name}: ${p.points}`)));
});
client.on("status", (st) => {
  if (st !== "open") $("status").textContent = st;
});

function item(text) {
  const li = document.createElement("li");
  li.textContent = text; // text from players is always text, never HTML
  return li;
}

await client.connect();
