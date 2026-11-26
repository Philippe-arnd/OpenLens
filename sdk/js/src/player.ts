/**
 * Dashboard replay player, served at `/assets/player.js` on the replay page
 * only. Plays a recording with rrweb's Replayer (MIT) inside its sandboxed
 * iframe; recorded scripts never run. Controls are our own: play/pause,
 * ±10 s, a seek bar with the session's events as markers, speed, "skip
 * idle", keyboard shortcuts, and the list of events that follows the
 * playhead (click one to jump there).
 *
 * The seek bar and the times shown cover active time only: each idle gap
 * (see `gaps.ts`) is a "⏸ … idle" marker and playback jumps it (#404).
 * Positions handed to rrweb stay real offsets from the first event.
 *
 * Markup it expects: `<div id="ol-player" data-src="…events.json"
 * data-timeline="…timeline.json" data-at="time?">`.
 */
import { Replayer } from "rrweb";

import { type Ev, type Gap, gapAt, gaps, idleLabel, toActive, toReal } from "./gaps";

const root = document.getElementById("ol-player");
const src = root?.dataset.src;

function fmt(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string> = {}, text = ""): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  if (text) e.textContent = text;
  return e;
}

async function main(): Promise<void> {
  if (!root || !src) return;
  const r = await fetch(src, { credentials: "same-origin" });
  if (!r.ok) {
    root.textContent = "This recording cannot be loaded.";
    return;
  }
  const data = (await r.json()) as { events?: unknown[] };
  const events = (data.events ?? []) as ConstructorParameters<typeof Replayer>[0];
  if (events.length < 2) {
    root.textContent = "This recording is still empty: it fills in as the session goes on.";
    return;
  }
  const stage = el("div", { class: "ol-stage" });
  const bar = el("div", { class: "ol-controls" });
  const back = el("button", { type: "button", class: "ol-jump", "aria-label": "Back 10 seconds", title: "Back 10 s (J)" }, "−10 s");
  const play = el("button", { type: "button", class: "ol-play", "aria-label": "Play", title: "Play / pause (Space)" }, "▶");
  const fwd = el("button", { type: "button", class: "ol-jump", "aria-label": "Forward 10 seconds", title: "Forward 10 s (L)" }, "+10 s");
  const track = el("div", { class: "ol-track" });
  const seek = el("input", { type: "range", min: "0", step: "100", value: "0", "aria-label": "Position" });
  const marks = el("div", { class: "ol-marks" });
  track.append(seek, marks);
  const time = el("span", { class: "ol-time" });
  const speed = el("select", { "aria-label": "Speed", title: "Speed (+ / −)" });
  for (const s of ["0.5", "1", "2", "4", "8"]) speed.append(el("option", { value: s }, `${s}×`));
  speed.value = "1";
  const skipLabel = el("label", { class: "ol-skip" });
  const skip = el("input", { type: "checkbox", checked: "" });
  skipLabel.append(skip, " skip idle");
  const zoom = el("button", { type: "button", class: "ol-zoom", "aria-pressed": "false", title: "Actual size (Z)" }, "1:1");
  bar.append(back, play, fwd, track, time, speed, skipLabel, zoom);
  const info = el("p", { class: "ol-info", "aria-live": "polite" });
  const legend = el("ul", { class: "ol-legend", "aria-label": "Event types" });
  const log = el("ol", { class: "ol-log", "aria-label": "Events in this session" });
  const help = el("p", { class: "ol-help" }, "Space play/pause · J/L ±10 s · ←/→ ±5 s · +/− speed · click an event to jump to it");
  const main = el("div", { class: "ol-main" });
  main.append(stage, bar, info, legend, help);
  root.replaceChildren(main, log);

  const rep = new Replayer(events, { root: stage, skipInactive: true, mouseTail: false, showWarning: false });
  const total = rep.getMetaData().totalTime;
  const idle = gaps(events as unknown as Ev[]);
  // Real offset -> position on the seek bar (active time).
  const pos = (t: number) => toActive(idle, t);
  const span = pos(total);
  seek.max = String(span);
  let playing = false;
  let items: { t: number; li: HTMLLIElement }[] = [];
  let current: HTMLLIElement | undefined;
  const show = () => {
    let t = rep.getCurrentTime();
    const gap = playing ? gapAt(idle, t) : undefined;
    if (gap) {
      // Playback reached an idle gap: jump it.
      t = Math.min(total, gap.end);
      rep.play(t);
    }
    seek.value = String(pos(t));
    time.textContent = `${fmt(pos(t))} / ${fmt(span)}`;
    // The event list follows the playhead.
    const now = [...items].reverse().find((x) => x.t <= t + 250)?.li;
    if (now !== current) {
      current?.removeAttribute("aria-current");
      now?.setAttribute("aria-current", "true");
      if (now) log.scrollTop = now.offsetTop - log.offsetTop - log.clientHeight / 2;
      current = now;
    }
  };
  const setPlaying = (p: boolean) => {
    playing = p;
    play.textContent = p ? "❚❚" : "▶";
    play.setAttribute("aria-label", p ? "Pause" : "Play");
  };
  const go = (t: number) => {
    const at = Math.max(0, Math.min(total, t));
    if (playing) rep.play(at);
    else rep.pause(at);
    show();
  };
  // Move by `d` ms of active time.
  const step = (d: number) => go(toReal(idle, pos(rep.getCurrentTime()) + d));
  const toggle = () => {
    if (playing) rep.pause();
    else rep.play(rep.getCurrentTime() >= total ? 0 : rep.getCurrentTime());
    setPlaying(!playing);
  };
  play.addEventListener("click", toggle);
  back.addEventListener("click", () => step(-10_000));
  fwd.addEventListener("click", () => step(10_000));
  seek.addEventListener("input", () => go(toReal(idle, Number(seek.value))));
  const setSpeed = (v: string) => {
    speed.value = v;
    rep.setConfig({ speed: Number(v) });
  };
  speed.addEventListener("change", () => setSpeed(speed.value));
  addEventListener("keydown", (e) => {
    const t = e.target as HTMLElement;
    if (e.ctrlKey || e.metaKey || e.altKey || (t.tagName === "INPUT" && (t as HTMLInputElement).type !== "range") || t.tagName === "SELECT") return;
    const opts = [...speed.options].map((o) => o.value);
    const i = opts.indexOf(speed.value);
    const keys: Record<string, () => void> = {
      " ": toggle,
      k: toggle,
      j: () => step(-10_000),
      l: () => step(10_000),
      ArrowLeft: () => step(-5_000),
      ArrowRight: () => step(5_000),
      "+": () => setSpeed(opts[Math.min(opts.length - 1, i + 1)] ?? "1"),
      "-": () => setSpeed(opts[Math.max(0, i - 1)] ?? "1"),
      z: () => zoom.click(),
    };
    const f = keys[e.key];
    if (f) {
      e.preventDefault();
      f();
    }
  });
  skip.addEventListener("change", () => rep.setConfig({ skipInactive: skip.checked }));
  rep.on("finish", () => setPlaying(false));
  setInterval(show, 250);

  // Fit the recorded viewport into the room left on screen ("contain",
  // up to 2×), so the whole page and the controls show without scrolling;
  // the controls take the video's width. 1:1 shows actual size, scrolling
  // inside the stage. Heatmaps fit the width only (the page is tall).
  let actual = false;
  const widthOnly = Boolean(root.dataset.heatmap);
  const fit = () => {
    const wrapper = stage.querySelector<HTMLElement>(".replayer-wrapper");
    const frame = stage.querySelector("iframe");
    if (!wrapper || !frame) return;
    const w = Number(frame.getAttribute("width")) || 1;
    const h = Number(frame.getAttribute("height")) || 600;
    const room = main.clientWidth;
    const below = bar.offsetHeight + info.offsetHeight + 24;
    const top = Math.max(0, stage.getBoundingClientRect().top + scrollY);
    const tall = Math.max(240, innerHeight - Math.min(top, innerHeight / 3) - below);
    let scale = widthOnly ? Math.min(1, room / w) : Math.min(2, room / w, tall / h);
    if (actual) scale = 1;
    wrapper.style.transform = `scale(${scale})`;
    wrapper.style.transformOrigin = "0 0";
    const shownW = Math.min(room, w * scale);
    stage.style.width = `${shownW}px`;
    stage.style.height = `${actual ? Math.min(h, tall) : h * scale}px`;
    stage.style.overflow = actual ? "auto" : "hidden";
    root.style.setProperty("--ol-w", `${Math.max(shownW, Math.min(room, 320))}px`);
    const kind = w < 600 ? "Phone" : w < 1024 ? "Tablet" : "Desktop";
    info.textContent = `${kind} · ${w}×${h} · ${Math.round(scale * 100)} %`;
  };
  zoom.addEventListener("click", () => {
    actual = !actual;
    zoom.setAttribute("aria-pressed", String(actual));
    fit();
  });
  rep.on("resize", fit);
  addEventListener("resize", fit);
  if (typeof ResizeObserver !== "undefined") new ResizeObserver(() => fit()).observe(root);

  // data-at is a UTC timestamp ("2026-11-22 01:22:01.1"): start 3 s before it.
  if (root.dataset.heatmap) {
    await heatmapMode(rep, stage, bar, events as unknown as RecEvent[], root.dataset.heatmap, root.dataset.path ?? "", fit);
    return;
  }

  const first0 = (events[0] as { timestamp: number }).timestamp;
  const sc: Scale = { total, span, pos };
  if (root.dataset.timeline) {
    items = await timeline(root.dataset.timeline, first0, sc, marks, legend, log, (t) => go(t - 1000));
  }
  items = items
    .concat(comments(root.dataset.comments, sc, marks, (t) => go(t)))
    .concat(idleMarks(idle, sc, marks, (t) => go(t)))
    .sort((a, b) => a.t - b.t);
  for (const x of items) log.append(x.li);
  legend.hidden = !legend.childElementCount;
  if (!items.length) {
    legend.hidden = true;
    log.hidden = true;
  }
  // The comment form takes the moment the player is at (paused on focus).
  const form = document.getElementById("ol-comment") as HTMLFormElement | null;
  const atField = form?.querySelector<HTMLInputElement>("input[name=at]");
  form?.querySelector("textarea")?.addEventListener("focus", () => {
    if (playing) toggle();
  });
  form?.addEventListener("submit", () => {
    if (atField) atField.value = String(Math.round(rep.getCurrentTime()));
  });

  const at = root.dataset.at ? Date.parse(`${root.dataset.at.replace(" ", "T").slice(0, 23)}Z`) || 0 : 0;
  const first = (events[0] as { timestamp: number }).timestamp;
  const t0 = Number(root.dataset.t);
  const offset = root.dataset.t && t0 >= 0 ? Math.min(t0, total) : at > first ? Math.min(at - first - 3000, total) : 0;
  rep.pause(Math.max(0, offset));
  fit();
  show();
}

