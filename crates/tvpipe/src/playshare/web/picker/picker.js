"use strict";
/**
 * The game picker (docs/game-catalog.md): `tvpipe game play` without a
 * game opens the window here. The games list comes from tvpipe (which
 * fetched it and its screenshots from tvpipe.party); picking a game or
 * typing a source asks tvpipe to fetch it, a new one is asked about, and
 * once tvpipe started the game this page goes to its launcher.
 */
const $ = (id) => document.getElementById(id);
const LANGUAGES = {
    en: "English",
    de: "German",
    fr: "French",
    es: "Spanish",
    it: "Italian",
    nl: "Dutch",
    pt: "Portuguese",
    ja: "Japanese",
};
let current = { state: "idle" };
let polling = false;
function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls)
        e.className = cls;
    if (text !== undefined)
        e.textContent = text;
    return e;
}
function setState(text, cls = "") {
    const st = $("state");
    st.textContent = text;
    st.className = `state ${cls}`;
}
function showError(text) {
    const e = $("error");
    e.hidden = !text;
    e.textContent = text ?? "";
}
function players(g) {
    if (g.min === undefined || g.max === undefined)
        return "";
    return g.min === g.max ? `${g.min} players` : `${g.min}–${g.max} players`;
}
function renderGames(c) {
    const list = $("games");
    list.replaceChildren();
    for (const g of c.games) {
        const li = el("li", "game");
        li.tabIndex = 0;
        li.dataset["name"] = g.name;
        li.setAttribute("role", "button");
        li.setAttribute("aria-label", `Play ${g.title}`);
        const cover = g.screenshots.find((s) => s.kind === "tv") ?? g.screenshots[0];
        if (cover) {
            const img = el("img", "cover");
            img.src = cover.src;
            img.alt = cover.alt;
            img.loading = "lazy";
            img.onerror = () => img.removeAttribute("src");
            li.append(img);
        }
        else {
            li.append(el("div", "cover"));
        }
        const bar = el("div", "bar");
        bar.append(el("i"));
        li.append(bar);
        if (g.downloaded)
            li.append(el("span", "tag", "Downloaded"));
        const text = el("div", "text");
        text.append(el("h3", "", g.title));
        if (g.summary)
            text.append(el("p", "summary", g.summary));
        const facts = [players(g), g.languages.map((l) => LANGUAGES[l] ?? l).join(", ")].filter(Boolean).join(" · ");
        if (facts)
            text.append(el("p", "facts", facts));
        li.append(text);
        const pick = () => void choose(g.name);
        li.onclick = pick;
        li.onkeydown = (e) => {
            if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                pick();
            }
        };
        list.append(li);
    }
    const empty = $("empty");
    empty.hidden = c.games.length > 0;
    empty.textContent = c.error
        ? `The games list did not load: ${c.error}. A game from elsewhere still works.`
        : "The games list is empty.";
}
async function call(body) {
    const r = await fetch("/_tvpipe/api/game", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
    });
    const v = (await r.json());
    if (!r.ok)
        throw new Error(v.error ?? `${r.status}`);
    return v;
}
async function choose(source) {
    if (current.state !== "idle" && current.state !== "failed")
        return;
    showError(null);
    try {
        if (current.state === "failed")
            await call({ back: true });
        render(await call({ source }));
        void poll();
    }
    catch (err) {
        showError(err.message);
    }
}
function human(n) {
    if (n < 1024)
        return `${n} B`;
    if (n < 1 << 20)
        return `${(n / 1024).toFixed(1)} KiB`;
    return `${(n / (1 << 20)).toFixed(1)} MiB`;
}
function ask(g) {
    const d = $("ask");
    $("ask-title").textContent = g.title;
    $("ask-changed").hidden = !g.changed;
    const facts = $("ask-facts");
    facts.replaceChildren();
    const row = (k, v, cls = "") => {
        facts.append(el("dt", "", k));
        facts.append(el("dd", cls, v));
    };
    row("From", g.from);
    if (g.release)
        row("Release", g.release);
    row("Size", `${g.size}, ${g.files} files (${g.unpacked} unpacked)`);
    row("Made for", g.tvpipe ? `tvpipe ${g.tvpipe}` : "any tvpipe");
    row("SHA-256", g.sha256, "hash");
    if (!d.open)
        d.showModal();
}
function render(s) {
    current = s;
    const list = $("games");
    const busy = s.state === "fetching" || s.state === "starting" || s.state === "ready" || s.state === "ask";
    list.classList.toggle("locked", busy);
    $("open").disabled = busy;
    $("source").disabled = busy;
    for (const li of list.querySelectorAll(".game")) {
        const mine = "source" in s ? li.dataset["name"] === s.source : false;
        li.classList.toggle("busy", busy && (mine || (s.state !== "fetching" && li.classList.contains("busy"))));
        const bar = li.querySelector(".bar i");
        if (bar && s.state === "fetching" && mine) {
            bar.style.width = s.total ? `${Math.round((100 * s.done) / s.total)}%` : "30%";
        }
    }
    const dialog = $("ask");
    if (s.state !== "ask" && dialog.open)
        dialog.close();
    switch (s.state) {
        case "idle":
            setState("Pick a game", "ok");
            $("game").textContent = "Pick one";
            break;
        case "fetching":
            setState(s.done > 0 ? `Downloading ${human(s.done)}${s.total ? ` of ${human(s.total)}` : ""}` : "Downloading…", "on");
            $("game").textContent = s.source;
            break;
        case "ask":
            setState("Waiting for you", "on");
            $("game").textContent = s.game.title;
            ask(s.game);
            break;
        case "starting":
            setState("Starting…", "on");
            $("game").textContent = s.title;
            break;
        case "ready":
            setState("Starting…", "on");
            location.assign(s.url);
            break;
        case "failed":
            setState("Something failed", "bad");
            showError(`${s.source}: ${s.error}`);
            break;
    }
}
async function poll() {
    if (polling)
        return;
    polling = true;
    try {
        for (;;) {
            const r = await fetch("/_tvpipe/api/game");
            if (!r.ok)
                throw new Error(`tvpipe answered ${r.status}`);
            render((await r.json()));
            if (current.state === "idle" || current.state === "failed" || current.state === "ready" || current.state === "ask")
                break;
            await new Promise((ok) => setTimeout(ok, 200));
        }
    }
    catch (err) {
        setState("Not connected", "bad");
        showError(`${err.message}: is tvpipe still running?`);
    }
    finally {
        polling = false;
    }
}
/** The light/dark/system switch, shared with tvpipe's other pages. */
function themes() {
    const buttons = document.querySelectorAll(".theme button");
    const set = (t) => {
        document.documentElement.dataset["theme"] = t;
        try {
            localStorage.setItem("tvpipe-theme", t);
        }
        catch {
            // No storage: the choice lasts for this page.
        }
        buttons.forEach((b) => b.setAttribute("aria-pressed", String(b.dataset["t"] === t)));
    };
    set(document.documentElement.dataset["theme"] ?? "auto");
    buttons.forEach((b) => (b.onclick = () => set(b.dataset["t"] ?? "auto")));
}
async function main() {
    themes();
    $("where").textContent = `tvpipe game · ${location.host}`;
    $("other").onsubmit = (e) => {
        e.preventDefault();
        const v = $("source").value.trim();
        if (v)
            void choose(v);
    };
    $("ask-form").onsubmit = (e) => {
        e.preventDefault();
        void call({ trust: true })
            .then((s) => {
            render(s);
            void poll();
        })
            .catch((err) => showError(err.message));
    };
    $("ask-no").onclick = () => {
        void call({ back: true })
            .then(render)
            .catch((err) => showError(err.message));
    };
    $("ask").oncancel = (e) => {
        e.preventDefault();
        $("ask-no").click();
    };
    // A reload in the middle: pick up where tvpipe is.
    void poll();
    try {
        const r = await fetch("/_tvpipe/api/catalog");
        if (r.status === 403) {
            setState("Not connected", "bad");
            $("title").textContent = "Open the link tvpipe printed";
            $("message").textContent = "This page talks to tvpipe through the link it printed (or the window it opened).";
            return;
        }
        const c = (await r.json());
        renderGames(c);
        render(current);
    }
    catch (err) {
        showError(`The games list did not load: ${err.message}`);
    }
}
void main();
