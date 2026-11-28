export interface Dimensions { role?: string; account?: string; plan?: string }
export interface Options {
  host: string;
  key: string;
  /** Default: the commit the host exposes (SOURCE_COMMIT, GITHUB_SHA, ...), else npm_package_version. */
  release?: string;
  environment?: string;
  flushAt?: number;
  flushInterval?: number;
  onError?: (e: unknown) => void;
  fetch?: typeof fetch;
  /** Stats window in seconds: 60, 300 or 600 (anything else is ignored). Default: the window the
   * server advertises in `x-openlens-stats-window`, else 600. Set it only to pin one (D-091). */
  statsWindowSeconds?: number;
}
/** Request context the middleware puts on `req.openlens`. */
export interface RequestContext { traceId?: string; sessionId?: string }
export interface ExceptionContext extends RequestContext {
  distinctId?: string;
  route?: string;
  method?: string;
  properties?: Record<string, unknown>;
}
/** One request; `traceId` and `sessionId` are kept if it becomes an exemplar (never a user id). */
export interface RequestRecord { method: string; route: string; status: number; ms: number; traceId?: string; sessionId?: string }
export declare const SERVER_ID: "$server";
/** The default stats window (10 minutes), used until the server advertises another. */
export declare const STATS_WINDOW_MS: number;
/** The capture response header that carries the server's stats window, seconds. */
export declare const STATS_WINDOW_HEADER: "x-openlens-stats-window";
export declare const BOUNDS_S: number[];
/** Length of one runtime-health window: `$runtime_stats` every 10 minutes, whatever the stats window. */
export declare const RUNTIME_WINDOW_MS: number;
/** Health-check route templates never counted as route stats (`/health/*`: any route under `/health/`). */
export declare const HEALTH_ROUTES: readonly string[];
export declare const SESSION_HEADER: "x-openlens-session";
/** Distinct external hosts kept per stats window by `captureOutgoing()`; the rest count as `_OTHER`. */
export declare const MAX_CLIENT_HOSTS: number;
export declare class OpenLens {
  constructor(opts: Options);
  capture(distinctId: string, event: string, properties?: Record<string, unknown>): void;
  feature(distinctId: string, key: string, properties?: Record<string, unknown>): void;
  identify(distinctId: string, dims?: Dimensions): void;
  isEnabled(flag: string, distinctId: string, dims?: Dimensions): Promise<boolean>;
  /** Send a server exception as `$error` (`$kind: "server"`). */
  captureException(err: unknown, ctx?: ExceptionContext): void;
  /** Report uncaught exceptions and unhandled rejections (opt-in). */
  captureUncaught(): void;
  /** Count one request in its route's stats window (any framework); `ms` in milliseconds.
   * Health checks (`HEALTH_ROUTES`) are not counted. */
  recordRequest(r: RequestRecord): void;
  /** Runtime health: one `$runtime_stats` at the process start, then one every 10 minutes
   * (event-loop delay p50/p99, largest RSS and heap used of those 10 minutes). On with `express()`. */
  captureRuntime(): void;
  /** Outgoing calls (global `fetch`, `http`/`https`) per external host: one `$client_stats` per
   * method, host and status per stats window. The host only, never the path or query; calls to
   * the OpenLens host are not counted. Opt-in; `shutdown()` stops it. */
  captureOutgoing(): void;
  /** Express middleware: route stats, runtime health and `req.openlens`. `user` reads the signed-in user's id for server errors, never for exemplars. */
  express(opts?: { user?: (req: any) => string | undefined | null }): (req: any, res: any, next: (err?: unknown) => void) => void;
  /** Express error handler: reports the error, then `next(err)`. */
  expressErrors(): (err: unknown, req: any, res: any, next: (err?: unknown) => void) => void;
  /** Job check-in at the end of a cron or background job; false on any error, never throws. */
  checkIn(slug: string, opts?: { status?: "ok" | "error"; ms?: number; expectEvery?: number }): Promise<boolean>;
  /** Send queued events now; `all` also sends the stats of the open window. */
  flush(all?: boolean): Promise<void>;
  shutdown(): Promise<void>;
}
export declare function requestContext(headers: Record<string, string | string[] | undefined>): RequestContext;
/** Is `route` a health check: one of `HEALTH_ROUTES`, without case, query or trailing slash? */
export declare function isHealthRoute(route: string): boolean;
export declare function appFrame(stack?: string): string | undefined;
export declare function detectRelease(env: Record<string, string | undefined>): string | undefined;
export default OpenLens;
