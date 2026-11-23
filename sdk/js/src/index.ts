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
  /** Store nothing on the device (no cookie, no localStorage): the server
   * derives anonymous visitors and sessions from a hash that changes every
   * day. Session replay is off in this mode. (#273) */
  cookieless?: boolean;
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
  /** The team's own account (staff, testers): left out of the board by default. */
  internal?: boolean;
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
let activeVariants: Record<string, string> = {};

/** A micro-survey the server says this user may be asked (see survey.ts). */
export interface SurveyDef {
  key: string;
  question: string;
  kind: "rating" | "yes_no" | "choice" | "text";
  choices: string[];
  trigger: string;
}
let activeSurveys: SurveyDef[] = [];
let surveyOpen = false;

function uuid(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) return crypto.randomUUID();
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    return (c === "x" ? r : (r & 3) | 8).toString(16);
  });
}

/** Where the SDK keeps its state: the browser's localStorage, or memory
 * only in cookieless mode (#273), so nothing is written to the device. */
const memory = new Map<string, string>();
const memStore = {
  getItem: (k: string) => memory.get(k) ?? null,
  setItem: (k: string, v: string) => void memory.set(k, v),
  removeItem: (k: string) => void memory.delete(k),
};
function store(): Pick<Storage, "getItem" | "setItem" | "removeItem"> {
  return cfg?.cookieless ? memStore : localStorage;
}

/** In cookieless mode the server derives the anonymous visitor and the
 * session from a daily rotating hash: the SDK sends this placeholder. */
const COOKIELESS = "$cookieless";

function storageKey(): string {
  return `openlens_${cfg?.key ?? ""}`;
}

function load(): State {
  try {
    const raw = store().getItem(storageKey());
    if (raw) return JSON.parse(raw) as State;
  } catch {
    // Storage blocked: fall back to memory only.
  }
  if (cfg?.cookieless) return { anon: COOKIELESS, dims: {}, sid: COOKIELESS, last: 0 };
  return { anon: uuid(), dims: {}, sid: uuid(), last: 0 };
}

/** Pick up what another tab of the same app saved (login, logout, the
 * current session), so tabs share one visitor and one session. */
function refresh(): void {
  try {
    const raw = store().getItem(storageKey());
    if (raw) state = { ...state, ...(JSON.parse(raw) as State) };
  } catch {
    // Storage blocked: this tab keeps its own state.
  }
}

/** The page's route. Hash routers (`#/notes`, `#!/notes`) count as paths. */
function route(): string {
  const h = location.hash;
  const hashRoute = h.startsWith("#/") ? h.slice(1) : h.startsWith("#!/") ? h.slice(2) : "";
  if (!hashRoute) return location.pathname;
  return location.pathname.replace(/\/$/, "") + (hashRoute.split("?")[0] ?? "");
}

/** Crawlers and monitoring robots that run JavaScript are not visitors. */
function isBot(): boolean {
  return /bot\b|crawler|spider|slurp|facebookexternalhit|lighthouse|pingdom|uptime|prerender/i.test(navigator.userAgent || "");
}

function save(): void {
  try {
    store().setItem(storageKey(), JSON.stringify(state));
  } catch {
    // Storage blocked or full: keep going in memory.
  }
}

/** New session after 30 min of inactivity or when the UTC day changes. */
function touchSession(now: number): void {
  const dayChanged = new Date(now).toISOString().slice(0, 10) !== new Date(state.last).toISOString().slice(0, 10);
  if (!state.last || now - state.last > SESSION_GAP || dayChanged) state.sid = cfg?.cookieless ? COOKIELESS : uuid();
  state.last = now;
}

function base(): Props {
  const p: Props = {
    $current_url: location.href.split("#")[0],
    $pathname: route(),
    $environment: cfg?.environment || detectEnvironment(location.hostname),
    $lib: "web",
    $lib_version: VERSION,
  };
  if (cfg?.release) p.$release = cfg.release;
  if (state.dims.role) p.$role = state.dims.role;
  if (state.dims.account) p.$account = state.dims.account;
  if (state.dims.plan) p.$plan = state.dims.plan;
  if (state.dims.internal) p.$internal = true;
  if (activeFlags.length) p.$flags = activeFlags.join(",");
  const vs = Object.entries(activeVariants);
  if (vs.length) p.$variants = vs.map(([k, v]) => `${k}=${v}`).join(",");
  return p;
}

/** Record an event. */
export function track(event: string, props?: Props): void {
  if (!cfg || consent === "denied") return;
  const now = Date.now();
  refresh();
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
  maybeSurvey(event);
}

