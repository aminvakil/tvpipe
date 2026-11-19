/**
 * tvpipe's standard launcher (docs/playshare-design.md §7): the TVs as
 * they are found, AirPlay PIN pairing, then the game's host page. A game
 * may bring its own launcher with the same API.
 */

import { Tvpipe, type Device, type Info, type PairStatus } from "../tvpipe.ts";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const KIND: Record<Device["kind"], string> = {
  airplay: "AirPlay",
  cast: "Chromecast",
  both: "AirPlay · Cast",
  browser: "This computer",
};

const selected = new Set<string>();
const pairing = new Map<string, PairStatus>();
let devices: Device[] = [];
let info: Info;
let tvpipe: Tvpipe;
/** The device the PIN dialog is open for. */
let pinFor: string | null = null;

function showError(text: string | null): void {
  const e = $("error");
  e.hidden = !text;
  e.textContent = text ?? "";
}

function render(): void {
  const list = $("devices");
  list.replaceChildren(
    ...devices.map((d) => {
      const li = document.createElement("li");
      li.className = `device${selected.has(d.name) ? " selected" : ""}`;
      li.dataset["name"] = d.name;
      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = selected.has(d.name);
      box.setAttribute("aria-label", d.name);
      const text = document.createElement("div");
      text.className = "grow";
      const name = document.createElement("div");
      name.className = "name";
      name.textContent = d.kind === "browser" ? (d.title ?? "A browser tab on this computer") : d.name;
      const meta = document.createElement("div");
      meta.className = "meta";
      meta.textContent =
        d.kind === "browser" ? "Try the game without a TV: the TV picture plays in a tab" : (d.model ?? "");
      text.append(name, meta);
      const p = pairing.get(d.name);
      if (p) {
        const st = document.createElement("div");
        st.className = "state";
        st.textContent =
          p.state === "done" ? "Paired" : p.state === "failed" ? `Pairing failed: ${p.error ?? ""}` : "Pairing…";
        text.append(st);
      }
      const badge = document.createElement("span");
      badge.className = "badge";
      badge.textContent = KIND[d.kind] ?? d.kind;
      li.append(box, text, badge);
      if (d.needsPairing && pairing.get(d.name)?.state !== "done") {
        const pair = document.createElement("button");
        pair.textContent = "Pair";
        pair.onclick = (ev) => {
          ev.stopPropagation();
          void startPairing(d.name);
        };
        li.append(pair);
      }
      li.onclick = () => {
        if (selected.has(d.name)) selected.delete(d.name);
        else selected.add(d.name);
        render();
      };
      return li;
    }),
  );
  $("searching").hidden = devices.some((d) => d.kind !== "browser");
  ($("start") as HTMLButtonElement).disabled = selected.size === 0;
}

function renderPhones(): void {
  const t = info.tunnel;
  const p = $("phones");
  if (t.state === "starting") p.textContent = "Opening the link for phones…";
  else if (t.state === "up") p.textContent = `Phones join through ${t.url}`;
  else if (info.phone.mode === "lan") p.textContent = `Phones join on this Wi-Fi: ${info.phone.lan ?? info.phone.url}`;
  else if (t.state === "failed") p.textContent = `No public link for phones (${t.error ?? "the tunnel failed"}): phones on this computer only.`;
  else p.textContent = "Phones on this computer only (no tunnel).";
}

async function startPairing(device: string): Promise<void> {
  showError(null);
  pairing.set(device, { device, state: "connecting" });
  render();
  try {
    await tvpipe.pair(device);
  } catch (err) {
    pairing.set(device, { device, state: "failed", error: (err as Error).message });
    render();
  }
}

function openPin(device: string, error?: string): void {
  pinFor = device;
  $("pin-device").textContent = device;
  const e = $("pin-error");
  e.hidden = !error;
  e.textContent = error ?? "";
  ($("pin-input") as HTMLInputElement).value = "";
  const dlg = $("pin") as HTMLDialogElement;
  if (!dlg.open) dlg.showModal();
  ($("pin-input") as HTMLInputElement).focus();
}

function closePin(): void {
  pinFor = null;
  const dlg = $("pin") as HTMLDialogElement;
  if (dlg.open) dlg.close();
}

async function start(devicesToUse: string[]): Promise<void> {
  showError(null);
  try {
    await tvpipe.select(devicesToUse);
    await tvpipe.watchDevices(false);
    location.assign(info.pages.host);
  } catch (err) {
    showError((err as Error).message);
  }
}

async function main(): Promise<void> {
  try {
    tvpipe = await Tvpipe.connect();
  } catch (err) {
    $("title").textContent = "Not connected";
    showError(`${(err as Error).message}. Open the link tvpipe printed.`);
    return;
  }
  info = await tvpipe.info();
  document.title = `${info.title} · tvpipe`;
  $("title").textContent = info.title;
  const { min, max } = info.players;
  $("players").textContent = min === max ? `${min} players` : `${min}–${max} players, each with a phone`;
  renderPhones();
  for (const name of await tvpipe.selection()) selected.add(name);
  tvpipe.on("devices", (list) => {
    devices = list;
    render();
  });
  tvpipe.on("tunnel", (t) => {
    info.tunnel = t;
    if (t.state === "up" && t.url) info.phone = { ...info.phone, url: t.url, mode: "tunnel" };
    renderPhones();
  });
  tvpipe.on("pair", (p) => {
    pairing.set(p.device, p);
    if (p.state === "pin") openPin(p.device);
    else if (pinFor === p.device && p.state !== "connecting") closePin();
    if (p.state === "done") selected.add(p.device);
    render();
  });
  tvpipe.on("close", () => showError("tvpipe stopped."));
  tvpipe.on("closing", () => showError("tvpipe is ending."));
  devices = await tvpipe.watchDevices(true);
  render();

  $("start").onclick = () => void start([...selected]);
  $("no-tv").onclick = () => void start([]);
  ($("pin-form") as HTMLFormElement).onsubmit = (ev) => {
    ev.preventDefault();
    const pin = ($("pin-input") as HTMLInputElement).value.trim();
    if (!pinFor || !/^\d{4,8}$/.test(pin)) {
      const e = $("pin-error");
      e.hidden = false;
      e.textContent = "The code is the digits the TV shows.";
      return;
    }
    tvpipe.pairPin(pinFor, pin).catch((err) => openPin(pinFor ?? "", (err as Error).message));
  };
  $("pin-cancel").onclick = () => {
    if (pinFor) void tvpipe.pairCancel(pinFor);
    if (pinFor) pairing.delete(pinFor);
    closePin();
    render();
  };
}

void main();
