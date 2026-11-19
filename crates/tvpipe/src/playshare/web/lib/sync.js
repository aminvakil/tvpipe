/**
 * Views and their diffs (docs/party-design.md §5 "State sync").
 *
 * A view is a JSON value. A diff from `a` to `b` is `{ set, del }`:
 * `set` is merged into the old view (objects merge key by key; anything
 * else, `null` and arrays included, replaces), then the key paths in
 * `del` are removed. The client's `applyDiff` (client/party-client.js)
 * is the inverse; the tests check that the two agree.
 */
export function isObject(v) {
    return typeof v === "object" && v !== null && !Array.isArray(v);
}
/**
 * A JSON copy of `v`: what the client will see (undefined fields gone,
 * dates as strings). Throws on values JSON cannot carry (cycles, BigInt).
 */
export function toJson(v) {
    if (v === undefined)
        return null;
    return JSON.parse(JSON.stringify(v));
}
export function deepEqual(a, b) {
    if (a === b)
        return true;
    if (typeof a !== typeof b || a === null || b === null || typeof a !== "object")
        return false;
    if (Array.isArray(a)) {
        if (!Array.isArray(b) || a.length !== b.length)
            return false;
        for (let i = 0; i < a.length; i++)
            if (!deepEqual(a[i], b[i]))
                return false;
        return true;
    }
    if (Array.isArray(b))
        return false;
    const ak = Object.keys(a);
    const bo = b;
    if (ak.length !== Object.keys(bo).length)
        return false;
    for (const k of ak) {
        if (!Object.hasOwn(bo, k) || !deepEqual(a[k], bo[k]))
            return false;
    }
    return true;
}
/** The diff from object `a` to object `b`, or `undefined` when they are equal. */
export function diff(a, b) {
    const set = {};
    const del = [];
    walk(a, b, [], set, del);
    const out = {};
    if (Object.keys(set).length > 0)
        out.set = set;
    if (del.length > 0)
        out.del = del;
    return out.set || out.del ? out : undefined;
}
/**
 * The diff from `a` to `b` as it appears inside a bigger view at `path`
 * (`del` paths start with `path`; `set` is relative to it). A view's
 * diff is its sections' diffs in key order (`party.ts` composes them).
 */
export function diffAt(a, b, path) {
    const set = {};
    const del = [];
    walk(a, b, path, set, del);
    return { set, del };
}
function walk(a, b, path, set, del) {
    for (const k of Object.keys(a)) {
        if (!Object.hasOwn(b, k))
            del.push([...path, k]);
    }
    for (const k of Object.keys(b)) {
        const nv = b[k];
        if (!Object.hasOwn(a, k)) {
            set[k] = nv;
            continue;
        }
        const ov = a[k];
        if (isObject(ov) && isObject(nv)) {
            const sub = {};
            walk(ov, nv, [...path, k], sub, del);
            if (Object.keys(sub).length > 0)
                set[k] = sub;
        }
        else if (!deepEqual(ov, nv)) {
            set[k] = nv;
        }
    }
}
/** Applies a diff to a copy of `view` (the same rules as the client's). */
export function applyDiff(view, d) {
    const out = toJson(view);
    if (d.set)
        merge(out, d.set);
    for (const path of d.del ?? []) {
        let o = out;
        for (let i = 0; i < path.length - 1 && isObject(o); i++)
            o = o[path[i]];
        if (isObject(o))
            delete o[path[path.length - 1]];
    }
    return out;
}
function merge(target, patch) {
    for (const k of Object.keys(patch)) {
        const pv = patch[k];
        const tv = target[k];
        if (isObject(pv) && isObject(tv))
            merge(tv, pv);
        else
            target[k] = toJson(pv);
    }
}
/**
 * A piece of state the host sets and clients see (`party.room`,
 * `party.screen`, `player.state`). Changes in one tick are sent as one
 * diff.
 */
export class StateBox {
    #value = {};
    #version = 0;
    #snapshot;
    #changed;
    constructor(changed) {
        this.#changed = changed;
    }
    /** The current state (a JSON copy; changing it changes nothing). */
    get value() {
        return toJson(this.#value);
    }
    /** The state without copying: do not modify. */
    peek() {
        return this.#value;
    }
    /** Counts the changes (each `set`, `replace`, `update`). */
    get version() {
        return this.#version;
    }
    /** A copy of the state as of this version, made once per version: do not modify. */
    snapshot() {
        if (this.#snapshot?.version !== this.#version)
            this.#snapshot = { version: this.#version, value: toJson(this.#value) };
        return this.#snapshot.value;
    }
    /** Merges the top-level fields of `partial` into the state (`undefined` removes one). */
    set(partial) {
        const j = toJson(partial);
        if (!isObject(j))
            throw new TypeError("state must be an object");
        Object.assign(this.#value, j);
        for (const [k, v] of Object.entries(partial))
            if (v === undefined)
                delete this.#value[k];
        this.#version++;
        this.#changed();
    }
    /** Replaces the whole state. */
    replace(value) {
        const j = toJson(value);
        if (!isObject(j))
            throw new TypeError("state must be an object");
        this.#value = j;
        this.#version++;
        this.#changed();
    }
    /** Changes a copy of the state in place, then takes it. */
    update(fn) {
        const draft = toJson(this.#value);
        fn(draft);
        this.replace(draft);
    }
}
