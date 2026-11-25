/**
 * Session replay recorder, a separate bundle served at `/r.js`.
 *
 * The core SDK loads it lazily (after the page is idle, only with consent)
 * and hands over a bridge on `window.__openlensReplay`. This file records DOM
 * changes with rrweb (MIT) and sends them to `POST /r` in numbered chunks,
 * keyed by the same session id as the analytics events.
 *
 * Privacy defaults: every input value and every text is masked, except
 * text inside `data-openlens-unmask` (and not inside `data-openlens-mask`)
 * that the app marks as safe to show; elements with
 * `data-openlens-block` (and `data-openlens-ignore`) are not recorded at all.
 */
import { record } from "rrweb";

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

function start(b: Bridge): void {
  let buf: unknown[] = [];
  let chars = 0;
  let sid = b.ids().sid;
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

  const flush = (unloading = false) => {
    const { sid: cur, uid } = b.ids();
    const events = buf;
    buf = [];
    chars = 0;
    send(events, sid, uid, unloading);
    if (cur !== sid) {
      // A new session started: its recording needs its own full snapshot.
      sid = cur;
      record.takeFullSnapshot(true);
    }
  };

  record({
    emit(e) {
      buf.push(e);
      chars += JSON.stringify(e).length;
      if (buf.length >= MAX_EVENTS || chars >= MAX_CHARS) flush();
    },
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
  setInterval(() => flush(), FLUSH_MS);
  addEventListener("visibilitychange", () => document.visibilityState === "hidden" && flush(true));
  addEventListener("pagehide", () => flush(true));
}

if (typeof window !== "undefined" && window.__openlensReplay) start(window.__openlensReplay);
