// The stick's phone: a joystick whose position goes to the host as
// continuous input (`party.input`), over WebRTC when the host offers it.
// `?rtc=0` stays on the WebSocket. `window.measure` is for the live check
// (scripts/live/party-rtc.ts).
import { PartyClient } from "/party/client.js";

const rtc = new URLSearchParams(location.search).get("rtc") !== "0";
const party = PartyClient.fromLocation({ rtc });
window.party = party;
const $ = (id) => document.getElementById(id);

let n = 0;
/** Sends the stick's position, stamped for the host's latency numbers. */
function send(x, y) {
  party.input({ x, y, n: ++n, t: performance.timeOrigin + performance.now() });
}

function status() {
  $("status").textContent = `${party.transport === "rtc" ? "WebRTC" : "WebSocket"} · ${Math.round(party.rtt)} ms`;
}
party.on("transport", status);
party.on("bye", (reason, message) => ($("status").textContent = message || reason));

const pad = $("pad");
let held = false;
function move(ev) {
  const r = pad.getBoundingClientRect();
  const clamp = (v) => Math.max(-1, Math.min(1, v));
  const x = clamp(((ev.clientX - r.left) / r.width) * 2 - 1);
  const y = clamp(((ev.clientY - r.top) / r.height) * 2 - 1);
  $("knob").style.left = `${39 + x * 39}%`;
  $("knob").style.top = `${39 + y * 39}%`;
  send(Math.round(x * 100) / 100, Math.round(y * 100) / 100);
}
pad.addEventListener("pointerdown", (ev) => {
  held = true;
  pad.setPointerCapture(ev.pointerId);
  move(ev);
});
pad.addEventListener("pointermove", (ev) => held && move(ev));
pad.addEventListener("pointerup", () => {
  held = false;
  $("knob").style.left = $("knob").style.top = "39%";
  send(0, 0);
});

$("join").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  try {
    await party.join($("name").value);
    $("join").hidden = true;
    $("game").hidden = false;
    status();
    setInterval(status, 1000);
  } catch (err) {
    alert(err.message);
  }
});
if (party.canResume) party.connect().then(() => {
  $("join").hidden = true;
  $("game").hidden = false;
  status();
}, () => {});

/**
 * The live check's numbers: `acts` echo round trips (ms, sequential,
 * paced under the host's rate limit),
 * then `inputs` stick updates at `hz`; the host's view of them comes from
 * the `stats` action.
 */
window.measure = async (acts, inputs, hz) => {
  const rtts = [];
  for (let i = 0; i < acts; i++) {
    const t0 = performance.now();
    await party.act("echo", i);
    rtts.push(performance.now() - t0);
    // The host allows 20 messages a second per phone.
    await new Promise((r) => setTimeout(r, 60));
  }
  await party.act("stats");
  for (let i = 0; i < inputs; i++) {
    send(Math.sin(i / 10), Math.cos(i / 10));
    await new Promise((r) => setTimeout(r, 1000 / hz));
  }
  await new Promise((r) => setTimeout(r, 300));
  const host = await party.act("stats");
  return { transport: party.transport, rtts, host, sent: inputs };
};