interface Mark {
  event: string;
  ms: number;
  feature?: string | null;
  path?: string | null;
  message?: string | null;
  rage?: boolean | null;
}

// Fixed order and colours (checked for colour-blind separation in light and
// dark); errors use the status colour and their own shape.
const KINDS: Record<string, string> = {
  page: "Page view",
  goal: "Success or custom event",
  submit: "Form sent",
  click: "Click",
  rage: "Rage click",
  error: "Error",
};

function kindOf(m: Mark): string {
  if (m.event === "$pageview") return "page";
  if (m.event === "$click") return m.rage ? "rage" : "click";
  if (m.event === "$submit") return "submit";
  if (m.event === "$error" || m.event === "$request_failed") return "error";
  return "goal";
}

function labelOf(m: Mark): string {
  switch (m.event) {
    case "$pageview":
      return m.path ?? "page";
    case "$click":
    case "$submit":
      return m.feature ? `${m.event === "$click" ? "click" : "submit"}: ${m.feature}` : `${m.event.slice(1)} on ${m.path ?? "the page"}`;
    case "$error":
      return `error: ${m.message ?? "unknown"}`;
    case "$request_failed":
      return `failed request on ${m.path ?? "the page"}`;
    case "$feature":
      return `feature: ${m.feature ?? ""}`;
    default:
      return m.event;
  }
}

