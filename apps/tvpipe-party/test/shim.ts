// Test doubles of the Worker's bindings: D1 on node:sqlite, a counting
// rate limiter and a static-files fetcher.

import { readFileSync } from "node:fs";
import { DatabaseSync, type SQLInputValue } from "node:sqlite";
import type { D1Database, D1PreparedStatement, D1Result, Env, Fetcher, RateLimit } from "../src/env.ts";

const MIGRATION = new URL("../migrations/0001_links.sql", import.meta.url);

class Statement implements D1PreparedStatement {
  db: DatabaseSync;
  sql: string;
  values: SQLInputValue[];
  constructor(db: DatabaseSync, sql: string, values: SQLInputValue[] = []) {
    this.db = db;
    this.sql = sql;
    this.values = values;
  }
  bind(...values: unknown[]): D1PreparedStatement {
    // D1 takes JS values; undefined is an error there too.
    for (const v of values) if (v === undefined) throw new TypeError("D1_TYPE_ERROR: undefined");
    return new Statement(this.db, this.sql, values as SQLInputValue[]);
  }
  async first<T>(): Promise<T | null> {
    const row = this.db.prepare(this.sql).get(...this.values);
    return (row as T | undefined) ?? null;
  }
  async run<T>(): Promise<D1Result<T>> {
    const stmt = this.db.prepare(this.sql);
    if (/\bRETURNING\b/i.test(this.sql) || /^\s*SELECT/i.test(this.sql)) {
      const results = stmt.all(...this.values) as T[];
      return { results, success: true, meta: { changes: results.length } };
    }
    const info = stmt.run(...this.values);
    return { results: [], success: true, meta: { changes: Number(info.changes) } };
  }
  async all<T>(): Promise<D1Result<T>> {
    const results = this.db.prepare(this.sql).all(...this.values) as T[];
    return { results, success: true, meta: { changes: 0 } };
  }
}

export class MemoryD1 implements D1Database {
  sqlite = new DatabaseSync(":memory:");
  constructor() {
    this.sqlite.exec(readFileSync(MIGRATION, "utf8"));
  }
  prepare(query: string): D1PreparedStatement {
    return new Statement(this.sqlite, query);
  }
}

export class FakeLimiter implements RateLimit {
  limitPerKey: number;
  counts = new Map<string, number>();
  constructor(limitPerKey: number) {
    this.limitPerKey = limitPerKey;
  }
  async limit({ key }: { key: string }): Promise<{ success: boolean }> {
    const n = (this.counts.get(key) ?? 0) + 1;
    this.counts.set(key, n);
    return { success: n <= this.limitPerKey };
  }
  reset(): void {
    this.counts.clear();
  }
}

export class FakeAssets implements Fetcher {
  files: Record<string, string>;
  constructor(files: Record<string, string>) {
    this.files = files;
  }
  async fetch(request: Request): Promise<Response> {
    const path = new URL(request.url).pathname;
    const body = this.files[path];
    if (body === undefined) return new Response("not found", { status: 404 });
    return new Response(body, { status: 200, headers: { "Content-Type": "text/html" } });
  }
}

export interface TestEnv extends Env {
  DB: MemoryD1;
  LOOKUPS: FakeLimiter;
  CREATES: FakeLimiter;
  UPDATES: FakeLimiter;
}

export function testEnv(): TestEnv {
  return {
    DB: new MemoryD1(),
    ASSETS: new FakeAssets({ "/": "<h1>tvpipe</h1>", "/index.html": "<h1>tvpipe</h1>" }),
    LOOKUPS: new FakeLimiter(20),
    CREATES: new FakeLimiter(10),
    UPDATES: new FakeLimiter(60),
  };
}
