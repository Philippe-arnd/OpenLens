/**
 * Session replay recorder, a separate bundle served at `/r.js`.
 *
 * The core SDK loads it lazily (after the page is idle, only with consent)
 * and hands over a bridge on `window.__openlensReplay`. This file records DOM
 * changes with rrweb (MIT) and sends them to `POST /r` in numbered chunks,
 * keyed by the same session id as the analytics events.
 *
 * The recorder pauses after two minutes without input and as soon as the
 * tab is hidden (#404): nothing is recorded or sent while paused. The next
 * input, or the tab coming back, resumes the same replay with a custom
 * `idle` event (the gap length) and a fresh full snapshot; after the
 * session gap the core SDK starts a new session, so a new replay.
 *
 * Sampling (#327, D-035): a session the core SDK did not draw for
 * recording keeps only a rolling buffer in memory (a full snapshot every
 * 60 s, the last one or two minutes kept), sent when an error or a rage
 * click happens; the session is recorded from then on. Nothing buffered
 * leaves the page without such a trigger, and withdrawing consent empties
 * the buffer.
 *
 * Privacy defaults: every input value and every text is masked, except
 * text inside `data-openlens-unmask` (and not inside `data-openlens-mask`)
 * that the app marks as safe to show; elements with
 * `data-openlens-block` (and `data-openlens-ignore`) are not recorded at all.
 */
import { record } from "rrweb";

// No input for IDLE_MS pauses the recorder; the player and the server's
// active time use the same delay.
import { IDLE_MS } from "./gaps";
import { type Item, chunks, size, trim } from "./sampling";

export interface Bridge {
  host: string;
  key: string;
  /** Current session and person, read at every flush. */
  ids(): { sid: string; uid: string };
  /** False once consent is withdrawn. */
  allowed(): boolean;
  /** The page's own `fetch`, before the SDK wrapped it: the recorder's
   * requests stay out of the host app's error monitoring (#433). */
  fetch?: typeof fetch;
  /** Send while the page goes away, within the budget the core SDK shares
   * with its own last batch, or park it for the next visit (#433). */
  unload?: (url: string, body: string) => unknown;
  /** Input after a pause is activity: the core SDK refreshes the session
   * (a new one after its 30-minute gap) before the recorder resumes (#404). */
  touch?: () => void;
  /** Is the current session recorded (#327)? `undefined` while the rate
   * is not known yet. A core SDK without it records every session. */
  sampled?: () => boolean | undefined;
  /** Set by the recorder: the core SDK calls it after a trigger (error,
   * rage click) or a consent change, so the buffer goes out (or is
   * emptied) at once. */
  wake?: () => void;
}

declare global {
  interface Window {
    __openlensReplay?: Bridge;
  }
}

const FLUSH_MS = 10_000;
const MAX_EVENTS = 300;
/** About 30 KB of buffered events: the chunk sent as the page goes away
 * then fits the browsers' ~64 KB unload budget (#433). */
const MAX_CHARS = 30_000;
/** What counts as input. */
/** A session left out takes a full snapshot this often while buffering. */
const CHECKOUT_MS = 60_000;
/** Most characters the rolling buffer holds (about 1 MB of events). */
const BUFFER_CHARS = 1_000_000;
const INPUTS = ["mousemove", "mousedown", "pointerdown", "keydown", "wheel", "scroll", "touchstart"];

