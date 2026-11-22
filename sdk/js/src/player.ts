/**
 * Dashboard replay player, served at `/assets/player.js` on the replay page
 * only. Plays a recording with rrweb's Replayer (MIT) inside its sandboxed
 * iframe; recorded scripts never run. Controls are our own: play/pause, a
 * seek bar, speed and "skip idle".
 *
 * Markup it expects: `<div id="ol-player" data-src="…events.json" data-at="ms?">`.
 */
import { Replayer } from "rrweb";

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
  const play = el("button", { type: "button", class: "ol-play" }, "Play");
  const seek = el("input", { type: "range", min: "0", step: "100", value: "0", "aria-label": "Position" });
  const time = el("span", { class: "ol-time" });
  const speed = el("select", { "aria-label": "Speed" });
  for (const s of ["1", "2", "4", "8"]) speed.append(el("option", { value: s }, `${s}×`));
  const skipLabel = el("label");
  const skip = el("input", { type: "checkbox", checked: "" });
  skipLabel.append(skip, " skip idle");
  bar.append(play, seek, time, speed, skipLabel);
  root.replaceChildren(stage, bar);

  const rep = new Replayer(events, { root: stage, skipInactive: true, mouseTail: false, showWarning: false });
  const total = rep.getMetaData().totalTime;
  seek.max = String(total);
  let playing = false;
  const show = () => {
    const t = rep.getCurrentTime();
    seek.value = String(t);
    time.textContent = `${fmt(t)} / ${fmt(total)}`;
  };
  const setPlaying = (p: boolean) => {
    playing = p;
    play.textContent = p ? "Pause" : "Play";
  };
  play.addEventListener("click", () => {
    if (playing) rep.pause();
    else rep.play(rep.getCurrentTime() >= total ? 0 : rep.getCurrentTime());
    setPlaying(!playing);
  });
  seek.addEventListener("input", () => {
    const t = Number(seek.value);
    if (playing) rep.play(t);
    else rep.pause(t);
    show();
  });
  speed.addEventListener("change", () => rep.setConfig({ speed: Number(speed.value) }));
  skip.addEventListener("change", () => rep.setConfig({ skipInactive: skip.checked }));
  rep.on("finish", () => setPlaying(false));
  setInterval(show, 250);

  // Fit the recorded viewport into the stage.
  const fit = () => {
    const wrapper = stage.querySelector<HTMLElement>(".replayer-wrapper");
    const frame = stage.querySelector("iframe");
    if (!wrapper || !frame) return;
    const w = Number(frame.getAttribute("width")) || 1;
    const scale = Math.min(1, stage.clientWidth / w);
    wrapper.style.transform = `scale(${scale})`;
    wrapper.style.transformOrigin = "0 0";
    stage.style.height = `${(Number(frame.getAttribute("height")) || 600) * scale}px`;
  };
  rep.on("resize", fit);
  addEventListener("resize", fit);

  // data-at is a UTC timestamp ("2026-11-22 01:22:01.1"): start 3 s before it.
  if (root.dataset.heatmap) {
    await heatmapMode(rep, stage, bar, events as unknown as RecEvent[], root.dataset.heatmap, root.dataset.path ?? "", fit);
    return;
  }

  const at = root.dataset.at ? Date.parse(`${root.dataset.at.replace(" ", "T").slice(0, 23)}Z`) || 0 : 0;
  const first = (events[0] as { timestamp: number }).timestamp;
  const offset = at > first ? Math.min(at - first - 3000, total) : 0;
  rep.pause(Math.max(0, offset));
  fit();
  show();
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
