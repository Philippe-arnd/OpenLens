/**
 * OpenLens browser SDK.
 *
 * One call (`init`) and everything a PM needs is captured: page views,
 * clicks, form submits and SPA routes, sessions, errors, Web Vitals, failed
 * requests, plus environment and release. `identify` adds the user's role.
 * No cookies. Input values are never captured.
 */

import { detectEnvironment } from "./env";
import { trackVitals } from "./vitals";
import type { Bridge } from "./replay";

declare global {
  interface Window {
    __openlensReplay?: Bridge;
  }
}

export type Props = Record<string, unknown>;

export interface Config {
  /** OpenLens server URL, e.g. https://lens.example.com */
  host: string;
  /** Project capture key (pk_…). */
  key: string;
  /** Release of the app (version or commit). */
  release?: string;
  /** Override the detected environment. */
  environment?: string;
  /** Capture page views, clicks, submits, errors, vitals and failed requests (default true). */
  autocapture?: boolean;
  /** "pending" queues events until consent(true); "denied" captures nothing. Default "granted". */
  consent?: "granted" | "pending" | "denied";
  /** Record sessions for replay, inputs masked (default true). Loads `/r.js` lazily. */
  replay?: boolean;
}

export interface Dimensions {
  role?: string;
  account?: string;
  plan?: string;
}

interface State {
  anon: string;
  uid?: string;
  dims: Dimensions;
  sid: string;
  last: number;
}

const VERSION = "0.1.0";
const SESSION_GAP = 30 * 60 * 1000;
const MAX_QUEUE = 500;

let cfg: Config | undefined;
let state: State;
let queue: Props[] = [];
let consent: Config["consent"] = "granted";
let timer: ReturnType<typeof setInterval> | undefined;
let lastPath = "";
let activeFlags: string[] = [];

function uuid(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) return crypto.randomUUID();
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    return (c === "x" ? r : (r & 3) | 8).toString(16);
  });
}

function storageKey(): string {
  return `openlens_${cfg?.key ?? ""}`;
}

function load(): State {
  try {
    const raw = localStorage.getItem(storageKey());
    if (raw) return JSON.parse(raw) as State;
  } catch {
    // Storage blocked: fall back to memory only.
  }
  return { anon: uuid(), dims: {}, sid: uuid(), last: 0 };
}

function save(): void {
  try {
    localStorage.setItem(storageKey(), JSON.stringify(state));
  } catch {
    // Storage blocked or full: keep going in memory.
  }
}

/** New session after 30 min of inactivity or when the UTC day changes. */
function touchSession(now: number): void {
  const dayChanged = new Date(now).toISOString().slice(0, 10) !== new Date(state.last).toISOString().slice(0, 10);
  if (!state.last || now - state.last > SESSION_GAP || dayChanged) state.sid = uuid();
  state.last = now;
}

function base(): Props {
  const p: Props = {
    $current_url: location.href.split("#")[0],
    $pathname: location.pathname,
    $environment: cfg?.environment || detectEnvironment(location.hostname),
    $lib: "web",
    $lib_version: VERSION,
  };
  if (cfg?.release) p.$release = cfg.release;
  if (state.dims.role) p.$role = state.dims.role;
  if (state.dims.account) p.$account = state.dims.account;
  if (state.dims.plan) p.$plan = state.dims.plan;
  if (activeFlags.length) p.$flags = activeFlags.join(",");
  return p;
}

/** Record an event. */
export function track(event: string, props?: Props): void {
  if (!cfg || consent === "denied") return;
  const now = Date.now();
  touchSession(now);
  save();
  queue.push({
    event,
    distinct_id: state.uid ?? state.anon,
    session_id: state.sid,
    uuid: uuid(),
    timestamp: new Date(now).toISOString(),
    properties: { ...base(), ...props },
  });
  if (queue.length > MAX_QUEUE) queue = queue.slice(-MAX_QUEUE);
  if (queue.length >= 20) flush();
}

/** Mark a use of a feature (a task start), for features without a data-feature control. */
export function feature(key: string, props?: Props): void {
  track("$feature", { ...props, $feature: key });
}

/** Fetch the flags that are on for the current user. */
function loadFlags(): void {
  if (!cfg || !origFetch) return;
  const q = new URLSearchParams({ key: cfg.key, distinct_id: state.uid ?? state.anon });
  if (state.dims.role) q.set("role", state.dims.role);
  if (state.dims.account) q.set("account", state.dims.account);
  origFetch(`${cfg.host.replace(/\/$/, "")}/flags?${q}`)
    .then((r) => (r.ok ? r.json() : null))
    .then((j: { flags?: string[] } | null) => {
      if (j?.flags) {
        activeFlags = j.flags;
        try {
          localStorage.setItem(`${storageKey()}_flags`, JSON.stringify(activeFlags));
        } catch {
          // memory only
        }
      }
    })
    .catch(() => undefined);
}