/** Real offsets (what rrweb plays) and their place on the seek bar, which
 * covers active time only (#404). */
interface Scale {
  /** Real length of the recording, ms. */
  total: number;
  /** Active length, ms: the seek bar's range. */
  span: number;
  /** Real offset -> active time. */
  pos: (t: number) => number;
}

function left(sc: Scale, t: number): string {
  return `${sc.span ? (sc.pos(t) / sc.span) * 100 : 0}%`;
}

/** A "⏸ … idle" marker on the track and in the list for each gap; a click
 * goes to where activity resumes. */
function idleMarks(gs: readonly Gap[], sc: Scale, marks: HTMLElement, jump: (t: number) => void): { t: number; li: HTMLLIElement }[] {
  return gs.map((g) => {
    const label = `${idleLabel(g.end - g.start)} idle`;
    const at = fmt(sc.pos(g.start));
    const b = el("button", { type: "button", class: "ol-mark ol-gap k-idle", title: `${at} · ${label}`, "aria-label": `${at}, ${label}` }, "⏸");
    b.style.left = left(sc, g.start);
    b.addEventListener("click", () => jump(g.end));
    marks.append(b);
    const li = el("li", { class: "k-idle" });
    const go = el("button", { type: "button" });
    go.append(el("span", { class: "ol-at" }, at), el("span", { class: "ol-dot", "aria-hidden": "true" }), el("span", { class: "ol-what" }, `⏸ ${label}`));
    go.addEventListener("click", () => jump(g.end));
    li.append(go);
    return { t: g.start, li };
  });
}

