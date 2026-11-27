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
}
/** Request context the middleware puts on `req.openlens`. */
export interface RequestContext { traceId?: string; sessionId?: string }
export interface ExceptionContext extends RequestContext {
  distinctId?: string;
  route?: string;
  method?: string;
  properties?: Record<string, unknown>;
}
export interface RequestRecord { method: string; route: string; status: number; ms: number }
export declare const SERVER_ID: "$server";
export declare const STATS_WINDOW_MS: number;
export declare const BOUNDS_S: number[];
export declare const SESSION_HEADER: "x-openlens-session";
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
  /** Count one request in its route's 10-minute stats (any framework); `ms` in milliseconds. */
  recordRequest(r: RequestRecord): void;
  /** Runtime health: one `$runtime_stats` at the process start, then one per 10-minute window
   * (event-loop delay p50/p99, largest RSS and heap used). On with `express()`. */
  captureRuntime(): void;
  /** Express middleware: route stats, runtime health and `req.openlens`. `user` reads the signed-in user's id. */
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
export declare function appFrame(stack?: string): string | undefined;
export declare function detectRelease(env: Record<string, string | undefined>): string | undefined;
export default OpenLens;
