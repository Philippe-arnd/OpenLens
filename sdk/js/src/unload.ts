/**
 * What the SDK sends while the page goes away (#433).
 *
 * Browsers give the requests a leaving page may still send (`sendBeacon`
 * and `keepalive` fetches) one shared budget of about 64 KB, and abort any
 * other request. A payload over the budget used to be lost: the last batch
 * of events, the last replay chunk, or both. Here every unload send is
 * counted against one budget; what does not fit is parked in
 * `localStorage` and sent on the next page load. Resending is safe: events
 * are deduplicated by uuid and replay chunks by their number.
 *
 * No DOM here, so it is tested under Node.
 */

/** The part of `localStorage` this module uses. */
export interface Store {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/** A request that could not leave with the page. */
export interface Parked {
  url: string;
  body: string;
  /** When it was parked, in ms. */
  at: number;
}

const KEY = "openlens_unsent";
/** Bytes a leaving page may still send, below the browsers' ~64 KB. */
export const UNLOAD_BUDGET = 60_000;
/** Most bytes kept for the next visit; the oldest go first. */
export const PARK_CAP = 256_000;
/** How long a parked request is worth sending. */
export const PARK_TTL_MS = 24 * 3600 * 1000;

/** Size in bytes on the wire (UTF-8), not in characters. */
export function bytes(s: string): number {
  return new TextEncoder().encode(s).length;
}

function read(store: Store, now: number): Parked[] {
  try {
    const list = JSON.parse(store.getItem(KEY) || "[]");
    if (!Array.isArray(list)) return [];
    return list.filter(
      (p: Parked) =>
        p && typeof p.url === "string" && typeof p.body === "string" && typeof p.at === "number" && now - p.at < PARK_TTL_MS,
    );
  } catch {
    return [];
  }
}

/** Keep `item` for the next visit, within [`PARK_CAP`] bytes. */
export function park(store: Store, item: Parked): void {
  const list = read(store, item.at);
  list.push(item);
  let total = list.reduce((n, p) => n + bytes(p.body), 0);
  while (list.length && total > PARK_CAP) {
    const old = list.shift();
    if (old) total -= bytes(old.body);
  }
  try {
    if (list.length) store.setItem(KEY, JSON.stringify(list));
    else store.removeItem(KEY);
  } catch {
    // Storage full or blocked: nothing more can be done for this one.
  }
}

/** The parked requests still worth sending, removed from the store. */
export function takeParked(store: Store, now: number): Parked[] {
  const list = read(store, now);
  try {
    store.removeItem(KEY);
  } catch {
    // ignore
  }
  return list;
}

/** Forget every parked request (consent withdrawn). */
export function clearParked(store: Store): void {
  try {
    store.removeItem(KEY);
  } catch {
    // ignore
  }
}

/** How a leaving page sends a request: with the budget it has left. */
export interface Sender {
  /** `navigator.sendBeacon`-like: true when the browser took it. */
  beacon?: (url: string, body: string) => boolean;
  /** A `keepalive` fetch. */
  keepalive: (url: string, body: string) => void;
  /** Where to park what does not fit; none in cookieless mode or without consent. */
  store?: Store;
  now: () => number;
}

/** One unload's budget, shared by the core SDK and the replay bundle. */
export class UnloadBudget {
  private spent = 0;

  /** A new unload can spend the whole budget again (the page came back). */
  reset(): void {
    this.spent = 0;
  }

  /** Send `body` to `url` if it fits in what is left, else park it.
   * Returns how it went. */
  send(s: Sender, url: string, body: string): "sent" | "parked" | "dropped" {
    const n = bytes(body);
    if (this.spent + n <= UNLOAD_BUDGET) {
      if (s.beacon?.(url, body)) {
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
    // No storage allowed: try anyway; the browser may still let it go.
    s.keepalive(url, body);
    return "dropped";
  }
}
