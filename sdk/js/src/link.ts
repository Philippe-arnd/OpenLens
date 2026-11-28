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
export const ANON_PARAM = "_ol";

/** Sister hosts kept at most (the server sends at most as many). */
export const MAX_SISTERS = 20;

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const HOST = /^[a-z0-9.\-:[\]_]{1,253}$/;

/** An anonymous id from a URL: the SDK's own uuid shape, nothing else. */
export function validAnon(v: unknown): v is string {
  return typeof v === "string" && v.length === 36 && UUID.test(v);
}

/** The sister hosts from the `/flags` answer: lowercase host names, at
 * most {@link MAX_SISTERS}; anything else is dropped. */
export function validSisters(raw: unknown): string[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .filter((h): h is string => typeof h === "string")
    .map((h) => h.toLowerCase())
    .filter((h) => HOST.test(h))
    .slice(0, MAX_SISTERS);
}

/** The address to follow instead of `href` (resolved against `base`, the
 * current page): `href` with `_ol=<anon>` when it leads to a sister host
 * other than the page's own, else null (the link is left alone). */
export function withAnon(href: string, base: string, anon: string, sisters: string[]): string | null {
  if (!validAnon(anon) || sisters.length === 0) return null;
  let u: URL;
  let here: URL;
  try {
    u = new URL(href, base);
    here = new URL(base);
  } catch {
    return null;
  }
  if (u.protocol !== "https:" && u.protocol !== "http:") return null;
  const host = u.hostname.toLowerCase();
  if (host === here.hostname.toLowerCase() || !sisters.includes(host)) return null;
  u.searchParams.set(ANON_PARAM, anon);
  return u.href;
}

/** What a landing URL carries: the anonymous id (null when absent or not
 * valid) and the address without the parameter (null when there was no
 * parameter, so the address bar is left alone). */
export function takeAnon(href: string): { id: string | null; clean: string } | null {
  let u: URL;
  try {
    u = new URL(href);
  } catch {
    return null;
  }
  if (!u.searchParams.has(ANON_PARAM)) return null;
  const v = u.searchParams.get(ANON_PARAM);
  u.searchParams.delete(ANON_PARAM);
  return { id: validAnon(v) ? v.toLowerCase() : null, clean: u.href };
}

/** May the landing's id replace this page's anonymous id? Only for a
 * visitor with no history here: not identified, nothing tracked yet. */
export function mayAdopt(s: { uid?: string; last: number }): boolean {
  return !s.uid && !s.last;
}
