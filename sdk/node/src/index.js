// OpenLens server SDK for Node (>= 18): a thin client over the HTTP API.
// Events are queued and sent in batches (every `flushInterval` ms or
// `flushAt` events). Errors never throw into the app: they go to `onError`.

/** @typedef {{ role?: string, account?: string, plan?: string }} Dimensions */

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
    this.timer = setInterval(() => void this.flush(), opts.flushInterval ?? 5000);
    this.timer.unref?.();
  }

  /** Record an event for a user. */
  capture(distinctId, event, properties = {}) {
    if (!distinctId || !event) return;
    const props = { $lib: "node", $environment: this.environment, ...properties };
    if (this.release) props.$release = this.release;
    this.queue.push({
      event,
      distinct_id: String(distinctId),
      uuid: crypto.randomUUID(),
      timestamp: new Date().toISOString(),
      properties: props,
    });
    if (this.queue.length >= this.flushAt) void this.flush();
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

  /** Send queued events now. */
  async flush() {
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
    await this.flush();
  }
}

export default OpenLens;
