// src/env.ts
function detectEnvironment(host) {
  const h = host.toLowerCase();
  if (h === "localhost" || h === "0.0.0.0" || h === "[::1]" || h.startsWith("127.") || h.endsWith(".local") || h.endsWith(".localhost") || h.endsWith(".test")) {
    return "development";
  }
  if (/(^|[.-])(staging|stage|stg)([.-]|$)/.test(h)) return "staging";
  if (/(^|[.-])(preview|pr-\d+|review)([.-]|$)/.test(h) || h.endsWith(".vercel.app") || h.endsWith(".netlify.app")) {
    return "preview";
  }
  return "production";
}

// src/vitals.ts
var THRESHOLDS = {
  LCP: [2500, 4e3],
  INP: [200, 500],
  CLS: [0.1, 0.25],
  FCP: [1800, 3e3],
  TTFB: [800, 1800]
};
function rate(name, v) {
  const t = THRESHOLDS[name];
  if (!t) return "unknown";
  return v <= t[0] ? "good" : v <= t[1] ? "needs-improvement" : "poor";
}
function observe(type, cb, extra) {
  try {
    const po = new PerformanceObserver((list) => cb(list.getEntries()));
    po.observe({ type, buffered: true, ...extra });
  } catch {
  }
}
function trackVitals(report) {
  if (typeof PerformanceObserver === "undefined") return;
  const values = {};
  const nav = performance.getEntriesByType("navigation")[0];
  if (nav && nav.responseStart > 0) values.TTFB = nav.responseStart;
  observe("paint", (es) => {
    for (const e of es) if (e.name === "first-contentful-paint") values.FCP = e.startTime;
  });
  observe("largest-contentful-paint", (es) => {
    const last = es[es.length - 1];
    if (last) values.LCP = last.startTime;
  });
  let win = 0;
  let winStart = 0;
  let lastTs = 0;
  observe("layout-shift", (es) => {
    var _a;
    for (const e of es) {
      if (e.hadRecentInput) continue;
      if (e.startTime - lastTs > 1e3 || e.startTime - winStart > 5e3) {
        win = 0;
        winStart = e.startTime;
      }
      win += e.value;
      lastTs = e.startTime;
      values.CLS = Math.max((_a = values.CLS) != null ? _a : 0, win);
    }
  });
  observe(
    "event",
    (es) => {
      var _a;
      for (const e of es) {
        if (e.interactionId) values.INP = Math.max((_a = values.INP) != null ? _a : 0, e.duration);
      }
    },
    { durationThreshold: 40 }
  );
  let sent = false;
  const flush2 = (force) => {
    if (sent || !force && document.visibilityState !== "hidden") return;
    sent = true;
    for (const name of Object.keys(values)) {
      const v = values[name];
      if (v === void 0) continue;
      const value = name === "CLS" ? Math.round(v * 1e3) / 1e3 : Math.round(v);
      report(name, value, rate(name, value));
    }
  };
  addEventListener("visibilitychange", () => flush2(false), true);
  addEventListener("pagehide", () => flush2(true), true);
}