/** Is a feature flag on for this user? Uses the last known answer (cached across page loads). */
export function isEnabled(key: string): boolean {
  return activeFlags.includes(key);
}

/** Identify the logged-in user and the dimensions OpenLens slices by. */
export function identify(id: string, dims: Dimensions = {}): void {
  if (!cfg || !id) return;
  const anon = state.uid ? undefined : state.anon;
  state.uid = String(id);
  state.dims = { ...state.dims, ...dims };
  save();
  track("$identify", anon ? { $anon_distinct_id: anon } : {});
  loadFlags();
}

/** Forget the user (call on logout). */
export function reset(): void {
  if (!cfg) return;
  flush();
  state = { anon: uuid(), dims: {}, sid: uuid(), last: 0 };
  save();
}

/** Grant or withdraw consent at runtime. */
export function setConsent(granted: boolean): void {
  consent = granted ? "granted" : "denied";
  if (!granted) queue = [];
  else {
    flush();
    if (cfg?.replay !== false) startReplay();
  }
}

/** Send queued events now. */
export function flush(beacon = false): void {
  if (!cfg || consent !== "granted" || queue.length === 0) return;
  const body = JSON.stringify({ sent_at: new Date().toISOString(), batch: queue });
  queue = [];
  const url = `${cfg.host.replace(/\/$/, "")}/capture?key=${encodeURIComponent(cfg.key)}`;
  // text/plain keeps this a "simple" request: no CORS preflight.
  if (beacon && navigator.sendBeacon) {
    navigator.sendBeacon(url, new Blob([body], { type: "text/plain" }));
    return;
  }
  origFetch?.(url, { method: "POST", body, keepalive: true, headers: { "content-type": "text/plain" } }).catch(
    () => undefined,
  );
}

let origFetch: typeof fetch | undefined;

// Deepest scroll on the current page, as % of the document height.
let maxScroll = 0;
function measureScroll(): void {
  const doc = document.documentElement;
  const h = Math.max(doc.scrollHeight - window.innerHeight, 1);
  maxScroll = Math.max(maxScroll, Math.min(100, Math.round(((window.scrollY || doc.scrollTop) / h) * 100)));
}
function reportScroll(): void {
  if (lastPath && maxScroll >= 0) track("$scroll", { $depth: document.documentElement.scrollHeight <= window.innerHeight ? 100 : maxScroll, $pathname: lastPath });
  maxScroll = 0;
}

function pageview(): void {
  if (location.pathname === lastPath) return;
  if (lastPath) reportScroll();
  const first = lastPath === "";
  lastPath = location.pathname;
  track("$pageview", first && document.referrer ? { $referrer: document.referrer } : {});
}

function describe(el: Element): Props {
  const p: Props = { $tag: el.tagName.toLowerCase() };
  const f = el.closest("[data-feature]");
  if (f) p.$feature = f.getAttribute("data-feature");
  if (el.id) p.$id = el.id;
  const isInput = el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement;
  if (!isInput) {
    const text = (el.textContent || "").replace(/\s+/g, " ").trim();
    if (text) p.$text = text.slice(0, 60);
  }
  const href = el.getAttribute("href");
  if (href) p.$href = href.split("?")[0]?.slice(0, 200);
  return p;
}

const CLICKABLE = "a,button,[role=button],input[type=submit],input[type=button],summary,label,[data-feature]";
let rage: { el: Element | null; n: number; t: number } = { el: null, n: 0, t: 0 };

