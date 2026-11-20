// The few Cloudflare types the Worker uses, declared here instead of
// depending on @cloudflare/workers-types. They follow the runtime's API.

export interface D1Meta {
  changes: number;
  [key: string]: unknown;
}

export interface D1Result<T = Record<string, unknown>> {
  results: T[];
  success: boolean;
  meta: D1Meta;
}

export interface D1PreparedStatement {
  bind(...values: unknown[]): D1PreparedStatement;
  first<T = Record<string, unknown>>(): Promise<T | null>;
  run<T = Record<string, unknown>>(): Promise<D1Result<T>>;
  all<T = Record<string, unknown>>(): Promise<D1Result<T>>;
}

export interface D1Database {
  prepare(query: string): D1PreparedStatement;
}

/** The `ratelimits` binding. */
export interface RateLimit {
  limit(options: { key: string }): Promise<{ success: boolean }>;
}

/** The `assets` binding. */
export interface Fetcher {
  fetch(request: Request): Promise<Response>;
}

export interface Env {
  DB: D1Database;
  ASSETS: Fetcher;
  /** Lookups of codes: 20 a minute per client. */
  LOOKUPS: RateLimit;
  /** POST /api/links: 10 a minute per client. */
  CREATES: RateLimit;
  /** PUT/DELETE /api/links/CODE: 60 a minute per client. */
  UPDATES: RateLimit;
}