// src/index.ts
var VERSION = "0.1.0";
var SESSION_GAP = 30 * 60 * 1e3;
var MAX_QUEUE = 500;
var cfg;
var state;
var queue = [];
var consent = "granted";
var timer;
var lastPath = "";
var activeFlags = [];
function uuid() {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) return crypto.randomUUID();
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = Math.random() * 16 | 0;
    return (c === "x" ? r : r & 3 | 8).toString(16);
  });
}
function storageKey() {
  var _a;
  return `openlens_${(_a = cfg == null ? void 0 : cfg.key) != null ? _a : ""}`;
}
function load() {
  try {
    const raw = localStorage.getItem(storageKey());
    if (raw) return JSON.parse(raw);
  } catch {
  }
  return { anon: uuid(), dims: {}, sid: uuid(), last: 0 };
}
function save() {
  try {
    localStorage.setItem(storageKey(), JSON.stringify(state));
  } catch {
  }
}
function touchSession(now) {
  const dayChanged = new Date(now).toISOString().slice(0, 10) !== new Date(state.last).toISOString().slice(0, 10);
  if (!state.last || now - state.last > SESSION_GAP || dayChanged) state.sid = uuid();
  state.last = now;
}
function base() {
  const p = {
    $current_url: location.href.split("#")[0],
    $pathname: location.pathname,
    $environment: (cfg == null ? void 0 : cfg.environment) || detectEnvironment(location.hostname),
    $lib: "web",
    $lib_version: VERSION
  };
  if (cfg == null ? void 0 : cfg.release) p.$release = cfg.release;
  if (state.dims.role) p.$role = state.dims.role;
  if (state.dims.account) p.$account = state.dims.account;
  if (state.dims.plan) p.$plan = state.dims.plan;
  if (activeFlags.length) p.$flags = activeFlags.join(",");
  return p;
}
function track(event, props) {
  var _a;
  if (!cfg || consent === "denied") return;
  const now = Date.now();
  touchSession(now);
  save();
  queue.push({
    event,
    distinct_id: (_a = state.uid) != null ? _a : state.anon,
    session_id: state.sid,
    uuid: uuid(),
    timestamp: new Date(now).toISOString(),
    properties: { ...base(), ...props }
  });
  if (queue.length > MAX_QUEUE) queue = queue.slice(-MAX_QUEUE);
  if (queue.length >= 20) flush();
}
function feature(key, props) {
  track("$feature", { ...props, $feature: key });
}
function loadFlags() {
  var _a;
  if (!cfg || !origFetch) return;
  const q = new URLSearchParams({ key: cfg.key, distinct_id: (_a = state.uid) != null ? _a : state.anon });
  if (state.dims.role) q.set("role", state.dims.role);
  if (state.dims.account) q.set("account", state.dims.account);
  origFetch(`${cfg.host.replace(/\/$/, "")}/flags?${q}`).then((r) => r.ok ? r.json() : null).then((j) => {
    if (j == null ? void 0 : j.flags) {
      activeFlags = j.flags;
      try {
        localStorage.setItem(`${storageKey()}_flags`, JSON.stringify(activeFlags));
      } catch {
      }
    }
  }).catch(() => void 0);
}
function isEnabled(key) {
  return activeFlags.includes(key);
}
function identify(id, dims = {}) {
  if (!cfg || !id) return;
  const anon = state.uid ? void 0 : state.anon;
  state.uid = String(id);
  state.dims = { ...state.dims, ...dims };
  save();
  track("$identify", anon ? { $anon_distinct_id: anon } : {});
  loadFlags();
}
function reset() {
  if (!cfg) return;
  flush();
  state = { anon: uuid(), dims: {}, sid: uuid(), last: 0 };
  save();
}
function setConsent(granted) {
  consent = granted ? "granted" : "denied";
  if (!granted) queue = [];
  else flush();
}
function flush(beacon = false) {
  if (!cfg || consent !== "granted" || queue.length === 0) return;
  const body = JSON.stringify({ sent_at: (/* @__PURE__ */ new Date()).toISOString(), batch: queue });
  queue = [];
  const url = `${cfg.host.replace(/\/$/, "")}/capture?key=${encodeURIComponent(cfg.key)}`;
  if (beacon && navigator.sendBeacon) {
    navigator.sendBeacon(url, new Blob([body], { type: "text/plain" }));
    return;
  }
  origFetch == null ? void 0 : origFetch(url, { method: "POST", body, keepalive: true, headers: { "content-type": "text/plain" } }).catch(
    () => void 0
  );
}
var origFetch;
function pageview() {
  if (location.pathname === lastPath) return;
  const first = lastPath === "";
  lastPath = location.pathname;
  track("$pageview", first && document.referrer ? { $referrer: document.referrer } : {});
}
function describe(el) {
  var _a;
  const p = { $tag: el.tagName.toLowerCase() };
  const f = el.closest("[data-feature]");
  if (f) p.$feature = f.getAttribute("data-feature");
  if (el.id) p.$id = el.id;
  const isInput = el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement;
  if (!isInput) {
    const text = (el.textContent || "").replace(/\s+/g, " ").trim();
    if (text) p.$text = text.slice(0, 60);
  }
  const href = el.getAttribute("href");
  if (href) p.$href = (_a = href.split("?")[0]) == null ? void 0 : _a.slice(0, 200);
  return p;
}
var CLICKABLE = "a,button,[role=button],input[type=submit],input[type=button],summary,label,[data-feature]";
var rage = { el: null, n: 0, t: 0 };
function autocapture() {
  pageview();
  for (const m of ["pushState", "replaceState"]) {
    const orig = history[m];
    history[m] = function(...args) {
      const r = orig.apply(this, args);
      setTimeout(pageview, 0);
      return r;
    };
  }
  addEventListener("popstate", pageview);
  document.addEventListener(
    "click",
    (e) => {
      const target = e.target instanceof Element ? e.target.closest(CLICKABLE) : null;
      if (!target || target.closest("[data-openlens-ignore]")) return;
      const now = Date.now();
      rage = rage.el === target && now - rage.t < 1e3 ? { el: target, n: rage.n + 1, t: now } : { el: target, n: 1, t: now };
      track("$click", { ...describe(target), ...rage.n >= 3 ? { $rage: true } : {} });
    },
    true
  );
  document.addEventListener(
    "submit",
    (e) => {
      const form = e.target instanceof HTMLFormElement ? e.target : null;
      if (!form || form.closest("[data-openlens-ignore]")) return;
      track("$submit", { ...describe(form), $form: form.getAttribute("name") || form.id || void 0 });
    },
    true
  );
  const seen = {};
  const report = (kind, message, stack, source) => {
    var _a;
    const k = message.slice(0, 200);
    seen[k] = ((_a = seen[k]) != null ? _a : 0) + 1;
    if (seen[k] > 5) return;
    track("$error", { $kind: kind, $message: message.slice(0, 500), $stack: stack == null ? void 0 : stack.slice(0, 4e3), $source: source });
  };
  addEventListener("error", (e) => {
    var _a;
    if (e instanceof ErrorEvent) report("error", e.message || "error", (_a = e.error) == null ? void 0 : _a.stack, `${e.filename}:${e.lineno}:${e.colno}`);
  });
  addEventListener("unhandledrejection", (e) => {
    const r = e.reason;
    report("unhandledrejection", r instanceof Error ? r.message : String(r), r instanceof Error ? r.stack : void 0);
  });
  trackVitals((name, value, rating) => track("$vital", { $name: name, $value: value, $rating: rating }));
  const own = (u) => !!cfg && u.startsWith(cfg.host);
  const failed = (method, rawUrl, status) => {
    let path = rawUrl;
    try {
      const u = new URL(rawUrl, location.href);
      path = u.host === location.host ? u.pathname : u.host + u.pathname;
    } catch {
    }
    track("$request_failed", { $method: method.toUpperCase(), $path: path.slice(0, 300), $status: status });
  };
  if (origFetch) {
    const f = origFetch;
    window.fetch = function(input, init2) {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const method = (init2 == null ? void 0 : init2.method) || (input instanceof Request ? input.method : "GET");
      const p = f.call(window, input, init2);
      if (!own(url)) {
        p.then(
          (r) => r.status >= 500 && failed(method, url, r.status),
          () => failed(method, url, 0)
        );
      }
      return p;
    };
  }
  const open = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function(method, url, ...rest) {
    const u = String(url);
    if (!own(u)) {
      this.addEventListener("loadend", () => {
        if (this.status >= 500 || this.status === 0) failed(method, u, this.status);
      });
    }
    return open.call(this, method, url, ...rest);
  };
}
function init(config) {
  var _a, _b;
  if (cfg || typeof window === "undefined") return;
  if (!config.host || !config.key) return;
  if (navigator.doNotTrack === "1" || config.consent === "denied") consent = "denied";
  else consent = (_a = config.consent) != null ? _a : "granted";
  cfg = config;
  origFetch = (_b = window.fetch) == null ? void 0 : _b.bind(window);
  state = load();
  try {
    activeFlags = JSON.parse(localStorage.getItem(`${storageKey()}_flags`) || "[]");
  } catch {
    activeFlags = [];
  }
  if (consent === "denied") return;
  loadFlags();
  if (config.autocapture !== false) autocapture();
  timer = setInterval(() => flush(), 5e3);
  addEventListener("visibilitychange", () => document.visibilityState === "hidden" && flush(true));
  addEventListener("pagehide", () => flush(true));
}
function shutdown() {
  if (timer) clearInterval(timer);
  flush(true);
}
var index_default = { init, identify, track, feature, isEnabled, reset, flush, setConsent, shutdown };
export {
  index_default as default,
  feature,
  flush,
  identify,
  init,
  isEnabled,
  reset,
  setConsent,
  shutdown,
  track
};
