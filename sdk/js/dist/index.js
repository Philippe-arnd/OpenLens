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
  return "unknown";
}
function detectRelease(script) {
  const fromScript = script == null ? void 0 : script.dataset.release;
  if (fromScript) return fromScript;
  for (const name of ["openlens-release", "release"]) {
    const meta = document.querySelector(`meta[name="${name}"]`);
    if (meta == null ? void 0 : meta.content) return meta.content;
  }
  return void 0;
}
var OWN_BUNDLES = ["/c.js", "/r.js", "/s.js"];
function buildFingerprint(urls, origin, host) {
  const own = host ? OWN_BUNDLES.map((b) => host.replace(/\/$/, "") + b) : [];
  const kept = /* @__PURE__ */ new Set();
  for (const raw of urls) {
    let u;
    try {
      u = new URL(raw, origin);
    } catch {
      continue;
    }
    if (u.origin !== origin || own.includes(u.origin + u.pathname)) continue;
    kept.add(u.pathname + u.search);
  }
  if (kept.size === 0) return void 0;
  const text = [...kept].sort().join("\n");
  let h = 2166136261;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return (h >>> 0).toString(16).padStart(8, "0");
}
function pageAssets() {
  const urls = [];
  document.querySelectorAll("script[src]").forEach((s) => urls.push(s.src));
  document.querySelectorAll('link[rel~="stylesheet"][href]').forEach((l) => urls.push(l.href));
  return urls;
}

