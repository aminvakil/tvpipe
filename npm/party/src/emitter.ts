/** A small typed event emitter (the browser has no `node:events`). */

export type Listener<A extends unknown[]> = (...args: A) => void;

export class Emitter<E extends { [K in keyof E]: unknown[] }> {
  #listeners = new Map<keyof E, Set<Listener<any>>>();

  /** Calls `fn` on every `event`; returns a function that stops it. */
  on<K extends keyof E>(event: K, fn: Listener<E[K]>): () => void {
    let set = this.#listeners.get(event);
    if (!set) this.#listeners.set(event, (set = new Set()));
    set.add(fn);
    return () => this.off(event, fn);
  }

  /** Calls `fn` on the next `event` only. */
  once<K extends keyof E>(event: K, fn: Listener<E[K]>): () => void {
    const off = this.on(event, ((...args: E[K]) => {
      off();
      fn(...args);
    }) as Listener<E[K]>);
    return off;
  }

  off<K extends keyof E>(event: K, fn: Listener<E[K]>): void {
    this.#listeners.get(event)?.delete(fn);
  }

  listenerCount<K extends keyof E>(event: K): number {
    return this.#listeners.get(event)?.size ?? 0;
  }

  /** Calls the listeners; one that throws is reported and the others still run. */
  emit<K extends keyof E>(event: K, ...args: E[K]): boolean {
    const set = this.#listeners.get(event);
    if (!set || set.size === 0) return false;
    for (const fn of [...set]) {
      try {
        fn(...args);
      } catch (err) {
        reportError(err);
      }
    }
    return true;
  }
}

function reportError(err: unknown): void {
  const g = globalThis as { reportError?: (e: unknown) => void };
  if (typeof g.reportError === "function") g.reportError(err);
  else console.error(err);
}
