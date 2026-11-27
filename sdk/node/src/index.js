// OpenLens server SDK for Node (>= 18): a thin client over the HTTP API.
// Events are queued and sent in batches (every `flushInterval` ms or
// `flushAt` events). Errors never throw into the app: they go to `onError`.

/** @typedef {{ role?: string, account?: string, plan?: string }} Dimensions */

/** Distinct id of server events tied to no user: left out of user counts (#593). */
export const SERVER_ID = "$server";
/** Length of one request-stats window (#593, decision 1): one event per
 * (method, route, status class) per window, never one per request. */
export const STATS_WINDOW_MS = 10 * 60 * 1000;
/** Upper bounds (ms) of the request duration histogram; one more bucket holds the rest. */
export const BUCKETS_MS = [5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000];
/** Distinct (method, route, status class) keys kept per window: a bound on memory. */
const MAX_STATS_KEYS = 500;
/** Same error message sent at most this many times per minute. */
const MAX_SAME_ERROR = 5;

/** Variables hosts set to the deployed commit, in order of preference (#495). */
const COMMIT_VARS = [
  "SOURCE_COMMIT",
  "GITHUB_SHA",
  "VERCEL_GIT_COMMIT_SHA",
  "CF_PAGES_COMMIT_SHA",
  "RENDER_GIT_COMMIT",
  "HEROKU_SLUG_COMMIT",
];

/**
 * The release the host exposes: the deployed commit (shortened to 12
 * characters when it is a full hash), else the package version npm or a
 * script runner sets. `undefined` when the host says nothing.
 * @param {Record<string, string | undefined>} env
 * @returns {string | undefined}
 */
export function detectRelease(env) {
  for (const name of COMMIT_VARS) {
    const v = (env[name] || "").trim();
    if (v) return /^[0-9a-f]{13,64}$/i.test(v) ? v.slice(0, 12).toLowerCase() : v.slice(0, 200);
  }
  const version = (env.npm_package_version || "").trim();
  return version ? version.slice(0, 200) : undefined;
}

export class OpenLens {
  /**
   * @param {{ host: string, key: string, release?: string, environment?: string,
   *   flushAt?: number, flushInterval?: number, onError?: (e: unknown) => void,
   *   fetch?: typeof fetch }} opts
   */
  constructor(opts) {
    if (!opts || !opts.host || !opts.key) throw new Error("OpenLens: host and key are required");
    this.host = opts.host.replace(/\/$/, "");
    this.key = opts.key;
    // An explicit release wins; else what the host exposes (#495).
    this.release = opts.release || detectRelease(process.env);
    this.environment = opts.environment ?? process.env.NODE_ENV ?? "production";
    this.flushAt = opts.flushAt ?? 50;
    this.onError = opts.onError ?? (() => {});
    this.fetch = opts.fetch ?? globalThis.fetch;
    this.queue = [];
    /** @type {Map<string, any>} */
    this.stats = new Map();
    this.seenErrors = new Map();
    this.reported = new WeakSet();
    this.seenMinute = 0;
    this.timer = setInterval(() => void this.flush(), opts.flushInterval ?? 5000);
    this.timer.unref?.();
  }

  /** Record an event for a user. */
  capture(distinctId, event, properties = {}) {
    if (!distinctId || !event) return;
    this.push(event, distinctId, properties);
  }

  /** Mark a use of a feature (a task start), as `openlens.feature()` does in the browser. */
  feature(distinctId, key, properties = {}) {
    this.capture(distinctId, "$feature", { ...properties, $feature: key });
  }

  /** Set the user's role (and account, plan): what OpenLens slices every KPI by. */
  identify(distinctId, dims = {}) {
    const p = {};
    if (dims.role) p.$role = dims.role;
    if (dims.account) p.$account = dims.account;
    if (dims.plan) p.$plan = dims.plan;
    this.capture(distinctId, "$identify", p);
  }

  /** Is a flag on for this user? Resolves to false on any error.
   * `dims`: { role, account, plan, properties: { seats: 12, … } } for rules. */
  async isEnabled(flag, distinctId, dims = {}) {
    try {
      const q = new URLSearchParams({ key: this.key, distinct_id: String(distinctId) });
      if (dims.role) q.set("role", dims.role);
      if (dims.account) q.set("account", dims.account);
      if (dims.plan) q.set("plan", dims.plan);
      for (const [k, v] of Object.entries(dims.properties || {}).slice(0, 20)) {
        if (v !== undefined && v !== null && v !== "") q.set(`p.${k.toLowerCase()}`, String(v));
      }
      const r = await this.fetch(`${this.host}/flags?${q}`);
      if (!r.ok) return false;
      const j = await r.json();
      return Array.isArray(j.flags) && j.flags.includes(flag);
    } catch (e) {
      this.onError(e);
      return false;
    }
  }