// src/attribution.ts
var MAX_SELECTOR = 120;
function stable(name) {
  if (!/^[a-zA-Z][\w-]{0,39}$/.test(name) || /\d/.test(name)) return false;
  if (/^(css|sc|jsx|svelte|emotion|chakra|mui|tw)-/i.test(name)) return false;
  const tail = name.split(/[-_]+/).pop() || "";
  return tail === name || !(/[a-z]/.test(tail) && /[A-Z]/.test(tail));
}
function step(el2, masked) {
  const tag = el2.tagName.toLowerCase();
  if (masked) return tag;
  if (el2.id && stable(el2.id)) return `${tag}#${el2.id}`;
  const cls = (el2.getAttribute("class") || "").split(/\s+/).filter(stable).slice(0, 2);
  return tag + cls.map((c) => "." + c).join("");
}
function hidden(el2) {
  let h = 0;
  for (let n = el2; n; n = n.parentElement) {
    if (n.hasAttribute("data-openlens-ignore")) return 2;
    if (n.hasAttribute("data-openlens-mask")) h = 1;
  }
  return h;
}
function selector(el2) {
  const h = hidden(el2);
  if (!el2 || h > 1) return "";
  const up = [];
  for (let n = el2; n; n = n.parentElement) up.push(n);
  const masked = h > 0;
  const parts = [];
  for (const n of up.slice(0, 3)) {
    const s = step(n, masked);
    if (parts.length && [s, ...parts].join(" > ").length > MAX_SELECTOR) break;
    parts.unshift(s);
    if (s.includes("#")) break;
  }
  return parts.join(" > ").slice(-MAX_SELECTOR);
}
function resourcePath(url, page) {
  try {
    const u = new URL(url, page);
    if (!/^https?:$/.test(u.protocol)) return u.protocol;
    const p = u.host === new URL(page).host ? u.pathname : u.host + u.pathname;
    return p.slice(0, 200);
  } catch {
    return "";
  }
}
function lcpParts(lcp, ttfb, loadStart, loadEnd) {
  const t = Math.min(Math.max(ttfb, 0), lcp);
  const start = loadStart ? Math.min(Math.max(loadStart, t), lcp) : t;
  const end = loadEnd ? Math.min(Math.max(loadEnd, start), lcp) : start;
  return [t, start - t, end - start, lcp - end].map(Math.round);
}
function inpParts(start, procStart, procEnd, duration) {
  const end = start + duration;
  const a = Math.min(Math.max(procStart, start), end);
  const b = Math.min(Math.max(procEnd, a), end);
  return [a - start, b - a, end - b].map(Math.round);
}
function deviceClass(coarse, shortSide) {
  return !coarse ? "desktop" : shortSide < 600 ? "phone" : "tablet";
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
var el = (n) => selector(n instanceof Element ? n : null);
function lcpFacts(e, lcp, ttfb) {
  var _a;
  const f = {};
  const s = el(e.element);
  if (s) f.$el = s;
  const tag = (_a = e.element) == null ? void 0 : _a.tagName;
  f.$el_type = !e.url ? "text" : tag === "IMG" || tag === "image" ? "image" : tag === "VIDEO" ? "video" : "background";
  let res;
  if (e.url && !hidden(e.element)) {
    f.$res = resourcePath(e.url, location.href);
    res = performance.getEntriesByName(e.url, "resource")[0];
  }
  const p = lcpParts(lcp, ttfb, res && (res.requestStart || res.startTime), res == null ? void 0 : res.responseEnd);
  ["$ttfb", "$load_delay", "$load_time", "$render_delay"].forEach((k, i) => f[k] = p[i]);
  return f;
}
function trackVitals(report) {
  if (typeof PerformanceObserver === "undefined") return;
  const values = {};
  const facts = {};
  const nav = performance.getEntriesByType("navigation")[0];
  const ttfb = nav && nav.responseStart > 0 ? nav.responseStart : 0;
  if (ttfb) values.TTFB = ttfb;
  observe("paint", (es) => {
    for (const e of es) if (e.name === "first-contentful-paint") values.FCP = e.startTime;
  });
  observe("largest-contentful-paint", (es) => {
    const last = es[es.length - 1];
    if (!last) return;
    values.LCP = last.startTime;
    facts.LCP = lcpFacts(last, last.startTime, ttfb);
  });
  let win = 0;
  let winStart = 0;
  let lastTs = 0;
  let top = 0;
  let topEl = "";
  observe("layout-shift", (es) => {
    var _a;
    for (const e of es) {
      if (e.hadRecentInput) continue;
      if (e.startTime - lastTs > 1e3 || e.startTime - winStart > 5e3) {
        win = 0;
        top = 0;
        winStart = e.startTime;
      }
      win += e.value;
      lastTs = e.startTime;
      if (e.value > top) {
        top = e.value;
        let big;
        let area = -1;
        for (const s of e.sources || []) {
          const a = s.currentRect.width * s.currentRect.height;
          if (a > area) [area, big] = [a, s.node];
        }
        topEl = el(big);
      }
      if (win > ((_a = values.CLS) != null ? _a : 0)) {
        values.CLS = win;
        facts.CLS = topEl ? { $el: topEl } : {};
      }
    }
  });
  observe(
    "event",
    (es) => {
      var _a;
      for (const e of es) {
        if (!e.interactionId || e.duration <= ((_a = values.INP) != null ? _a : -1)) continue;
        values.INP = e.duration;
        const p = inpParts(e.startTime, e.processingStart, e.processingEnd, e.duration);
        const f = { $input: e.name.startsWith("key") ? "keyboard" : "pointer", $input_delay: p[0], $processing: p[1], $presentation: p[2] };
        const s = el(e.target);
        if (s) f.$el = s;
        facts.INP = f;
      }
    },
    { durationThreshold: 40 }
  );
  let device = "";
  try {
    device = deviceClass(matchMedia("(pointer: coarse)").matches, Math.min(screen.width, screen.height));
  } catch {
  }
  let sent = false;
  const flush2 = (force) => {
    if (sent || !force && document.visibilityState !== "hidden") return;
    sent = true;
    for (const name of Object.keys(values)) {
      const v = values[name];
      if (v === void 0) continue;
      const value = name === "CLS" ? Math.round(v * 1e3) / 1e3 : Math.round(v);
      report(name, value, rate(name, value), { ...facts[name], ...device ? { $device: device } : {} });
    }
  };
  addEventListener("visibilitychange", () => flush2(false), true);
  addEventListener("pagehide", () => flush2(true), true);
}

// src/sampling.ts
function decide(prev, sid, rate2, rand) {
  if (prev && prev.sid === sid) return prev;
  if (rate2 === void 0) return void 0;
  return { sid, on: rand() < rate2 };
}
function validRate(v) {
  return typeof v === "number" && v >= 0 && v <= 1 ? v : void 0;
}

// src/unload.ts
var KEY = "openlens_unsent";
var UNLOAD_BUDGET = 6e4;
var PARK_CAP = 256e3;
var PARK_TTL_MS = 24 * 3600 * 1e3;
function bytes(s) {
  return new TextEncoder().encode(s).length;
}
function read(store2, now) {
  try {
    const list = JSON.parse(store2.getItem(KEY) || "[]");
    if (!Array.isArray(list)) return [];
    return list.filter(
      (p) => p && typeof p.url === "string" && typeof p.body === "string" && typeof p.at === "number" && now - p.at < PARK_TTL_MS
    );
  } catch {
    return [];
  }
}
function park(store2, item) {
  const list = read(store2, item.at);
  list.push(item);
  let total = list.reduce((n, p) => n + bytes(p.body), 0);
  while (list.length && total > PARK_CAP) {
    const old = list.shift();
    if (old) total -= bytes(old.body);
  }
  try {
    if (list.length) store2.setItem(KEY, JSON.stringify(list));
    else store2.removeItem(KEY);
  } catch {
  }
}
function takeParked(store2, now) {
  const list = read(store2, now);
  try {
    store2.removeItem(KEY);
  } catch {
  }
  return list;
}
function clearParked(store2) {
  try {
    store2.removeItem(KEY);
  } catch {
  }
}
var UnloadBudget = class {
  constructor() {
    this.spent = 0;
  }
  /** A new unload can spend the whole budget again (the page came back). */
  reset() {
    this.spent = 0;
  }
  /** Send `body` to `url` if it fits in what is left, else park it.
   * Returns how it went. */
  send(s, url, body) {
    var _a;
    const n = bytes(body);
    if (this.spent + n <= UNLOAD_BUDGET) {
      if ((_a = s.beacon) == null ? void 0 : _a.call(s, url, body)) {
        this.spent += n;
        return "sent";
      }
      if (!s.beacon) {
        this.spent += n;
        s.keepalive(url, body);
        return "sent";
      }
    }
    if (s.store) {
      park(s.store, { url, body, at: s.now() });
      return "parked";
    }
    s.keepalive(url, body);
    return "dropped";
  }
};

// src/link.ts
var ANON_PARAM = "_ol";
var MAX_SISTERS = 20;
var UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
var HOST = /^[a-z0-9.\-:[\]_]{1,253}$/;
function validAnon(v) {
  return typeof v === "string" && v.length === 36 && UUID.test(v);
}
function validSisters(raw) {
  if (!Array.isArray(raw)) return [];
  return raw.filter((h) => typeof h === "string").map((h) => h.toLowerCase()).filter((h) => HOST.test(h)).slice(0, MAX_SISTERS);
}
function withAnon(href, base2, anon, sisters) {
  if (!validAnon(anon) || sisters.length === 0) return null;
  let u;
  let here;
  try {
    u = new URL(href, base2);
    here = new URL(base2);
  } catch {
    return null;
  }
  if (u.protocol !== "https:" && u.protocol !== "http:") return null;
  const host = u.hostname.toLowerCase();
  if (host === here.hostname.toLowerCase() || !sisters.includes(host)) return null;
  u.searchParams.set(ANON_PARAM, anon);
  return u.href;
}
function takeAnon(href) {
  let u;
  try {
    u = new URL(href);
  } catch {
    return null;
  }
  if (!u.searchParams.has(ANON_PARAM)) return null;
  const v = u.searchParams.get(ANON_PARAM);
  u.searchParams.delete(ANON_PARAM);
  return { id: validAnon(v) ? v.toLowerCase() : null, clean: u.href };
}
function mayAdopt(s) {
  return !s.uid && !s.last;
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
var activeVariants = {};
var replayRate;
var build;
var sisterHosts = [];
var activeSurveys = [];
var surveyOpen = false;
function uuid() {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) return crypto.randomUUID();
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = Math.random() * 16 | 0;
    return (c === "x" ? r : r & 3 | 8).toString(16);
  });
}
var memory = /* @__PURE__ */ new Map();
var memStore = {
  getItem: (k) => {
    var _a;
    return (_a = memory.get(k)) != null ? _a : null;
  },
  setItem: (k, v) => void memory.set(k, v),
  removeItem: (k) => void memory.delete(k)
};
function store() {
  return (cfg == null ? void 0 : cfg.cookieless) ? memStore : localStorage;
}
var COOKIELESS = "$cookieless";
function storageKey() {
  var _a;
  return `openlens_${(_a = cfg == null ? void 0 : cfg.key) != null ? _a : ""}`;
}
function load() {
  try {
    const raw = store().getItem(storageKey());
    if (raw) return JSON.parse(raw);
  } catch {
  }
  if (cfg == null ? void 0 : cfg.cookieless) return { anon: COOKIELESS, dims: {}, sid: COOKIELESS, last: 0 };
  return { anon: uuid(), dims: {}, sid: uuid(), last: 0 };
}
function refresh() {
  try {
    const raw = store().getItem(storageKey());
    if (raw) state = { ...state, ...JSON.parse(raw) };
  } catch {
  }
}
function route() {
  var _a;
  const h = location.hash;
  const hashRoute = h.startsWith("#/") ? h.slice(1) : h.startsWith("#!/") ? h.slice(2) : "";
  if (!hashRoute) return location.pathname;
  return location.pathname.replace(/\/$/, "") + ((_a = hashRoute.split("?")[0]) != null ? _a : "");
}
function isBot() {
  return /bot\b|crawler|spider|slurp|facebookexternalhit|lighthouse|pingdom|uptime|prerender/i.test(navigator.userAgent || "");
}
function save() {
  try {
    store().setItem(storageKey(), JSON.stringify(state));
  } catch {
  }
}
function touchSession(now) {
  const dayChanged = new Date(now).toISOString().slice(0, 10) !== new Date(state.last).toISOString().slice(0, 10);
  if (!state.last || now - state.last > SESSION_GAP || dayChanged) state.sid = (cfg == null ? void 0 : cfg.cookieless) ? COOKIELESS : uuid();
  state.last = now;
}
function cleanUrl(href) {
  const [base2, query] = (href.split("#")[0] || "").split("?");
  if (!query) return base2 || "";
  const kept = query.split("&").filter((kv) => {
    const k = (kv.split("=")[0] || "").toLowerCase();
    return k.startsWith("utm_") || k === "ref" || k === "gclid";
  });
  return kept.length ? `${base2}?${kept.join("&")}` : base2 || "";
}
function personal(text) {
  return text.includes("@") || /\d{7,}/.test(text);
}
function base() {
  const p = {
    $current_url: cleanUrl(location.href),
    $pathname: route(),
    $environment: (cfg == null ? void 0 : cfg.environment) || detectEnvironment(location.hostname),
    $lib: "web",
    $lib_version: VERSION
  };
  if (cfg == null ? void 0 : cfg.release) p.$release = cfg.release;
  if (build) p.$build = build;
  if (state.dims.role) p.$role = state.dims.role;
  if (state.dims.account) p.$account = state.dims.account;
  if (state.dims.plan) p.$plan = state.dims.plan;
  if (state.dims.internal) p.$internal = true;
  if (activeFlags.length) p.$flags = activeFlags.join(",");
  const vs = Object.entries(activeVariants);
  if (vs.length) p.$variants = vs.map(([k, v]) => `${k}=${v}`).join(",");
  return p;
}
function track(event, props) {
  var _a, _b, _c;
  if (!cfg || consent === "denied") return;
  const now = Date.now();
  refresh();
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
  if (event === "$error" || (props == null ? void 0 : props.$rage) === true) {
    state.rs = { sid: state.sid, on: true };
    save();
    (_c = (_b = window.__openlensReplay) == null ? void 0 : _b.wake) == null ? void 0 : _c.call(_b);
  }
  maybeSurvey(event);
}
function maybeSurvey(event) {
  if (surveyOpen || consent !== "granted" || !cfg) return;
  const done = (k) => {
    try {
      return store().getItem(`${storageKey()}_survey_${k}`) === "1";
    } catch {
      return true;
    }
  };
  const s = activeSurveys.find((x) => x.trigger === event && !done(x.key));
  if (!s) return;
  surveyOpen = true;
  const close = (props) => {
    try {
      store().setItem(`${storageKey()}_survey_${s.key}`, "1");
    } catch {
    }
    surveyOpen = false;
    track("$survey", { $survey: s.key, ...props });
    flush();
  };
  const w = window;
  w.__openlensSurvey = { survey: s, answer: (v) => close({ $answer: v }), dismiss: () => close({ $dismissed: true }) };
  const host = cfg.host.replace(/\/$/, "");
  setTimeout(() => {
    if (w.__openlensSurveyShow) w.__openlensSurveyShow();
    else {
      const el2 = document.createElement("script");
      el2.src = `${host}/s.js`;
      el2.async = true;
      document.head.appendChild(el2);
    }
  }, 1200);
}
function feature(key, props) {
  track("$feature", { ...props, $feature: key });
}
function loadFlags() {
  var _a, _b;
  if (!cfg || !origFetch) return;
  const q = new URLSearchParams({ key: cfg.key, distinct_id: (_a = state.uid) != null ? _a : state.anon });
  if (state.dims.role) q.set("role", state.dims.role);
  if (state.dims.account) q.set("account", state.dims.account);
  if (state.dims.plan) q.set("plan", state.dims.plan);
  for (const [k, v] of Object.entries((_b = state.dims.properties) != null ? _b : {}).slice(0, 20)) {
    if (v !== void 0 && v !== null && v !== "") q.set(`p.${k.toLowerCase()}`, String(v).slice(0, 100));
  }
  origFetch(`${cfg.host.replace(/\/$/, "")}/flags?${q}`).then((r) => r.ok ? r.json() : null).then((j) => {
    var _a2, _b2, _c, _d;
    if (Array.isArray(j == null ? void 0 : j.surveys)) activeSurveys = j.surveys;
    if (j) sisterHosts = validSisters(j.sister_hosts);
    const rate2 = j ? (_a2 = validRate(j.replay_rate)) != null ? _a2 : 1 : void 0;
    if (rate2 !== void 0) {
      replayRate = rate2;
      try {
        store().setItem(`${storageKey()}_rate`, String(rate2));
      } catch {
      }
      (_c = (_b2 = window.__openlensReplay) == null ? void 0 : _b2.wake) == null ? void 0 : _c.call(_b2);
    }
    if (j == null ? void 0 : j.flags) {
      activeFlags = j.flags;
      activeVariants = (_d = j.variants) != null ? _d : {};
      try {
        store().setItem(`${storageKey()}_flags`, JSON.stringify(activeFlags));
        store().setItem(`${storageKey()}_variants`, JSON.stringify(activeVariants));
      } catch {
      }
    }
  }).catch(() => void 0);
}
function isEnabled(key) {
  return activeFlags.includes(key);
}
function variant(key) {
  var _a;
  return (_a = activeVariants[key]) != null ? _a : null;
}
function createdAt(v) {
  if (v === void 0 || v === null) return void 0;
  const t = v instanceof Date ? v : new Date(String(v));
  return isNaN(t.getTime()) ? void 0 : t.toISOString();
}
function identify(id, dims = {}) {
  if (!cfg || !id) return;
  const anon = state.uid ? void 0 : state.anon;
  const { created_at, ...traits } = dims;
  state.uid = String(id);
  state.dims = { ...state.dims, ...traits };
  save();
  const props = anon ? { $anon_distinct_id: anon } : {};
  const created = createdAt(created_at);
  if (created) props.$created_at = created;
  track("$identify", props);
  loadFlags();
}
function reset() {
  if (!cfg || !state.uid) return;
  flush();
  state = { anon: uuid(), dims: {}, sid: uuid(), last: 0 };
  save();
}
function setConsent(granted) {
  var _a, _b;
  consent = granted ? "granted" : "denied";
  if (!granted) {
    queue = [];
    (_b = (_a = window.__openlensReplay) == null ? void 0 : _a.wake) == null ? void 0 : _b.call(_a);
    if (cfg && !cfg.cookieless) {
      try {
        clearParked(localStorage);
      } catch {
      }
    }
  } else {
    flush();
    if ((cfg == null ? void 0 : cfg.replay) !== false && !(cfg == null ? void 0 : cfg.cookieless)) startReplay();
  }
}
function flush(beacon = false) {
  if (!cfg || consent !== "granted" || queue.length === 0) return;
  const batch = queue;
  const body = JSON.stringify({ sent_at: (/* @__PURE__ */ new Date()).toISOString(), batch });
  queue = [];
  const url = `${cfg.host.replace(/\/$/, "")}/capture?key=${encodeURIComponent(cfg.key)}`;
  if (beacon) {
    unloadBudget.send(unloadSender(), url, body);
    return;
  }
  const retry = () => {
    queue = batch.concat(queue).slice(-MAX_QUEUE);
  };
  origFetch == null ? void 0 : origFetch(url, { method: "POST", body, keepalive: body.length < 6e4, headers: { "content-type": "text/plain" } }).then(
    (r) => (r.status >= 500 || r.status === 429) && retry(),
    retry
  );
}
var origFetch;
var unloadBudget = new UnloadBudget();
function deviceStore() {
  if (!cfg || cfg.cookieless || consent !== "granted") return void 0;
  try {
    return localStorage;
  } catch {
    return void 0;
  }
}
function unloadSender() {
  const beacon = navigator.sendBeacon ? (url, body) => navigator.sendBeacon(url, new Blob([body], { type: "text/plain" })) : void 0;
  return {
    beacon,
    keepalive: (url, body) => void (origFetch == null ? void 0 : origFetch(url, { method: "POST", body, keepalive: true, headers: { "content-type": "text/plain" } }).catch(
      () => void 0
    )),
    store: deviceStore(),
    now: Date.now
  };
}
function resendParked() {
  const s = deviceStore();
  if (!s || !cfg) return;
  const host = cfg.host.replace(/\/$/, "");
  for (const p of takeParked(s, Date.now())) {
    if (!p.url.startsWith(`${host}/`)) continue;
    const again = () => park(s, p);
    origFetch == null ? void 0 : origFetch(p.url, { method: "POST", body: p.body, headers: { "content-type": "text/plain" } }).then(
      (r) => (r.status >= 500 || r.status === 429) && again(),
      again
    );
  }
}
var maxScroll = 0;
function measureScroll() {
  const doc = document.documentElement;
  const h = Math.max(doc.scrollHeight - window.innerHeight, 1);
  maxScroll = Math.max(maxScroll, Math.min(100, Math.round((window.scrollY || doc.scrollTop) / h * 100)));
}
function reportScroll() {
  if (lastPath && maxScroll >= 0) track("$scroll", { $depth: document.documentElement.scrollHeight <= window.innerHeight ? 100 : maxScroll, $pathname: lastPath });
  maxScroll = 0;
}
function pageview() {
  const path = route();
  if (path === lastPath) return;
  if (lastPath) reportScroll();
  const first = lastPath === "";
  lastPath = path;
  track(
    "$pageview",
    first ? { ...document.referrer ? { $referrer: document.referrer } : {}, ...utm(), $lang: navigator.language, $sw: screen.width } : {}
  );
}
function utm() {
  const out = {};
  const q = new URLSearchParams(location.search);
  for (const k of ["utm_source", "utm_medium", "utm_campaign", "utm_term", "utm_content", "gclid", "fbclid", "msclkid"]) {
    const v = q.get(k);
    if (v) out["$" + k] = v.slice(0, 100);
  }
  return out;
}
function lineage(el2, path) {
  const from = path ? path.indexOf(el2) : -1;
  if (from >= 0) return path.slice(from).filter((x) => x instanceof Element);
  const out = [];
  for (let n = el2; n; n = n instanceof ShadowRoot ? n.host : n.parentNode) {
    if (n instanceof Element) out.push(n);
  }
  return out;
}
function describe(el2, path) {
  var _a;
  const up = lineage(el2, path);
  const p = { $tag: el2.tagName.toLowerCase() };
  const f = up.find((x) => x.hasAttribute("data-feature"));
  if (f) p.$feature = f.getAttribute("data-feature");
  if (el2.id) p.$id = el2.id;
  const isInput = el2 instanceof HTMLInputElement || el2 instanceof HTMLTextAreaElement;
  const masked = up.some((x) => x.hasAttribute("data-openlens-mask"));
  if (!isInput && !masked) {
    const text = (el2.textContent || "").replace(/\s+/g, " ").trim().slice(0, 60);
    if (text && (f || !personal(text))) p.$text = text;
  }
  const href = masked ? null : el2.getAttribute("href");
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
  addEventListener("hashchange", pageview);
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
      const path = e.composedPath();
      const target = path.find((x) => x instanceof Element && x.matches(CLICKABLE));
      if (!target || lineage(target, path).some((x) => x.hasAttribute("data-openlens-ignore"))) return;
      const now = Date.now();
      rage = rage.el === target && now - rage.t < 1e3 ? { el: target, n: rage.n + 1, t: now } : { el: target, n: 1, t: now };
      const pos = e instanceof MouseEvent && innerWidth > 0 ? { $x: Math.round(e.clientX / innerWidth * 1e3) / 1e3, $y: Math.round(e.pageY), $vw: innerWidth } : {};
      track("$click", { ...describe(target, path), ...pos, ...rage.n >= 3 ? { $rage: true } : {} });
    },
    true
  );
  const onSubmit = (e) => {
    const form = e.target instanceof HTMLFormElement ? e.target : null;
    if (!form || lineage(form).some((x) => x.hasAttribute("data-openlens-ignore"))) return;
    track("$submit", { ...describe(form), $form: form.getAttribute("name") || form.id || void 0 });
  };
  document.addEventListener("submit", onSubmit, true);
  const fields = /* @__PURE__ */ new WeakMap();
  const fieldOf = (e) => {
    const el2 = e.composedPath()[0];
    if (!(el2 instanceof HTMLInputElement || el2 instanceof HTMLTextAreaElement || el2 instanceof HTMLSelectElement)) return null;
    if (el2 instanceof HTMLInputElement && ["password", "hidden", "submit", "button", "reset", "image", "file"].includes(el2.type)) return null;
    const form = el2.form;
    if (!form || lineage(el2).some((x) => x.hasAttribute("data-openlens-ignore") || x.hasAttribute("data-openlens-mask"))) return null;
    return { el: el2, form };
  };
  document.addEventListener(
    "focusin",
    (e) => {
      const f = fieldOf(e);
      if (f) fields.set(f.el, { t: Date.now(), v: f.el.value });
    },
    true
  );
  document.addEventListener(
    "focusout",
    (e) => {
      var _a, _b, _c;
      const f = fieldOf(e);
      const start = f && fields.get(f.el);
      if (!f || !start) return;
      fields.delete(f.el);
      const label = (_c = (_b = (_a = f.el.labels) == null ? void 0 : _a[0]) == null ? void 0 : _b.textContent) == null ? void 0 : _c.trim();
      const name = (f.el.getAttribute("name") || f.el.id || f.el.getAttribute("aria-label") || label || f.el.type || "field").slice(0, 60);
      track("$field", {
        $form: f.form.getAttribute("name") || f.form.id || void 0,
        $field: name,
        $ms: Math.min(Date.now() - start.t, 36e5),
        $changed: f.el.value !== start.v
      });
    },
    true
  );
  const watched = /* @__PURE__ */ new WeakSet();
  const watch = (e) => {
    for (const n of e.composedPath()) {
      if (n instanceof ShadowRoot && !watched.has(n)) {
        watched.add(n);
        n.addEventListener("submit", onSubmit, true);
      }
    }
  };
  document.addEventListener("focusin", watch, true);
  document.addEventListener("pointerdown", watch, true);
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
  trackVitals((name, value, rating, facts) => track("$vital", { ...facts, $name: name, $value: value, $rating: rating }));
  const own = (u) => !!cfg && u.startsWith(cfg.host);
  const failed = (method, rawUrl, status, trace) => {
    let path = rawUrl;
    try {
      const u = new URL(rawUrl, location.href);
      path = u.host === location.host ? u.pathname : u.host + u.pathname;
    } catch {
    }
    track("$request_failed", { $method: method.toUpperCase(), $path: path.slice(0, 300), $status: status, trace_id: trace });
  };
  if (origFetch) {
    const f = origFetch;
    window.fetch = function(input, init2) {
      var _a;
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const method = (init2 == null ? void 0 : init2.method) || (input instanceof Request ? input.method : "GET");
      let trace;
      if (!own(url) && traced(url)) {
        try {
          const h = new Headers((_a = init2 == null ? void 0 : init2.headers) != null ? _a : input instanceof Request ? input.headers : void 0);
          trace = linkHeaders((k) => h.get(k), (k, v) => h.set(k, v));
          init2 = { ...init2, headers: h };
        } catch {
        }
      }
      const p = f.call(window, input, init2);
      if (!own(url)) {
        p.then(
          (r) => r.status >= 500 && failed(method, url, r.status, trace),
          () => failed(method, url, 0, trace)
        );
      }
      return p;
    };
  }
  const open = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function(method, url, ...rest) {
    const u = String(url);
    const r = open.call(this, method, url, ...rest);
    if (!own(u)) {
      let trace;
      if (traced(u)) {
        try {
          linkHeaders(() => null, (k, v) => this.setRequestHeader(k, v));
        } catch {
        }
      }
      this.addEventListener("loadend", () => {
        if (this.status >= 500 || this.status === 0) failed(method, u, this.status, trace);
      });
    }
    return r;
  };
}
function traced(rawUrl) {
  var _a;
  if (!cfg || consent !== "granted") return false;
  try {
    const o = new URL(rawUrl, location.href).origin;
    return o === location.origin || ((_a = cfg.traceOrigins) != null ? _a : []).some((t) => t.replace(/\/$/, "") === o);
  } catch {
    return false;
  }
}
function linkHeaders(get, set) {
  var _a;
  if (!(cfg == null ? void 0 : cfg.cookieless) && !get("x-openlens-session")) set("x-openlens-session", state.sid);
  const m = /^[0-9a-f]{2}-([0-9a-f]{32})-[0-9a-f]{16}-[0-9a-f]{2}$/.exec(((_a = get("traceparent")) != null ? _a : "").trim());
  const id = m == null ? void 0 : m[1];
  return id && !/^0+$/.test(id) ? id : void 0;
}
function arrive() {
  let got;
  try {
    got = takeAnon(location.href);
  } catch {
    return;
  }
  if (!got) return;
  if (got.id && consent !== "denied" && !(cfg == null ? void 0 : cfg.cookieless) && mayAdopt(state)) {
    state.anon = got.id;
    save();
  }
  try {
    history.replaceState(history.state, "", got.clean);
  } catch {
  }
}
function linkOut(e) {
  var _a;
  if (!cfg || cfg.cookieless || consent !== "granted" || sisterHosts.length === 0) return;
  const a = e.composedPath().find((x) => x instanceof HTMLAnchorElement && x.hasAttribute("href"));
  if (!a) return;
  refresh();
  if (state.uid) return;
  const orig = (_a = a.getAttribute("href")) != null ? _a : "";
  const next = withAnon(a.href, location.href, state.anon, sisterHosts);
  if (!next) return;
  a.href = next;
  setTimeout(() => a.setAttribute("href", orig), 0);
}
function init(config) {
  var _a, _b, _c;
  if (cfg || typeof window === "undefined") return;
  if (!config.host || !config.key) return;
  const gpc = navigator.globalPrivacyControl === true;
  if (navigator.doNotTrack === "1" || gpc || config.consent === "denied") consent = "denied";
  else consent = (_a = config.consent) != null ? _a : "granted";
  cfg = { ...config, release: config.release || detectRelease() };
  try {
    build = buildFingerprint(pageAssets(), location.origin, config.host);
  } catch {
    build = void 0;
  }
  origFetch = (_b = window.fetch) == null ? void 0 : _b.bind(window);
  state = load();
  try {
    activeFlags = JSON.parse(store().getItem(`${storageKey()}_flags`) || "[]");
    activeVariants = JSON.parse(store().getItem(`${storageKey()}_variants`) || "{}");
    replayRate = validRate(Number((_c = store().getItem(`${storageKey()}_rate`)) != null ? _c : NaN));
  } catch {
    activeFlags = [];
    activeVariants = {};
  }
  if (isBot()) consent = "denied";
  arrive();
  if (consent === "denied") return;
  if (!config.cookieless) {
    document.addEventListener("click", linkOut, true);
    document.addEventListener("auxclick", linkOut, true);
  }
  loadFlags();
  const doc = document;
  if (config.autocapture !== false) {
    if (doc.prerendering) document.addEventListener("prerenderingchange", autocapture, { once: true });
    else autocapture();
  }
  resendParked();
  if (config.replay !== false && !config.cookieless && consent === "granted") startReplay();
  timer = setInterval(() => flush(), 5e3);
  addEventListener(
    "visibilitychange",
    () => document.visibilityState === "hidden" ? flush(true) : unloadBudget.reset()
  );
  addEventListener("pagehide", () => flush(true));
  addEventListener("pageshow", () => unloadBudget.reset());
}
function startReplay() {
  if (!cfg || window.__openlensReplay) return;
  const c = cfg;
  window.__openlensReplay = {
    host: c.host,
    key: c.key,
    ids: () => {
      var _a;
      return { sid: state.sid, uid: (_a = state.uid) != null ? _a : state.anon };
    },
    allowed: () => consent === "granted",
    fetch: origFetch,
    unload: (url, body) => unloadBudget.send(unloadSender(), url, body),
    touch: () => {
      refresh();
      touchSession(Date.now());
      save();
    },
    sampled: () => {
      refresh();
      const d = decide(state.rs, state.sid, replayRate, Math.random);
      if (d && d !== state.rs) {
        state.rs = d;
        save();
      }
      return d == null ? void 0 : d.on;
    }
  };
  const load2 = () => {
    const s = document.createElement("script");
    s.src = `${c.host.replace(/\/$/, "")}/r.js`;
    s.async = true;
    document.head.appendChild(s);
  };
  const idle = window.requestIdleCallback;
  if (document.readyState === "complete") (idle != null ? idle : setTimeout)(load2);
  else addEventListener("load", () => (idle != null ? idle : setTimeout)(load2));
}
function shutdown() {
  if (timer) clearInterval(timer);
  flush(true);
}
var index_default = { init, identify, track, feature, isEnabled, variant, reset, flush, setConsent, shutdown };
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
  track,
  variant
};
