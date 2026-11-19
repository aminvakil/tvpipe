/**
 * The connections the host gets from tvpipe's relay
 * (docs/playshare-design.md §6): every phone or TV page's WebSocket ends
 * in tvpipe, which hands it to the host page as a {@link RelayConn}.
 * {@link TvpipeRelay} speaks the relay protocol over the API socket;
 * `memoryRelay()` (testing.ts) connects clients in the same process.
 */
/** Relay frame ops. */
export const OP_TEXT = 1;
export const OP_BINARY = 2;
class Conn {
    id;
    via;
    address;
    query;
    agent;
    busy = false;
    closed = false;
    onmessage;
    onclose;
    ondrain;
    #sink;
    constructor(sink, open) {
        this.#sink = sink;
        this.id = Number(open["conn"]);
        const via = open["via"];
        this.via = via === "lan" || via === "local" ? via : "public";
        this.address = String(open["addr"] ?? "?");
        this.query = String(open["query"] ?? "");
        this.agent = String(open["agent"] ?? "");
    }
    send(text) {
        if (this.closed)
            return;
        this.#sink.sendBinary(frame(OP_TEXT, this.id, encoder.encode(text)));
    }
    sendBinary(data) {
        if (this.closed)
            return;
        this.#sink.sendBinary(frame(OP_BINARY, this.id, data));
    }
    close(code, reason = "") {
        if (this.closed)
            return;
        this.closed = true;
        this.#sink.sendText(JSON.stringify({ t: "close", conn: this.id, code, reason: reason.slice(0, 60) }));
    }
}
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: false });
/** `op | conn (u32 big-endian) | payload`. */
export function frame(op, conn, payload) {
    const f = new Uint8Array(5 + payload.length);
    f[0] = op;
    new DataView(f.buffer).setUint32(1, conn >>> 0);
    f.set(payload, 5);
    return f;
}
/**
 * The relay over tvpipe's API socket: `handleText`/`handleBinary` get
 * what tvpipe sends, `sink` sends frames back.
 */
export class TvpipeRelay {
    onopen;
    onclosing;
    publicUrl;
    #sink;
    #conns = new Map();
    constructor(sink, publicUrl) {
        this.#sink = sink;
        this.publicUrl = publicUrl;
    }
    /** A control message from tvpipe; false when it was not one. */
    handleControl(msg) {
        const id = Number(msg["conn"]);
        switch (msg["t"]) {
            case "open": {
                const c = new Conn(this.#sink, msg);
                this.#conns.set(c.id, c);
                if (this.onopen)
                    this.onopen(c);
                else
                    c.close(1013, "no host");
                return true;
            }
            case "close": {
                const c = this.#conns.get(id);
                if (!c)
                    return true;
                this.#conns.delete(id);
                c.closed = true;
                c.onclose?.(Number(msg["code"] ?? 1006), String(msg["reason"] ?? ""));
                return true;
            }
            case "busy": {
                const c = this.#conns.get(id);
                if (c)
                    c.busy = true;
                return true;
            }
            case "drain": {
                const c = this.#conns.get(id);
                if (c) {
                    c.busy = false;
                    c.ondrain?.();
                }
                return true;
            }
            default:
                return false;
        }
    }
    /** A data frame from tvpipe. */
    handleBinary(data) {
        if (data.length < 5)
            return;
        const id = new DataView(data.buffer, data.byteOffset, data.byteLength).getUint32(1);
        const c = this.#conns.get(id);
        if (!c || c.closed)
            return;
        const payload = data.subarray(5);
        if (data[0] === OP_TEXT)
            c.onmessage?.(decoder.decode(payload));
        else if (data[0] === OP_BINARY)
            c.onmessage?.(payload.slice());
    }
    /** The API socket closed: every connection is gone (tvpipe closes them). */
    handleDetach() {
        for (const c of this.#conns.values()) {
            c.closed = true;
            c.onclose?.(1012, "host restart");
        }
        this.#conns.clear();
    }
    get size() {
        return this.#conns.size;
    }
}