  /**
   * Report a server exception. It is sent as `$error` with `$kind: "server"`,
   * so it is grouped, alerted on and judged per release like a browser error.
   * `ctx` comes from `req.openlens` (the middleware) or is given by hand.
   * @param {unknown} err
   * @param {{ distinctId?: string, sessionId?: string, traceId?: string, route?: string,
   *   method?: string, properties?: Record<string, unknown> }} [ctx]
   */
  captureException(err, ctx = {}) {
    try {
      if (err && typeof err === "object") {
        if (this.reported.has(err)) return;
        this.reported.add(err);
      }
      const e = err instanceof Error ? err : new Error(typeof err === "string" ? err : safeString(err));
      const message = `${e.name || "Error"}: ${e.message || ""}`.slice(0, 500);
      if (!this.allowError(message)) return;
      const stack = typeof e.stack === "string" ? e.stack.slice(0, 4000) : undefined;
      const props = { ...(ctx.properties || {}), $kind: "server", $message: message };
      if (stack) props.$stack = stack;
      const source = appFrame(stack);
      if (source) props.$source = source;
      if (ctx.route) props.$route = String(ctx.route).slice(0, 300);
      if (ctx.method) props.$method = String(ctx.method).toUpperCase().slice(0, 10);
      if (ctx.traceId) props.$trace_id = ctx.traceId;
      this.push("$error", ctx.distinctId || SERVER_ID, props, ctx.sessionId);
    } catch (e) {
      this.onError(e);
    }
  }

  /** Report uncaught exceptions and unhandled rejections, then let the
   * process fail as it would have. Opt-in. A crash exits at once, so its
   * event is sent only if the flush wins the race (best effort); a
   * rejection is flushed before the process fails. */
  captureUncaught() {
    process.on("uncaughtExceptionMonitor", (err) => {
      this.captureException(err);
      void this.flush();
    });
    process.on("unhandledRejection", (reason) => {
      this.captureException(reason);
      void this.flush().finally(() => {
        // Node's default for an unhandled rejection (since v15) is to crash.
        if (process.listenerCount("unhandledRejection") === 1) {
          setImmediate(() => {
            throw reason instanceof Error ? reason : new Error(safeString(reason));
          });
        }
      });
    });
  }

  /**
   * Count one request in its route's stats (any framework). `route` is the
   * route template (`/api/entries/:id`), never the raw URL.
   * @param {{ method: string, route: string, status: number, ms: number }} r
   */
  recordRequest(r) {
    try {
      if (!r || !r.route) return;
      const now = Date.now();
      const start = now - (now % STATS_WINDOW_MS);
      const method = String(r.method || "GET").toUpperCase().slice(0, 10);
      const route = String(r.route).slice(0, 300);
      const status = Number(r.status) || 0;
      const cls = status >= 100 && status < 600 ? `${Math.floor(status / 100)}xx` : "0xx";
      const key = `${start} ${method} ${route} ${cls}`;
      let s = this.stats.get(key);
      if (!s) {
        if (this.stats.size >= MAX_STATS_KEYS) return;
        s = { start, method, route, cls, count: 0, errors: 0, sum: 0, max: 0, buckets: new Array(BUCKETS_MS.length + 1).fill(0) };
        this.stats.set(key, s);
      }
      const ms = Math.max(0, Number(r.ms) || 0);
      s.count += 1;
      if (status >= 500) s.errors += 1;
      s.sum += ms;
      if (ms > s.max) s.max = ms;
      let i = BUCKETS_MS.findIndex((b) => ms <= b);
      if (i < 0) i = BUCKETS_MS.length;
      s.buckets[i] += 1;
    } catch (e) {
      this.onError(e);
    }
  }

  /**
   * Express (4 and 5) middleware: route stats for every request, and
   * `req.openlens` = { traceId, sessionId, distinctId } read from the
   * `traceparent` and `baggage` headers the browser SDK adds. Mount it
   * first; mount `expressErrors()` last.
   */
  express(opts = {}) {
    return (req, res, next) => {
      const t0 = process.hrtime.bigint();
      try {
        req.openlens = requestContext(req.headers || {});
      } catch {
        req.openlens = {};
      }
      // The signed-in user, when the app says how to read it, wins over
      // the id the browser sent (read when the error happens: auth runs later).
      if (typeof opts.user === "function") req.openlens.user = () => safeCall(opts.user, req);
      res.on("finish", () => {
        const ms = Number(process.hrtime.bigint() - t0) / 1e6;
        this.recordRequest({ method: req.method, route: routeOf(req), status: res.statusCode, ms });
      });
      next();
    };
  }

  /** Express error handler: reports the error with its route and request
   * context, then hands it on (`next(err)`). Mount it after the routes. */
  expressErrors() {
    return (err, req, _res, next) => {
      const ctx = req.openlens || {};
      const user = ctx.user ? ctx.user() : undefined;
      this.captureException(err, { ...ctx, distinctId: user || ctx.distinctId, route: routeOf(req), method: req.method });
      next(err);
    };
  }