function start(b: Bridge): void {
  let buf: Item[] = [];
  let chars = 0;
  let sid = b.ids().sid;
  const sampled = () => !b.sampled || b.sampled() === true;
  // Recording live, or buffering until a trigger (#327).
  let live = sampled();
  const clear = () => {
    buf = [];
    chars = 0;
  };
  const url = `${b.host.replace(/\/$/, "")}/r?key=${encodeURIComponent(b.key)}`;

  // Chunk numbers are shared by the tabs of one session, so retries and
  // parallel tabs never overwrite each other.
  const nextSeq = (s: string): number => {
    const k = `openlens_rseq_${s}`;
    try {
      const n = Number(localStorage.getItem(k) || "0");
      localStorage.setItem(k, String(n + 1));
      return n;
    } catch {
      return Math.floor(Math.random() * 9000) + 1000;
    }
  };

  const send = (events: unknown[], s: string, uid: string, unloading: boolean) => {
    if (!events.length || !b.allowed()) return;
    const body = JSON.stringify({ session_id: s.replace(/[^A-Za-z0-9_-]/g, ""), distinct_id: uid, seq: nextSeq(s), events });
    if (unloading && b.unload) {
      b.unload(url, body);
      return;
    }
    const post = b.fetch ?? fetch;
    const go = (payload: BodyInit) =>
      post(url, { method: "POST", body: payload, keepalive: unloading && body.length < 60_000, headers: { "content-type": "text/plain" } }).catch(
        () => undefined,
      );
    if (!unloading && typeof CompressionStream !== "undefined") {
      new Response(new Blob([body]).stream().pipeThrough(new CompressionStream("gzip")))
        .blob()
        .then(go, () => go(body));
    } else go(body);
  };

  // The recorder's stop function; none while paused.
  let stop: (() => void) | undefined;
  let lastInput = Date.now();
  let pausedAt = 0;
  // The session the recorder paused in: resuming in it adds an idle event.
  let pausedIn = "";

  const flush = (unloading = false) => {
    const { sid: cur, uid } = b.ids();
    // Buffering, nothing is sent; a buffer of an earlier session is dropped.
    if (live || cur !== sid) {
      const events = buf;
      clear();
      if (live) for (const c of chunks(events, MAX_EVENTS, MAX_CHARS)) send(c, sid, uid, unloading);
    }
    if (cur !== sid) {
      // A new session started: its recording needs its own full snapshot
      // (taken by the recorder itself when it resumes, if it is paused),
      // and its own draw.
      sid = cur;
      live = sampled();
      if (stop) record.takeFullSnapshot(true);
    }
  };

  const keep = (e: unknown) => {
    if (!b.allowed()) return clear();
    const n = JSON.stringify(e).length;
    buf.push({ e, n });
    chars += n;
    if (live) {
      if (buf.length >= MAX_EVENTS || chars >= MAX_CHARS) flush();
    } else if ((e as { type?: unknown }).type === 2) {
      // A full snapshot: keep from the one before it.
      buf = trim(buf, BUFFER_CHARS);
      chars = size(buf);
    }
  };

  // A trigger, the rate arriving, another tab's trigger, or consent
  // withdrawn: send the buffer and go live, or empty it.
  const wake = () => {
    if (!b.allowed()) return clear();
    if (!live && sampled()) {
      live = true;
      flush();
    }
  };
  b.wake = wake;

  const rec = () =>
    record({
      emit: keep,
      maskAllInputs: true,
      // Every text is masked unless the app opts a part in with
      // data-openlens-unmask (data-openlens-mask inside it still wins).
      maskTextSelector: "*",
      maskTextFn: (text, el) =>
        el?.closest("[data-openlens-unmask]") && !el.closest("[data-openlens-mask]")
          ? text
          : text.replace(/\S/g, "*"),
      blockSelector: "[data-openlens-block],[data-openlens-ignore]",
      slimDOMOptions: "all",
      recordCanvas: false,
      collectFonts: false,
      inlineImages: false,
      sampling: { mousemove: 100, scroll: 150, media: 800, input: "last" },
    });

  // Hidden or idle: stop recording and send what is buffered, through the
  // unload path when the tab is hidden (the page may never come back).
  const pause = (unloading: boolean) => {
    if (!stop) return;
    stop();
    stop = undefined;
    pausedAt = Date.now();
    pausedIn = sid;
    flush(unloading);
  };

  const resume = () => {
    if (stop || !b.allowed()) return;
    const now = Date.now();
    b.touch?.();
    const cur = b.ids().sid;
    // Same replay: mark the gap. A new session starts a new replay instead.
    if (cur === pausedIn) keep({ type: 5, timestamp: now, data: { tag: "idle", payload: { ms: now - pausedAt } } });
    sid = cur;
    lastInput = now;
    stop = rec();
  };

  const onInput = () => {
    lastInput = Date.now();
    if (!stop && document.visibilityState !== "hidden") resume();
  };
  for (const t of INPUTS) addEventListener(t, onInput, { capture: true, passive: true });

  if (document.visibilityState !== "hidden") stop = rec();
  setInterval(() => {
    wake();
    if (!stop) return;
    if (Date.now() - lastInput >= IDLE_MS) pause(false);
    else flush();
    if (!live && chars > BUFFER_CHARS) {
      // Over the cap even from the latest snapshot: start again from a new one.
      clear();
      record.takeFullSnapshot(true);
    }
  }, FLUSH_MS);
  // The checkouts of a buffering session (rrweb's checkoutEveryNms, but
  // only until the session goes live).
  setInterval(() => {
    if (!live && stop) record.takeFullSnapshot(true);
  }, CHECKOUT_MS);
  addEventListener("visibilitychange", () => (document.visibilityState === "hidden" ? pause(true) : resume()));
  addEventListener("pagehide", () => flush(true));
}

if (typeof window !== "undefined" && window.__openlensReplay) start(window.__openlensReplay);
