/** Rate limits: a token bucket per socket, a sliding counter per address. */

export class TokenBucket {
  readonly rate: number;
  readonly burst: number;
  #tokens: number;
  #last: number;

  /** `rate` tokens per second, at most `burst` saved up. */
  constructor(rate: number, burst: number, now: number = performance.now()) {
    this.rate = rate;
    this.burst = burst;
    this.#tokens = burst;
    this.#last = now;
  }

  /** Takes one token; `false` when there is none. */
  take(now: number = performance.now()): boolean {
    this.#tokens = Math.min(this.burst, this.#tokens + ((now - this.#last) / 1000) * this.rate);
    this.#last = now;
    if (this.#tokens < 1) return false;
    this.#tokens -= 1;
    return true;
  }
}

/** At most `limit` events per key within `windowMs` (e.g. joins per address). */
export class WindowCounter {
  readonly limit: number;
  readonly windowMs: number;
  readonly #hits = new Map<string, number[]>();

  constructor(limit: number, windowMs: number) {
    this.limit = limit;
    this.windowMs = windowMs;
  }

  /** Counts one event for `key`; `false` when over the limit (then it is not counted). */
  hit(key: string, now: number = performance.now()): boolean {
    const since = now - this.windowMs;
    const list = (this.#hits.get(key) ?? []).filter((t) => t > since);
    if (list.length >= this.limit) {
      this.#hits.set(key, list);
      return false;
    }
    list.push(now);
    this.#hits.set(key, list);
    if (this.#hits.size > 10_000) this.#prune(since);
    return true;
  }

  /** Whether `key` is over the limit now (without counting). */
  over(key: string, now: number = performance.now()): boolean {
    const since = now - this.windowMs;
    return (this.#hits.get(key) ?? []).filter((t) => t > since).length >= this.limit;
  }

  #prune(since: number): void {
    for (const [k, v] of this.#hits) {
      if (v.every((t) => t <= since)) this.#hits.delete(k);
    }
  }
}