/** Session events this long before the recording are not in it. */
const EARLIER_MS = 10_000;

/** Markers on the seek bar, a legend, and the list of events. */
async function timeline(
  url: string,
  first: number,
  sc: Scale,
  marks: HTMLElement,
  legend: HTMLElement,
  log: HTMLElement,
  jump: (t: number) => void,
): Promise<{ t: number; li: HTMLLIElement }[]> {
  const r = await fetch(url, { credentials: "same-origin" }).catch(() => null);
  if (!r?.ok) return [];
  const data = (await r.json()) as { events?: Mark[] };
  const seen = new Set<string>();
  const out: { t: number; li: HTMLLIElement }[] = [];
  for (const m of data.events ?? []) {
    // A recording that starts mid-session (sent after an error or a rage
    // click, #327) has none of the session's earlier events. The page view
    // a few seconds before the recorder loaded still shows, at the start.
    if (m.ms < first - EARLIER_MS) continue;
    const t = Math.max(0, Math.min(sc.total, m.ms - first));
    const k = kindOf(m);
    seen.add(k);
    const label = labelOf(m);
    const b = el("button", { type: "button", class: `ol-mark k-${k}`, title: `${fmt(sc.pos(t))} · ${label}`, "aria-label": `${fmt(sc.pos(t))}, ${label}` });
    b.style.left = left(sc, t);
    b.addEventListener("click", () => jump(t));
    marks.append(b);
    const li = el("li", { class: `k-${k}` });
    const go = el("button", { type: "button" });
    go.append(el("span", { class: "ol-at" }, fmt(sc.pos(t))), el("span", { class: "ol-dot", "aria-hidden": "true" }), el("span", { class: "ol-what" }, label));
    go.addEventListener("click", () => jump(t));
    li.append(go);
    log.append(li);
    out.push({ t, li });
  }
  for (const [k, name] of Object.entries(KINDS)) {
    if (!seen.has(k)) continue;
    const li = el("li", { class: `k-${k}` });
    li.append(el("span", { class: "ol-dot", "aria-hidden": "true" }), name);
    legend.append(li);
  }
  return out;
}