/** Show a micro-survey after its trigger event, once per user. */
function maybeSurvey(event: string): void {
  if (surveyOpen || consent !== "granted" || !cfg) return;
  const done = (k: string) => {
    try {
      return store().getItem(`${storageKey()}_survey_${k}`) === "1";
    } catch {
      return true;
    }
  };
  const s = activeSurveys.find((x) => x.trigger === event && !done(x.key));
  if (!s) return;
  surveyOpen = true;
  const close = (props: Props) => {
    try {
      store().setItem(`${storageKey()}_survey_${s.key}`, "1");
    } catch {
      // memory only
    }
    surveyOpen = false;
    track("$survey", { $survey: s.key, ...props });
    flush();
  };
  const w = window as unknown as {
    __openlensSurvey?: { survey: SurveyDef; answer: (v: string | number) => void; dismiss: () => void };
    __openlensSurveyShow?: () => void;
  };
  w.__openlensSurvey = { survey: s, answer: (v) => close({ $answer: v }), dismiss: () => close({ $dismissed: true }) };
  const host = cfg.host.replace(/\/$/, "");
  setTimeout(() => {
    if (w.__openlensSurveyShow) w.__openlensSurveyShow();
    else {
      const el = document.createElement("script");
      el.src = `${host}/s.js`;
      el.async = true;
      document.head.appendChild(el);
    }
  }, 1200);
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
    .then((j: { flags?: string[]; variants?: Record<string, string>; surveys?: SurveyDef[] } | null) => {
      if (Array.isArray(j?.surveys)) activeSurveys = j.surveys;
      if (j?.flags) {
        activeFlags = j.flags;
        activeVariants = j.variants ?? {};
        try {
          store().setItem(`${storageKey()}_flags`, JSON.stringify(activeFlags));
          store().setItem(`${storageKey()}_variants`, JSON.stringify(activeVariants));
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

/** The variant of an experiment flag this user gets (null when the flag is
 * off or has no variants). Events carry it, so arms can be compared. */
export function variant(key: string): string | null {
  return activeVariants[key] ?? null;
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

/** Forget the user (call on logout). A no-op while nobody is identified, so
 * calling it on every logged-out page load does not mint a new visitor. */
export function reset(): void {
  if (!cfg || !state.uid) return;
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
    if (cfg?.replay !== false && !cfg?.cookieless) startReplay();
  }
}

/** Send queued events now. */
export function flush(beacon = false): void {
  if (!cfg || consent !== "granted" || queue.length === 0) return;
  const batch = queue;
  const body = JSON.stringify({ sent_at: new Date().toISOString(), batch });
  queue = [];
  const url = `${cfg.host.replace(/\/$/, "")}/capture?key=${encodeURIComponent(cfg.key)}`;
  // text/plain keeps this a "simple" request: no CORS preflight.
  if (beacon && navigator.sendBeacon && navigator.sendBeacon(url, new Blob([body], { type: "text/plain" }))) return;
  // Offline or server unavailable: keep the events for the next flush (same
  // uuids), so a network blip loses nothing.
  const retry = () => {
    queue = batch.concat(queue).slice(-MAX_QUEUE);
  };
  origFetch?.(url, { method: "POST", body, keepalive: body.length < 60000, headers: { "content-type": "text/plain" } }).then(
    (r) => (r.status >= 500 || r.status === 429) && retry(),
    retry,
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
  const path = route();
  if (path === lastPath) return;
  if (lastPath) reportScroll();
  const first = lastPath === "";
  lastPath = path;
  // The first view of a page load also says the language and screen width
  // (#225; the server adds country, device, browser and OS, never the IP).
  track(
    "$pageview",
    first
      ? { ...(document.referrer ? { $referrer: document.referrer } : {}), ...utm(), $lang: navigator.language, $sw: screen.width }
      : {},
  );
}

/** Campaign parameters of the landing URL (#226): `utm_*` and the common
 * click ids, as `$utm_source`... (values cut to 100 characters). */
function utm(): Record<string, string> {
  const out: Record<string, string> = {};
  const q = new URLSearchParams(location.search);
  for (const k of ["utm_source", "utm_medium", "utm_campaign", "utm_term", "utm_content", "gclid", "fbclid", "msclkid"]) {
    const v = q.get(k);
    if (v) out["$" + k] = v.slice(0, 100);
  }
  return out;
}

/** `el` and its ancestors, crossing open shadow roots (web components):
 * from the event's composed path when there is one. */
function lineage(el: Element, path?: EventTarget[]): Element[] {
  const from = path ? path.indexOf(el) : -1;
  if (from >= 0) return path!.slice(from).filter((x): x is Element => x instanceof Element);
  const out: Element[] = [];
  for (let n: Node | null = el; n; n = n instanceof ShadowRoot ? n.host : n.parentNode) {
    if (n instanceof Element) out.push(n);
  }
  return out;
}

function describe(el: Element, path?: EventTarget[]): Props {
  const up = lineage(el, path);
  const p: Props = { $tag: el.tagName.toLowerCase() };
  const f = up.find((x) => x.hasAttribute("data-feature"));
  if (f) p.$feature = f.getAttribute("data-feature");
  if (el.id) p.$id = el.id;
  const isInput = el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement;
  // Text under data-openlens-mask is private (e.g. decrypted user content): never sent.
  const masked = up.some((x) => x.hasAttribute("data-openlens-mask"));
  if (!isInput && !masked) {
    const text = (el.textContent || "").replace(/\s+/g, " ").trim();
    if (text) p.$text = text.slice(0, 60);
  }
  const href = masked ? null : el.getAttribute("href");
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
  addEventListener("hashchange", pageview);
  // Back/forward cache: the page is shown again without loading.
  addEventListener("pageshow", (e) => {
    if (e.persisted) {
      lastPath = "";
      pageview();
    }
  });
  addEventListener("scroll", measureScroll, { passive: true });
  addEventListener("visibilitychange", () => document.visibilityState === "hidden" && reportScroll(), true);

  document.addEventListener(
    "click",
    (e) => {
      // The composed path reaches inside open shadow roots, where e.target
      // is only the component's host element.
      const path = e.composedPath();
      const target = path.find((x): x is Element => x instanceof Element && x.matches(CLICKABLE));
      if (!target || lineage(target, path).some((x) => x.hasAttribute("data-openlens-ignore"))) return;
      const now = Date.now();
      rage = rage.el === target && now - rage.t < 1000 ? { el: target, n: rage.n + 1, t: now } : { el: target, n: 1, t: now };
      // Position for heatmaps: x as a share of the viewport width (layouts
      // reflow with width), y in page pixels.
      const pos = e instanceof MouseEvent && innerWidth > 0
        ? { $x: Math.round((e.clientX / innerWidth) * 1000) / 1000, $y: Math.round(e.pageY), $vw: innerWidth }
        : {};
      track("$click", { ...describe(target, path), ...pos, ...(rage.n >= 3 ? { $rage: true } : {}) });
    },
    true,
  );
  const onSubmit = (e: Event) => {
    const form = e.target instanceof HTMLFormElement ? e.target : null;
    if (!form || lineage(form).some((x) => x.hasAttribute("data-openlens-ignore"))) return;
    track("$submit", { ...describe(form), $form: form.getAttribute("name") || form.id || undefined });
  };
  document.addEventListener("submit", onSubmit, true);
  // Form fields (#232): one `$field` event when a field is left, with the
  // time spent in it and whether it changed. The value never leaves the
  // page; passwords and ignored or masked fields send nothing.
  const fields = new WeakMap<Element, { t: number; v: string }>();
  const fieldOf = (e: Event) => {
    const el = e.composedPath()[0];
    if (!(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement)) return null;
    if (el instanceof HTMLInputElement && ["password", "hidden", "submit", "button", "reset", "image", "file"].includes(el.type)) return null;
    const form = el.form;
    if (!form || lineage(el).some((x) => x.hasAttribute("data-openlens-ignore") || x.hasAttribute("data-openlens-mask"))) return null;
    return { el, form };
  };
  document.addEventListener(
    "focusin",
    (e) => {
      const f = fieldOf(e);
      if (f) fields.set(f.el, { t: Date.now(), v: f.el.value });
    },
    true,
  );
  document.addEventListener(
    "focusout",
    (e) => {
      const f = fieldOf(e);
      const start = f && fields.get(f.el);
      if (!f || !start) return;
      fields.delete(f.el);
      const label = f.el.labels?.[0]?.textContent?.trim();
      const name = (f.el.getAttribute("name") || f.el.id || f.el.getAttribute("aria-label") || label || f.el.type || "field").slice(0, 60);
      track("$field", {
        $form: f.form.getAttribute("name") || f.form.id || undefined,
        $field: name,
        $ms: Math.min(Date.now() - start.t, 3_600_000),
        $changed: f.el.value !== start.v,
      });
    },
    true,
  );
  // `submit` does not leave a shadow root: listen on each open root the
  // user reaches (focus or click inside it comes before any submit).
  const watched = new WeakSet<ShadowRoot>();
  const watch = (e: Event) => {
    for (const n of e.composedPath()) {
      if (n instanceof ShadowRoot && !watched.has(n)) {
        watched.add(n);
        n.addEventListener("submit", onSubmit, true);
      }
    }
  };
  document.addEventListener("focusin", watch, true);
  document.addEventListener("pointerdown", watch, true);

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
    activeFlags = JSON.parse(store().getItem(`${storageKey()}_flags`) || "[]") as string[];
    activeVariants = JSON.parse(store().getItem(`${storageKey()}_variants`) || "{}") as Record<string, string>;
  } catch {
    activeFlags = [];
    activeVariants = {};
  }
  if (isBot()) consent = "denied";
  if (consent === "denied") return;
  loadFlags();
  // A prerendered page (speculation rules) is not a visit until it is shown.
  const doc = document as Document & { prerendering?: boolean };
  if (config.autocapture !== false) {
    if (doc.prerendering) document.addEventListener("prerenderingchange", autocapture, { once: true });
    else autocapture();
  }
  if (config.replay !== false && !config.cookieless && consent === "granted") startReplay();
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

export default { init, identify, track, feature, isEnabled, variant, reset, flush, setConsent, shutdown };
