/**
 * One visitor across the project's own hosts (#710, D-112).
 *
 * localStorage is per origin, so a landing page on `www.example.com` and
 * the app on `app.example.com` would give the same visitor two anonymous
 * ids. The server lists the project's sister hosts with the flags (the
 * exact hosts its host rules map to this page's environment). When an
 * anonymous visitor follows a link to one of them, the SDK adds the
 * anonymous id as `_ol=<id>` to that link, at click time only. The app's
 * SDK adopts the id when it has no history of its own, then removes the
 * parameter from the address bar. No cookie; the id is the random one the
 * SDK already sends with every event.
 *
 * No DOM here, so it is tested under Node.
 */
/** The URL parameter that carries the anonymous id. */
export declare const ANON_PARAM = "_ol";
/** Sister hosts kept at most (the server sends at most as many). */
export declare const MAX_SISTERS = 20;
/** An anonymous id from a URL: the SDK's own uuid shape, nothing else. */
export declare function validAnon(v: unknown): v is string;
/** The sister hosts from the `/flags` answer: lowercase host names, at
 * most {@link MAX_SISTERS}; anything else is dropped. */
export declare function validSisters(raw: unknown): string[];
/** The address to follow instead of `href` (resolved against `base`, the
 * current page): `href` with `_ol=<anon>` when it leads to a sister host
 * other than the page's own, else null (the link is left alone). */
export declare function withAnon(href: string, base: string, anon: string, sisters: string[]): string | null;
/** What a landing URL carries: the anonymous id (null when absent or not
 * valid) and the address without the parameter (null when there was no
 * parameter, so the address bar is left alone). */
export declare function takeAnon(href: string): {
    id: string | null;
    clean: string;
} | null;
/** May the landing's id replace this page's anonymous id? Only for a
 * visitor with no history here: not identified, nothing tracked yet. */
export declare function mayAdopt(s: {
    uid?: string;
    last: number;
}): boolean;
