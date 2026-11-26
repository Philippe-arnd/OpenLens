/**
 * Core Web Vitals from the W3C Performance APIs (our own implementation,
 * docs/kpis.md). Reported once per page load, when the page is hidden.
 *
 * Each `$vital` also says where it comes from (#467, raw facts only):
 * `$device` (phone, tablet, desktop) on every vital; LCP `$el`, `$el_type`
 * (image, text, video, background), `$res` and the parts `$ttfb`,
 * `$load_delay`, `$load_time`, `$render_delay`; INP `$el`, `$input`
 * (pointer, keyboard) and the parts `$input_delay`, `$processing`,
 * `$presentation`; CLS `$el`, the element that moved most in the worst
 * burst. Selectors carry no text (see attribution.ts).
 */
import { deviceClass, hidden, inpParts, lcpParts, resourcePath, selector } from "./attribution";

export type Facts = Record<string, string | number>;
type Report = (name: string, value: number, rating: string, facts: Facts) => void;

const THRESHOLDS: Record<string, [number, number]> = {
  LCP: [2500, 4000],
  INP: [200, 500],
  CLS: [0.1, 0.25],
  FCP: [1800, 3000],
  TTFB: [800, 1800],
};

function rate(name: string, v: number): string {
  const t = THRESHOLDS[name];
  if (!t) return "unknown";
  return v <= t[0] ? "good" : v <= t[1] ? "needs-improvement" : "poor";
}

function observe(type: string, cb: (entries: PerformanceEntry[]) => void, extra?: object): void {
  try {
    const po = new PerformanceObserver((list) => cb(list.getEntries()));
    po.observe({ type, buffered: true, ...extra } as PerformanceObserverInit);
  } catch {
    // Unsupported entry type: this vital is simply not reported.
  }
}

type Lcp = PerformanceEntry & { element?: Element | null; url?: string };
type Shift = PerformanceEntry & { value: number; hadRecentInput: boolean; sources?: Array<{ node?: Node | null; currentRect: DOMRectReadOnly }> };
type Ev = PerformanceEntry & { interactionId?: number; target?: Node | null; processingStart: number; processingEnd: number };

const el = (n: Node | null | undefined) => selector(n instanceof Element ? n : null);

/** LCP's facts: what the element is, its resource and the four parts. */
function lcpFacts(e: Lcp, lcp: number, ttfb: number): Facts {
  const f: Facts = {};
  const s = el(e.element);
  if (s) f.$el = s;
  const tag = e.element?.tagName;
  f.$el_type = !e.url ? "text" : tag === "IMG" || tag === "image" ? "image" : tag === "VIDEO" ? "video" : "background";
  let res: PerformanceResourceTiming | undefined;
  // A masked or ignored element's resource may name a person: not sent.
  if (e.url && !hidden(e.element)) {
    f.$res = resourcePath(e.url, location.href);
    res = (performance.getEntriesByName(e.url, "resource") as PerformanceResourceTiming[])[0];
  }
  // A cross-origin resource without Timing-Allow-Origin has no requestStart.
  const p = lcpParts(lcp, ttfb, res && (res.requestStart || res.startTime), res?.responseEnd);
  ["$ttfb", "$load_delay", "$load_time", "$render_delay"].forEach((k, i) => (f[k] = p[i]!));
  return f;
}

export function trackVitals(report: Report): void {
  if (typeof PerformanceObserver === "undefined") return;
  const values: Record<string, number> = {};
  const facts: Record<string, Facts> = {};

  const nav = performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined;
  const ttfb = nav && nav.responseStart > 0 ? nav.responseStart : 0;
  if (ttfb) values.TTFB = ttfb;

  observe("paint", (es) => {
    for (const e of es) if (e.name === "first-contentful-paint") values.FCP = e.startTime;
  });
  observe("largest-contentful-paint", (es) => {
    const last = es[es.length - 1] as Lcp | undefined;
    if (!last) return;
    values.LCP = last.startTime;
    facts.LCP = lcpFacts(last, last.startTime, ttfb);
  });

  // CLS: largest session window (gap < 1 s, window < 5 s), ignoring shifts
  // right after user input. The window's largest shift names the element.
  let win = 0;
  let winStart = 0;
  let lastTs = 0;
  let top = 0;
  let topEl = "";
  observe("layout-shift", (es) => {
    for (const e of es as Shift[]) {
      if (e.hadRecentInput) continue;
      if (e.startTime - lastTs > 1000 || e.startTime - winStart > 5000) {
        win = 0;
        top = 0;
        winStart = e.startTime;
      }
      win += e.value;
      lastTs = e.startTime;
      if (e.value > top) {
        top = e.value;
        // The source that covers most of the screen after the shift.
        let big: Node | null | undefined;
        let area = -1;
        for (const s of e.sources || []) {
          const a = s.currentRect.width * s.currentRect.height;
          if (a > area) [area, big] = [a, s.node];
        }
        topEl = el(big);
      }
      if (win > (values.CLS ?? 0)) {
        values.CLS = win;
        facts.CLS = topEl ? { $el: topEl } : {};
      }
    }
  });

  // INP: the slowest interaction (a close proxy for the p98 at the
  // interaction counts typical of one page view).
  observe(
    "event",
    (es) => {
      for (const e of es as Ev[]) {
        if (!e.interactionId || e.duration <= (values.INP ?? -1)) continue;
        values.INP = e.duration;
        const p = inpParts(e.startTime, e.processingStart, e.processingEnd, e.duration);
        const f: Facts = { $input: e.name.startsWith("key") ? "keyboard" : "pointer", $input_delay: p[0]!, $processing: p[1]!, $presentation: p[2]! };
        const s = el(e.target);
        if (s) f.$el = s;
        facts.INP = f;
      }
    },
    { durationThreshold: 40 },
  );

  let device = "";
  try {
    device = deviceClass(matchMedia("(pointer: coarse)").matches, Math.min(screen.width, screen.height));
  } catch {
    // No matchMedia: the device is left out.
  }

  let sent = false;
  const flush = (force: boolean) => {
    if (sent || (!force && document.visibilityState !== "hidden")) return;
    sent = true;
    for (const name of Object.keys(values)) {
      const v = values[name];
      if (v === undefined) continue;
      const value = name === "CLS" ? Math.round(v * 1000) / 1000 : Math.round(v);
      report(name, value, rate(name, value), { ...facts[name], ...(device ? { $device: device } : {}) });
    }
  };
  // Registered before the SDK's own flush listeners, so vitals join the
  // final beacon.
  addEventListener("visibilitychange", () => flush(false), true);
  addEventListener("pagehide", () => flush(true), true);
}
