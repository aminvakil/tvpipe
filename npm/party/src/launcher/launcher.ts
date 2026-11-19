/**
 * tvpipe's standard launcher (docs/playshare-design.md §7): the TVs as
 * they are found, AirPlay PIN pairing, then the game's host page. A game
 * may bring its own launcher with the same API.
 */

import { PICKER_MESSAGE, Tvpipe, type Device, type Info, type PairStatus, type Resolution, type Selection } from "../tvpipe.ts";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const KIND: Record<Device["kind"], string> = {
  airplay: "AirPlay",
  cast: "Cast",
  both: "AirPlay · Cast",
  browser: "This computer",
};

/**
 * `?mode=change`: the cast picker (docs/playshare-design.md §7.1), in a
 * dialog over the host page while the game runs: it saves the choice and
 * tells the page, which casts again. Else the start of the game.
 */
const changing = new URLSearchParams(location.search).get("mode") === "change";

const RESOLUTION_LABEL: Record<Resolution, string> = { auto: "Auto", "720p": "720p", "1080p": "1080p", "2160p": "4K" };
const RESOLUTION_NOTE: Partial<Record<Resolution, string>> = {
  auto: "What the TVs take, up to 1080p.",
  "2160p": "4K needs a 4K TV and a fast computer.",
};

const selected = new Set<string>();
/** `null`: the game's picture has a fixed size. */
let resolution: Resolution | null = null;
const pairing = new Map<string, PairStatus>();
let devices: Device[] = [];
let info: Info;
let tvpipe: Tvpipe;
/** The device the PIN dialog is open for. */
let pinFor: string | null = null;
/** Set while the selection is cast and the host page loads. */
let starting: string | null = null;
/** Set when tvpipe is gone or never answered. */
let stopped: string | null = null;

function showError(text: string | null): void {
  const e = $("error");
  e.hidden = !text;
  e.textContent = text ?? "";
  renderState();
}

/** The status column: one state with a dot (grey, accent, pulsing, red). */
function renderState(): void {
  const tvs = devices.filter((d) => d.kind !== "browser").length;
  const busy = [...pairing.values()].find((p) => p.state === "connecting" || p.state === "pin");
  let text: string;
  let cls = "";
  if (stopped) [text, cls] = [stopped, "bad"];
  else if (starting) [text, cls] = [starting, "on"];
  else if (!$("error").hidden) [text, cls] = ["Something failed", "bad"];
  else if (busy) [text, cls] = [busy.state === "pin" ? "Waiting for the code" : "Pairing", "on"];
  else if (!info) text = "Connecting…";
  else if (tvs === 0) [text, cls] = ["Looking for TVs", "on"];
  else [text, cls] = [tvs === 1 ? "1 TV found" : `${tvs} TVs found`, "ok"];
  const st = $("state");
  st.textContent = text;
  st.className = `state ${cls}`;
}