/** Comments pinned to moments (#213): markers on the track and entries in
 * the list, distinct from events. */
function comments(
  raw: string | undefined,
  sc: Scale,
  marks: HTMLElement,
  jump: (t: number) => void,
): { t: number; li: HTMLLIElement }[] {
  let list: { id: number; ms: number; body: string; author: string }[] = [];
  try {
    list = JSON.parse(raw ?? "[]");
  } catch {
    return [];
  }
  return list.map((c) => {
    const t = Math.max(0, Math.min(sc.total, c.ms));
    const label = `${c.author}: ${c.body}`;
    const b = el("button", { type: "button", class: "ol-mark k-note", title: `${fmt(sc.pos(t))} · ${label}`, "aria-label": `Comment at ${fmt(sc.pos(t))}, ${label}` });
    b.style.left = left(sc, t);
    b.addEventListener("click", () => jump(t));
    marks.append(b);
    const li = el("li", { class: "k-note" });
    const go = el("button", { type: "button" });
    go.append(el("span", { class: "ol-at" }, fmt(sc.pos(t))), el("span", { class: "ol-dot", "aria-hidden": "true" }), el("span", { class: "ol-what" }, `💬 ${label}`));
    go.addEventListener("click", () => jump(t));
    li.append(go);
    return { t, li };
  });
}

interface RecEvent {
  type: number;
  timestamp: number;
  data: { href?: string };
}

interface ClickMapJson {
  columns: number;
  row: number;
  map: { cells: [number, number, number, number][] };
}

/** Heatmap over the real page: show the first recorded visit of `path`,
 * stretch the frame to the whole page, and draw the click density on it. */
async function heatmapMode(
  rep: Replayer,
  stage: HTMLElement,
  bar: HTMLElement,
  events: RecEvent[],
  url: string,
  path: string,
  fit: () => void,
): Promise<void> {
  bar.hidden = true;
  const first = events[0]?.timestamp ?? 0;
  const metas = events.filter((e) => e.type === 4 && e.data?.href);
  const visit =
    metas.find((e) => {
      try {
        return new URL(e.data.href ?? "").pathname === path;
      } catch {
        return false;
      }
    }) ?? metas[0];
  rep.pause(Math.max(0, (visit?.timestamp ?? first) - first + 800));
  const r = await fetch(url, { credentials: "same-origin" });
  if (!r.ok) return;
  const data = (await r.json()) as ClickMapJson;
  await new Promise((done) => setTimeout(done, 300));
  const frame = stage.querySelector("iframe");
  const wrapper = stage.querySelector<HTMLElement>(".replayer-wrapper");
  if (!frame || !wrapper) return;
  const doc = frame.contentDocument;
  const width = Number(frame.getAttribute("width")) || 1280;
  const height = Math.max(
    Number(frame.getAttribute("height")) || 0,
    doc?.documentElement?.scrollHeight ?? 0,
    doc?.body?.scrollHeight ?? 0,
  );
  frame.setAttribute("height", String(height));
  const layer = document.createElement("div");
  layer.className = "ol-heat";
  layer.style.width = `${width}px`;
  layer.style.height = `${height}px`;
  const max = Math.max(1, ...data.map.cells.map((c) => c[2]));
  const size = Math.max(18, (width / data.columns) * 1.8);
  for (const [cx, cy, n] of data.map.cells) {
    const dot = document.createElement("span");
    dot.style.left = `${((cx + 0.5) / data.columns) * width}px`;
    dot.style.top = `${cy * data.row + data.row / 2}px`;
    dot.style.width = dot.style.height = `${size}px`;
    dot.style.opacity = String(Math.min(1, 0.25 + (0.75 * n) / max));
    layer.append(dot);
  }
  wrapper.style.position = "relative";
  wrapper.append(layer);
  fit();
}

void main();
