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
  const at = root.dataset.at ? Date.parse(`${root.dataset.at.replace(" ", "T").slice(0, 23)}Z`) || 0 : 0;
  const first = (events[0] as { timestamp: number }).timestamp;
  const offset = at > first ? Math.min(at - first - 3000, total) : 0;
  rep.pause(Math.max(0, offset));
  fit();
  show();
}

void main();