  /** @private Queue one event. */
  push(event, distinctId, properties, sessionId, timestamp) {
    const props = { $lib: "node", $environment: this.environment, ...properties };
    if (this.release) props.$release = this.release;
    const ev = {
      event,
      distinct_id: String(distinctId),
      uuid: crypto.randomUUID(),
      timestamp: (timestamp || new Date()).toISOString(),
      properties: props,
    };
    if (sessionId) ev.session_id = String(sessionId);
    this.queue.push(ev);
    if (this.queue.length >= this.flushAt) void this.flush();
  }

  /** @private At most MAX_SAME_ERROR of one message per minute. */
  allowError(message) {
    const minute = Math.floor(Date.now() / 60000);
    if (minute !== this.seenMinute) {
      this.seenMinute = minute;
      this.seenErrors.clear();
    }
    const k = message.slice(0, 200);
    const n = (this.seenErrors.get(k) || 0) + 1;
    if (this.seenErrors.size < 1000 || this.seenErrors.has(k)) this.seenErrors.set(k, n);
    return n <= MAX_SAME_ERROR;
  }

  /** @private Turn the stats of closed windows (all of them when `all`) into events. */
  drainStats(all) {
    const open = Date.now() - (Date.now() % STATS_WINDOW_MS);
    for (const [key, s] of this.stats) {
      if (!all && s.start >= open) continue;
      this.stats.delete(key);
      this.push(
        "$request_stats",
        SERVER_ID,
        {
          $method: s.method,
          $route: s.route,
          $status_class: s.cls,
          $count: s.count,
          $errors: s.errors,
          $sum_ms: Math.round(s.sum),
          $max_ms: Math.round(s.max),
          $buckets: s.buckets,
          $window_s: STATS_WINDOW_MS / 1000,
        },
        undefined,
        new Date(s.start),
      );
    }
  }

  /** Send queued events now. */
  async flush(all = false) {
    this.drainStats(all);
    if (this.queue.length === 0) return;
    const batch = this.queue.splice(0, this.queue.length);
    try {
      const r = await this.fetch(`${this.host}/capture`, {
        method: "POST",
        headers: { "content-type": "application/json", "x-openlens-key": this.key },
        body: JSON.stringify({ sent_at: new Date().toISOString(), batch }),
      });
      if (!r.ok) this.onError(new Error(`OpenLens capture failed: HTTP ${r.status}`));
    } catch (e) {
      this.onError(e);
    }
  }

  /** Flush and stop the timer (call on shutdown). */
  async shutdown() {
    clearInterval(this.timer);
    await this.flush(true);
  }
}

/** The route template of an Express request (`/api/entries/:id`), or
 * `(unmatched)`: never the raw URL, which would make one key per id. */
function routeOf(req) {
  const p = req.route && req.route.path;
  if (typeof p !== "string") return "(unmatched)";
  return `${req.baseUrl || ""}${p}` || "/";
}

/** W3C trace context (`traceparent`) and the OpenLens baggage entries. */
export function requestContext(headers) {
  const ctx = {};
  const tp = String(headers.traceparent || "");
  const m = /^[0-9a-f]{2}-([0-9a-f]{32})-[0-9a-f]{16}-[0-9a-f]{2}$/.exec(tp.trim());
  if (m && !/^0+$/.test(m[1])) ctx.traceId = m[1];
  for (const part of String(headers.baggage || "").slice(0, 8192).split(",")) {
    const i = part.indexOf("=");
    if (i < 0) continue;
    const k = part.slice(0, i).trim();
    const v = decodeSafe(part.slice(i + 1).split(";")[0].trim()).slice(0, 200);
    if (!v) continue;
    if (k === "openlens-session") ctx.sessionId = v;
    else if (k === "openlens-distinct") ctx.distinctId = v;
  }
  return ctx;
}

function decodeSafe(v) {
  try {
    return decodeURIComponent(v);
  } catch {
    return "";
  }
}

function safeCall(fn, req) {
  try {
    const v = fn(req);
    return v === undefined || v === null || v === "" ? undefined : String(v).slice(0, 200);
  } catch {
    return undefined;
  }
}

function safeString(v) {
  try {
    return typeof v === "object" ? JSON.stringify(v).slice(0, 500) : String(v);
  } catch {
    return "non-error value";
  }
}

/** First stack frame outside node_modules and Node's own modules, as
 * `file:line:col`: the error's group is its message @ this file. */
export function appFrame(stack) {
  if (!stack) return undefined;
  for (const line of stack.split("\n").slice(1, 50)) {
    const m = /\(?((?:file:\/\/)?[^()\s]+):(\d+):(\d+)\)?\s*$/.exec(line.trim());
    if (!m) continue;
    const file = m[1];
    if (file.includes("node_modules") || file.startsWith("node:") || !file.includes("/")) continue;
    return `${file.replace(/^file:\/\//, "")}:${m[2]}:${m[3]}`;
  }
  return undefined;
}

export default OpenLens;
