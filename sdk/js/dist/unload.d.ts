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
/** Bytes a leaving page may still send, below the browsers' ~64 KB. */
export declare const UNLOAD_BUDGET = 60000;
/** Most bytes kept for the next visit; the oldest go first. */
export declare const PARK_CAP = 256000;
/** How long a parked request is worth sending. */
export declare const PARK_TTL_MS: number;
/** Size in bytes on the wire (UTF-8), not in characters. */
export declare function bytes(s: string): number;
/** Keep `item` for the next visit, within [`PARK_CAP`] bytes. */
export declare function park(store: Store, item: Parked): void;
/** The parked requests still worth sending, removed from the store. */
export declare function takeParked(store: Store, now: number): Parked[];
/** Forget every parked request (consent withdrawn). */
export declare function clearParked(store: Store): void;
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
export declare class UnloadBudget {
    private spent;
    /** A new unload can spend the whole budget again (the page came back). */
    reset(): void;
    /** Send `body` to `url` if it fits in what is left, else park it.
     * Returns how it went. */
    send(s: Sender, url: string, body: string): "sent" | "parked" | "dropped";
}
