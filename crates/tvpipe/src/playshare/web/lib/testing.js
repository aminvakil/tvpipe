/**
 * Test helpers: a relay in memory (`memoryRelay()`), so a host and fake
 * phones run in one process without tvpipe: `relay.WebSocket` is a
 * WebSocket class for `PartyClient` whose sockets end in the relay.
 * Delivery is asynchronous and in order, like a socket's.
 */
const later = (f) => setTimeout(f, 0);
class HostSide {
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
    #client;
    #done;
    #ended = false;
    constructor(id, client, via, address, query, done) {
        this.id = id;
        this.#done = done;
        this.#client = client;
        this.via = via;
        this.address = address;
        this.query = query;
        this.agent = "memory";
    }
    send(text) {
        if (this.closed)
            return;
        later(() => this.#client.deliver(text));
    }
    sendBinary(data) {
        if (this.closed)
            return;
        const copy = data.slice();
        later(() => this.#client.deliver(copy));
    }
    close(code, reason = "") {
        if (this.closed)
            return;
        this.closed = true;
        later(() => {
            this.#client.closedBy(code, reason);
            this.ended(code, reason);
        });
    }
    /** The connection is over (either side): the host hears of it once. */
    ended(code, reason) {
        if (this.#ended)
            return;
        this.#ended = true;
        this.closed = true;
        this.#done();
        this.onclose?.(code, reason);
    }
}
class FakeSocket {
    static CONNECTING = 0;
    static OPEN = 1;
    static CLOSED = 3;
    readyState = 0;
    onopen = null;
    onmessage = null;
    onclose = null;
    onerror = null;
    url;
    #host;
    constructor(relay, url, via, address) {
        this.url = url;
        later(() => {
            if (this.readyState !== 0)
                return;
            const conn = relay.open(this, via, address, url.includes("?") ? url.slice(url.indexOf("?") + 1) : "");
            if (!conn) {
                // tvpipe answers 503 without a host.
                this.readyState = 3;
                this.onerror?.({});
                this.onclose?.({ code: 1006, reason: "" });
                return;
            }
            this.#host = conn;
            this.readyState = 1;
            this.onopen?.({});
        });
    }
    send(data) {
        if (this.readyState !== 1)
            return;
        const host = this.#host;
        later(() => {
            if (host && !host.closed)
                host.onmessage?.(data);
        });
    }
    close(code = 1000, reason = "") {
        if (this.readyState === 3)
            return;
        const wasOpen = this.readyState === 1;
        this.readyState = 3;
        const host = this.#host;
        later(() => {
            if (wasOpen)
                host?.ended(code, reason);
            this.onclose?.({ code, reason });
        });
    }
    /** From the host. */
    deliver(data) {
        if (this.readyState !== 1)
            return;
        this.onmessage?.({ data });
    }
    /** The host closed it. */
    closedBy(code, reason) {
        if (this.readyState === 3)
            return;
        this.readyState = 3;
        this.onclose?.({ code, reason });
    }
}
/** A relay in memory; see the module docs. */
export class MemoryRelay {
    onopen;
    onclosing;
    publicUrl;
    /** A WebSocket class for `PartyClient` (`via: "public"`, address `127.0.0.1`). */
    WebSocket;
    #next = 0;
    #conns = new Set();
    constructor(publicUrl = "https://party.test/") {
        this.publicUrl = publicUrl;
        const relay = this;
        this.WebSocket = class extends FakeSocket {
            constructor(url) {
                super(relay, url, "public", "127.0.0.1");
            }
        };
    }
    /** A WebSocket class whose sockets come from `via` and `address`. */
    socketFrom(via, address) {
        const relay = this;
        return class extends FakeSocket {
            constructor(url) {
                super(relay, url, via, address);
            }
        };
    }
    /** @internal */
    open(client, via, address, query) {
        if (!this.onopen)
            return undefined;
        const conn = new HostSide(++this.#next, client, via, address, query, () => this.#conns.delete(conn));
        this.#conns.add(conn);
        this.onopen(conn);
        return conn;
    }
    /** Connections open now. */
    get size() {
        return this.#conns.size;
    }
    /** What tvpipe does at the end: `closing`, then every client closed with 4011. */
    async end(graceMs = 50) {
        this.onclosing?.();
        await new Promise((r) => setTimeout(r, graceMs));
        for (const c of [...this.#conns])
            c.close(4011, "closed");
    }
}
export function memoryRelay(publicUrl) {
    return new MemoryRelay(publicUrl);
}
