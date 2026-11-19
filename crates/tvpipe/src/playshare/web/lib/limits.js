/** Rate limits: a token bucket per socket, a sliding counter per address. */
export class TokenBucket {
    rate;
    burst;
    #tokens;
    #last;
    /** `rate` tokens per second, at most `burst` saved up. */
    constructor(rate, burst, now = performance.now()) {
        this.rate = rate;
        this.burst = burst;
        this.#tokens = burst;
        this.#last = now;
    }
    /** Takes one token; `false` when there is none. */
    take(now = performance.now()) {
        this.#tokens = Math.min(this.burst, this.#tokens + ((now - this.#last) / 1000) * this.rate);
        this.#last = now;
        if (this.#tokens < 1)
            return false;
        this.#tokens -= 1;
        return true;
    }
}
/** At most `limit` events per key within `windowMs` (e.g. joins per address). */
export class WindowCounter {
    limit;
    windowMs;
    #hits = new Map();
    constructor(limit, windowMs) {
        this.limit = limit;
        this.windowMs = windowMs;
    }
    /** Counts one event for `key`; `false` when over the limit (then it is not counted). */
    hit(key, now = performance.now()) {
        const since = now - this.windowMs;
        const list = (this.#hits.get(key) ?? []).filter((t) => t > since);
        if (list.length >= this.limit) {
            this.#hits.set(key, list);
            return false;
        }
        list.push(now);
        this.#hits.set(key, list);
        if (this.#hits.size > 10_000)
            this.#prune(since);
        return true;
    }
    /** Whether `key` is over the limit now (without counting). */
    over(key, now = performance.now()) {
        const since = now - this.windowMs;
        return (this.#hits.get(key) ?? []).filter((t) => t > since).length >= this.limit;
    }
    #prune(since) {
        for (const [k, v] of this.#hits) {
            if (v.every((t) => t <= since))
                this.#hits.delete(k);
        }
    }
}