function el(tag: string, cls: string, text?: string): HTMLElement {
  const e = document.createElement(tag);
  e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function pairingText(p: PairStatus): [string, string] {
  if (p.state === "done") return ["Paired", "done"];
  if (p.state === "failed") return [`Pairing failed${p.error ? `: ${p.error}` : ""}`, "bad"];
  if (p.state === "pin") return ["Type the code the TV shows", ""];
  return ["Pairing…", ""];
}

function render(): void {
  const list = $("devices");
  list.replaceChildren(
    ...devices.map((d) => {
      const li = el("li", `device${selected.has(d.name) ? " selected" : ""}`);
      li.dataset["name"] = d.name;
      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = selected.has(d.name);
      box.setAttribute("aria-label", d.name);
      const text = el("div", "grow");
      const browser = d.kind === "browser";
      text.append(
        el("div", "name", browser ? (d.title ?? "A browser tab on this computer") : d.name),
        el("div", "meta", browser ? "Try the game without a TV: the TV picture plays in a tab" : (d.model ?? "")),
      );
      const p = pairing.get(d.name);
      if (p) {
        const [t, c] = pairingText(p);
        text.append(el("div", `pairing ${c}`, t));
      }
      const end = el("div", "end");
      end.append(el("span", "tag", KIND[d.kind] ?? d.kind));
      if (d.needsPairing && pairing.get(d.name)?.state !== "done") {
        const pair = el("button", "btn small", "Pair") as HTMLButtonElement;
        pair.onclick = (ev) => {
          ev.stopPropagation();
          void startPairing(d.name);
        };
        end.append(pair);
      }
      li.append(box, text, end);
      li.onclick = () => {
        if (selected.has(d.name)) selected.delete(d.name);
        else selected.add(d.name);
        render();
      };
      return li;
    }),
  );
  $("searching").hidden = devices.some((d) => d.kind !== "browser");
  // The picker may also take every TV away (the cast stops).
  ($("start") as HTMLButtonElement).disabled = (selected.size === 0 && !changing) || starting !== null;
  renderState();
}

function renderPhones(): void {
  const t = info.tunnel;
  const url = (u: string) => el("code", "", u);
  const parts: (string | Node)[] =
    t.state === "starting"
      ? ["Opening the public link for phones…"]
      : t.state === "up" && t.url
        ? ["Phones join through ", url(t.url), ", on any network."]
        : info.phone.mode === "lan"
          ? ["Phones on this Wi-Fi join at ", url(info.phone.lan ?? info.phone.url), "."]
          : t.state === "failed"
            ? [el("span", "bad", `No public link for phones (${t.error ?? "the tunnel failed"}).`), " Phones on this computer only."]
            : ["Phones on this computer only: no public link (", url("--no-tunnel"), ")."];
  $("phones").replaceChildren(...parts);
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

/** The resolution buttons (none when the game's size is fixed). */
function renderResolution(): void {
  const box = $("res");
  const choices = info?.resolutions ?? [];
  box.hidden = choices.length === 0 || resolution === null;
  if (box.hidden) return;
  $("res-options").replaceChildren(
    ...choices.map((r) => {
      const b = el("button", "", RESOLUTION_LABEL[r] ?? r) as HTMLButtonElement;
      b.type = "button";
      b.setAttribute("role", "radio");
      b.setAttribute("aria-checked", String(r === resolution));
      b.onclick = () => {
        resolution = r;
        renderResolution();
      };
      return b;
    }),
  );
  $("res-note").textContent = (resolution && RESOLUTION_NOTE[resolution]) ?? "";
}

/** The cast picker's answer to the page it is in (`null`: cancelled). */
function answer(selection: Selection | null): void {
  if (window.parent !== window) {
    window.parent.postMessage({ type: PICKER_MESSAGE, selection }, location.origin);
    return;
  }
  // Opened by itself: nothing to tell.
  starting = selection ? "Saved" : null;
  render();
}

async function start(devicesToUse: string[]): Promise<void> {
  showError(null);
  if (changing) {
    try {
      answer(await tvpipe.select(devicesToUse, { resolution: resolution ?? undefined }));
    } catch (err) {
      showError((err as Error).message);
    }
    return;
  }
  starting = devicesToUse.some((d) => d !== "browser") ? "Casting…" : "Starting…";
  render();
  try {
    await tvpipe.select(devicesToUse, { resolution: resolution ?? undefined });
    await tvpipe.watchDevices(false);
    location.assign(info.pages.host);
  } catch (err) {
    starting = null;
    render();
    showError((err as Error).message);
  }
}

/** The light/dark/system switch, shared with tvpipe's other pages. */
function themes(): void {
  const buttons = document.querySelectorAll<HTMLButtonElement>(".theme button");
  const set = (t: string) => {
    document.documentElement.dataset["theme"] = t;
    try {
      localStorage.setItem("tvpipe-theme", t);
    } catch {
      // No storage: the choice lasts for this page.
    }
    buttons.forEach((b) => b.setAttribute("aria-pressed", String(b.dataset["t"] === t)));
  };
  set(document.documentElement.dataset["theme"] ?? "auto");
  buttons.forEach((b) => (b.onclick = () => set(b.dataset["t"] ?? "auto")));
}

async function main(): Promise<void> {
  themes();
  $("where").textContent = `tvpipe game · ${location.host}`;
  try {
    tvpipe = await Tvpipe.connect();
  } catch (err) {
    stopped = "Not connected";
    $("title").textContent = "Open the link tvpipe printed";
    $("message").textContent = "This page talks to tvpipe through the link it printed (or the window it opened).";
    showError(`${(err as Error).message}.`);
    return;
  }
  info = await tvpipe.info();
  document.title = `${info.title} · tvpipe`;
  $("game").textContent = info.title;
  $("game").title = info.title;
  const { min, max } = info.players;
  $("players").textContent = min === max ? `${min}` : `${min}–${max}`;
  $("message").textContent = `${info.title} shows on the TVs you pick. Everyone plays on their own phone.`;
  if (changing) {
    document.documentElement.dataset["mode"] = "change";
    $("title").textContent = "Change the TVs";
    $("message").textContent = "The TVs you pick show the game; they start the picture again.";
    $("start").textContent = "Apply";
    $("no-tv").textContent = "Cancel";
  }
  renderPhones();
  const chosen = await tvpipe.chosen();
  for (const name of chosen.devices) selected.add(name);
  resolution = chosen.resolution;
  renderResolution();
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
  tvpipe.on("close", () => {
    stopped = "Stopped";
    showError("tvpipe stopped. Run it again to play.");
  });
  tvpipe.on("closing", () => {
    stopped = "Ending";
    showError("tvpipe is ending.");
  });
  devices = await tvpipe.watchDevices(true);
  render();

  $("start").onclick = () => void start([...selected]);
  $("no-tv").onclick = () => (changing ? answer(null) : void start([]));
  if (changing) {
    addEventListener("keydown", (ev) => {
      if (ev.key === "Escape" && !($("pin") as HTMLDialogElement).open) answer(null);
    });
  }
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
