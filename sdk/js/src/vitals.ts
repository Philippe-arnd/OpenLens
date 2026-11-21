/**
 * Core Web Vitals from the W3C Performance APIs (our own implementation,
 * docs/kpis.md). Reported once per page load, when the page is hidden.
 */

type Report = (name: string, value: number, rating: string) => void;

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

export function trackVitals(report: Report): void {
  if (typeof PerformanceObserver === "undefined") return;
  const values: Record<string, number> = {};

  const nav = performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined;
  if (nav && nav.responseStart > 0) values.TTFB = nav.responseStart;

  observe("paint", (es) => {
    for (const e of es) if (e.name === "first-contentful-paint") values.FCP = e.startTime;
  });
  observe("largest-contentful-paint", (es) => {
    const last = es[es.length - 1];
    if (last) values.LCP = last.startTime;
  });

  // CLS: largest session window (gap < 1 s, window < 5 s), ignoring shifts
  // right after user input.
  let win = 0;
  let winStart = 0;
  let lastTs = 0;
  observe("layout-shift", (es) => {
    for (const e of es as Array<PerformanceEntry & { value: number; hadRecentInput: boolean }>) {
      if (e.hadRecentInput) continue;
      if (e.startTime - lastTs > 1000 || e.startTime - winStart > 5000) {
        win = 0;
        winStart = e.startTime;
      }
      win += e.value;
      lastTs = e.startTime;
      values.CLS = Math.max(values.CLS ?? 0, win);
    }
  });

  // INP: the slowest interaction (a close proxy for the p98 at the
  // interaction counts typical of one page view).
  observe(
    "event",
    (es) => {
      for (const e of es as Array<PerformanceEntry & { interactionId?: number }>) {
        if (e.interactionId) values.INP = Math.max(values.INP ?? 0, e.duration);
      }
    },
    { durationThreshold: 40 },
  );

  let sent = false;
  const flush = (force: boolean) => {
    if (sent || (!force && document.visibilityState !== "hidden")) return;
    sent = true;
    for (const name of Object.keys(values)) {
      const v = values[name];
      if (v === undefined) continue;
      const value = name === "CLS" ? Math.round(v * 1000) / 1000 : Math.round(v);
      report(name, value, rate(name, value));
    }
  };
  // Registered before the SDK's own flush listeners, so vitals join the
  // final beacon.
  addEventListener("visibilitychange", () => flush(false), true);
  addEventListener("pagehide", () => flush(true), true);
}