function autocapture(): void {
  pageview();
  for (const m of ["pushState", "replaceState"] as const) {
    const orig = history[m];
    history[m] = function (this: History, ...args: Parameters<History["pushState"]>) {
      const r = orig.apply(this, args);
      setTimeout(pageview, 0);
      return r;
    };
  }
  addEventListener("popstate", pageview);
  addEventListener("scroll", measureScroll, { passive: true });
  addEventListener("visibilitychange", () => document.visibilityState === "hidden" && reportScroll(), true);

  document.addEventListener(
    "click",
    (e) => {
      const target = e.target instanceof Element ? e.target.closest(CLICKABLE) : null;
      if (!target || target.closest("[data-openlens-ignore]")) return;
      const now = Date.now();
      rage = rage.el === target && now - rage.t < 1000 ? { el: target, n: rage.n + 1, t: now } : { el: target, n: 1, t: now };
      track("$click", { ...describe(target), ...(rage.n >= 3 ? { $rage: true } : {}) });
    },
    true,
  );
  document.addEventListener(
    "submit",
    (e) => {
      const form = e.target instanceof HTMLFormElement ? e.target : null;
      if (!form || form.closest("[data-openlens-ignore]")) return;
      track("$submit", { ...describe(form), $form: form.getAttribute("name") || form.id || undefined });
    },
    true,
  );

  // Errors: at most 5 of the same message per page.
  const seen: Record<string, number> = {};
  const report = (kind: string, message: string, stack?: string, source?: string) => {
    const k = message.slice(0, 200);
    seen[k] = (seen[k] ?? 0) + 1;
    if (seen[k]! > 5) return;
    track("$error", { $kind: kind, $message: message.slice(0, 500), $stack: stack?.slice(0, 4000), $source: source });
  };
  addEventListener("error", (e) => {
    if (e instanceof ErrorEvent) report("error", e.message || "error", e.error?.stack, `${e.filename}:${e.lineno}:${e.colno}`);
  });
  addEventListener("unhandledrejection", (e) => {
    const r = e.reason;
    report("unhandledrejection", r instanceof Error ? r.message : String(r), r instanceof Error ? r.stack : undefined);
  });

  trackVitals((name, value, rating) => track("$vital", { $name: name, $value: value, $rating: rating }));

  // Failed requests: path and status only, never bodies. Our own calls are skipped.
  const own = (u: string) => !!cfg && u.startsWith(cfg.host);
  const failed = (method: string, rawUrl: string, status: number) => {
    let path = rawUrl;
    try {
      const u = new URL(rawUrl, location.href);
      path = u.host === location.host ? u.pathname : u.host + u.pathname;
    } catch {
      // keep the raw value
    }
    track("$request_failed", { $method: method.toUpperCase(), $path: path.slice(0, 300), $status: status });
  };
  if (origFetch) {
    const f = origFetch;
    window.fetch = function (input: RequestInfo | URL, init?: RequestInit) {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const method = init?.method || (input instanceof Request ? input.method : "GET");
      const p = f.call(window, input, init);
      if (!own(url)) {
        p.then(
          (r) => r.status >= 500 && failed(method, url, r.status),
          () => failed(method, url, 0),
        );
      }
      return p;
    } as typeof fetch;
  }
  const open = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (this: XMLHttpRequest, method: string, url: string | URL, ...rest: unknown[]) {
    const u = String(url);
    if (!own(u)) {
      this.addEventListener("loadend", () => {
        if (this.status >= 500 || this.status === 0) failed(method, u, this.status);
      });
    }
    return (open as (...a: unknown[]) => void).call(this, method, url, ...rest);
  } as typeof XMLHttpRequest.prototype.open;
}

/** Start OpenLens. Safe to call once; later calls are ignored. */
export function init(config: Config): void {
  if (cfg || typeof window === "undefined") return;
  if (!config.host || !config.key) return;
  if (navigator.doNotTrack === "1" || config.consent === "denied") consent = "denied";
  else consent = config.consent ?? "granted";
  cfg = config;
  origFetch = window.fetch?.bind(window);
  state = load();
  try {
    activeFlags = JSON.parse(localStorage.getItem(`${storageKey()}_flags`) || "[]") as string[];
  } catch {
    activeFlags = [];
  }
  if (consent === "denied") return;
  loadFlags();
  if (config.autocapture !== false) autocapture();
  if (config.replay !== false && consent === "granted") startReplay();
  timer = setInterval(() => flush(), 5000);
  addEventListener("visibilitychange", () => document.visibilityState === "hidden" && flush(true));
  addEventListener("pagehide", () => flush(true));
}

/** Load the replay recorder once the page is idle (it is a separate, larger bundle). */
function startReplay(): void {
  if (!cfg || window.__openlensReplay) return;
  const c = cfg;
  window.__openlensReplay = {
    host: c.host,
    key: c.key,
    ids: () => ({ sid: state.sid, uid: state.uid ?? state.anon }),
    allowed: () => consent === "granted",
  };
  const load = () => {
    const s = document.createElement("script");
    s.src = `${c.host.replace(/\/$/, "")}/r.js`;
    s.async = true;
    document.head.appendChild(s);
  };
  const idle = (window as { requestIdleCallback?: (f: () => void) => void }).requestIdleCallback;
  if (document.readyState === "complete") (idle ?? setTimeout)(load);
  else addEventListener("load", () => (idle ?? setTimeout)(load));
}

/** Stop timers (tests, single-page teardown). */
export function shutdown(): void {
  if (timer) clearInterval(timer);
  flush(true);
}

export default { init, identify, track, feature, isEnabled, reset, flush, setConsent, shutdown };
