/**
 * The local API of `tvpipe playshare` (docs/playshare-design.md §5): the
 * launcher and the host page talk to tvpipe with it. One WebSocket
 * (`/_tvpipe/api`, the session cookie or `?t=TOKEN`): calls, events, and
 * the relay of the phones' connections once the host attaches.
 *
 *   const tvpipe = await Tvpipe.connect();
 *   const party = await tvpipe.party({ lateJoin: "wait" });   // the host
 *   await party.cast();                                      // the TV page on the chosen TVs
 */
import { Emitter } from "./emitter.js";
import { Party } from "./party.js";
import { TvpipeRelay } from "./relay.js";
/** A refused call. */
export class TvpipeError extends Error {
    code;
    constructor(code, message) {
        super(message);
        this.name = "TvpipeError";
        this.code = code;
    }
}
export class Tvpipe extends Emitter {
    /** Connects to tvpipe's API. */
    static connect(opts = {}) {
        let url = opts.url;
        if (!url) {
            const loc = globalThis.location;
            url = `${loc.protocol === "https:" ? "wss:" : "ws:"}//${loc.host}/_tvpipe/api`;
        }
        if (opts.token)
            url += `${url.includes("?") ? "&" : "?"}t=${encodeURIComponent(opts.token)}`;
        const WS = opts.WebSocket ?? globalThis.WebSocket;
        const ws = new WS(url);
        ws.binaryType = "arraybuffer";
        return new Promise((resolve, reject) => {
            ws.onopen = () => resolve(new Tvpipe(ws));
            ws.onerror = () => reject(new TvpipeError("connect", `cannot reach tvpipe at ${url.replace(/t=[^&]*/, "t=…")}`));
        });
    }
    #ws;
    #n = 0;
    #calls = new Map();
    #relay;
    #closed = false;
    constructor(ws) {
        super();
        this.#ws = ws;
        ws.onerror = null;
        ws.onmessage = (ev) => {
            if (typeof ev.data === "string")
                this.#onText(ev.data);
            else if (ev.data instanceof ArrayBuffer)
                this.#relay?.handleBinary(new Uint8Array(ev.data));
        };
        ws.onclose = () => {
            this.#closed = true;
            const err = new TvpipeError("closed", "the connection to tvpipe closed");
            for (const w of this.#calls.values())
                w.reject(err);
            this.#calls.clear();
            this.#relay?.handleDetach();
            this.emit("close");
        };
    }
    get closed() {
        return this.#closed;
    }
    #onText(text) {
        let msg;
        try {
            msg = JSON.parse(text);
        }
        catch {
            return;
        }
        if (typeof msg !== "object" || msg === null)
            return;
        if (msg["t"] === "reply") {
            const w = this.#calls.get(msg["n"]);
            if (!w)
                return;
            this.#calls.delete(msg["n"]);
            if (msg["ok"])
                w.resolve(msg["value"]);
            else
                w.reject(new TvpipeError(msg["error"]?.code ?? "failed", msg["error"]?.message ?? "failed"));
            return;
        }
        if (msg["t"] === "event") {
            const data = msg["data"];
            switch (msg["type"]) {
                case "devices":
                    this.emit("devices", Array.isArray(data) ? data : []);
                    return;
                case "pair":
                    this.emit("pair", data);
                    return;
                case "cast":
                    this.emit("cast", data);
                    return;
                case "tunnel":
                    this.emit("tunnel", data);
                    return;
                case "closing":
                    this.#relay?.onclosing?.();
                    this.emit("closing");
                    return;
                case "detached":
                    this.emit("detached");
                    return;
            }
            return;
        }
        this.#relay?.handleControl(msg);
    }
    /** Calls an API method (docs/playshare-design.md §5). */
    call(method, params = {}) {
        if (this.#closed)
            return Promise.reject(new TvpipeError("closed", "the connection to tvpipe closed"));
        const n = ++this.#n;
        return new Promise((resolve, reject) => {
            this.#calls.set(n, { resolve, reject });
            this.#ws.send(JSON.stringify({ t: "call", n, method, params }));
        });
    }
    info() {
        return this.call("info");
    }
    /** Starts (or stops) discovery; `devices` events follow. Resolves with the list so far. */
    watchDevices(on = true) {
        return this.call("devices.watch", { on });
    }
    /** The launcher's choice, kept by tvpipe across the navigation to the host page. */
    async select(devices) {
        await this.call("select", { devices });
    }
    async selection() {
        return (await this.call("selection")).devices;
    }
    /** AirPlay PIN pairing: `pair` events say when the PIN is wanted. */
    async pair(device) {
        await this.call("pair.start", { device });
    }
    async pairPin(device, pin) {
        await this.call("pair.pin", { device, pin });
    }
    async pairCancel(device) {
        await this.call("pair.cancel", { device });
    }
    /** Casts a page of this game (a path with its fragment) to `to` (default: the selection). */
    cast(opts) {
        const params = { page: opts.page };
        if (opts.to)
            params["to"] = opts.to;
        return this.call("cast.start", params);
    }
    async stopCast() {
        await this.call("cast.stop");
    }
    /** A QR code made by tvpipe: `size` modules, `rows` of "0"/"1". */
    qr(text) {
        return this.call("qr", { text });
    }
    /** Ends playshare (the TVs stop, phones are told). */
    async quit() {
        await this.call("quit").catch(() => { });
    }
    /** Becomes the relay's host: phones' connections come here. */
    async relay() {
        const r = await this.call("relay.attach");
        const ws = this.#ws;
        this.#relay = new TvpipeRelay({
            sendText: (t) => ws.send(t),
            sendBinary: (d) => ws.send(d),
        }, r.public);
        return this.#relay;
    }
    /** The host: a party on the relay, its links and QR code, casting. */
    async party(options = {}) {
        const info = await this.info();
        const relay = await this.relay();
        const party = new HostedParty(this, info, relay, options);
        await party.refreshJoin();
        return party;
    }
    /** Closes the API socket (tvpipe ends after a while without a page). */
    close() {
        this.#ws.close();
    }
}
/** A {@link Party} in a host page, with tvpipe behind it. */
export class HostedParty extends Party {
    tvpipe;
    info;
    relay;
    links;
    constructor(tvpipe, info, relay, options) {
        super({ maxPlayers: info.players.max, ...options });
        this.tvpipe = tvpipe;
        this.info = info;
        this.relay = relay;
        this.listen(relay);
        const tv = `${info.pages.tv}#r=${this.code}&k=${this.screenKey}&role=screen`;
        this.links = {
            join: this.#joinUrl(info.phone.url),
            tv,
            watch: `${info.local}${info.pages.tv}#r=${this.code}&k=${this.watchKey}&role=watch`,
        };
        tvpipe.on("tunnel", (t) => {
            if (t.state === "up" && t.url) {
                this.info.phone = { ...this.info.phone, url: t.url, mode: "tunnel" };
                void this.refreshJoin();
            }
        });
    }
    #joinUrl(base) {
        return `${base.endsWith("/") ? base : `${base}/`}?r=${this.code}#k=${this.joinKey}`;
    }
    /** Makes the join link and its QR code (again: after the tunnel came up). */
    async refreshJoin() {
        this.links = { ...this.links, join: this.#joinUrl(this.info.phone.url) };
        let qr = null;
        try {
            qr = await this.tvpipe.qr(this.links.join);
        }
        catch { }
        this.joinInfo = { code: this.code, url: this.links.join, qr, mode: this.info.phone.mode };
    }
    /** Casts the TV page to `to` (default: what the launcher chose). */
    cast(to) {
        return this.tvpipe.cast({ page: this.links.tv, to });
    }
    stopCast() {
        return this.tvpipe.stopCast();
    }
}
