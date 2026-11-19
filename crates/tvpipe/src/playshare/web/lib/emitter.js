/** A small typed event emitter (the browser has no `node:events`). */
export class Emitter {
    #listeners = new Map();
    /** Calls `fn` on every `event`; returns a function that stops it. */
    on(event, fn) {
        let set = this.#listeners.get(event);
        if (!set)
            this.#listeners.set(event, (set = new Set()));
        set.add(fn);
        return () => this.off(event, fn);
    }
    /** Calls `fn` on the next `event` only. */
    once(event, fn) {
        const off = this.on(event, ((...args) => {
            off();
            fn(...args);
        }));
        return off;
    }
    off(event, fn) {
        this.#listeners.get(event)?.delete(fn);
    }
    listenerCount(event) {
        return this.#listeners.get(event)?.size ?? 0;
    }
    /** Calls the listeners; one that throws is reported and the others still run. */
    emit(event, ...args) {
        const set = this.#listeners.get(event);
        if (!set || set.size === 0)
            return false;
        for (const fn of [...set]) {
            try {
                fn(...args);
            }
            catch (err) {
                reportError(err);
            }
        }
        return true;
    }
}
function reportError(err) {
    const g = globalThis;
    if (typeof g.reportError === "function")
        g.reportError(err);
    else
        console.error(err);
}
