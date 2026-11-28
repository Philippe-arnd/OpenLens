/** Environment and release detection (docs/decisions.md, D-012, D-034). */

export function detectEnvironment(host: string): string {
  const h = host.toLowerCase();
  if (
    h === "localhost" ||
    h === "0.0.0.0" ||
    h === "[::1]" ||
    h.startsWith("127.") ||
    h.endsWith(".local") ||
    h.endsWith(".localhost") ||
    h.endsWith(".test")
  ) {
    return "development";
  }
  if (/(^|[.-])(staging|stage|stg)([.-]|$)/.test(h)) return "staging";
  if (/(^|[.-])(preview|pr-\d+|review)([.-]|$)/.test(h) || h.endsWith(".vercel.app") || h.endsWith(".netlify.app")) {
    return "preview";
  }
  // No pattern: the server's host rules decide; without one it stays apart
  // from production (D-034).
  return "unknown";
}

/** The release the page states: `data-release` on the script tag, else
 * `<meta name="openlens-release">`, else the generic `<meta name="release">`
 * some frameworks emit (#495). */
export function detectRelease(script?: HTMLScriptElement | null): string | undefined {
  const fromScript = script?.dataset.release;
  if (fromScript) return fromScript;
  for (const name of ["openlens-release", "release"]) {
    const meta = document.querySelector<HTMLMetaElement>(`meta[name="${name}"]`);
    if (meta?.content) return meta.content;
  }
  return undefined;
}

/** OpenLens's own bundles, left out of the build fingerprint: an OpenLens
 * upgrade is not a release of the app. */
const OWN_BUNDLES = ["/c.js", "/r.js", "/s.js"];

/**
 * A short fingerprint of the build a page runs (#495): FNV-1a (32 bits, 8 hex
 * characters) of the sorted, de-duplicated script and stylesheet URLs of the
 * page's own origin (path and query; other origins and OpenLens's bundles
 * under `host` left out). Builds put a content hash in file names, so every
 * deploy changes it. `undefined` when the page loads no such file.
 */
export function buildFingerprint(urls: readonly string[], origin: string, host?: string): string | undefined {
  const own = host ? OWN_BUNDLES.map((b) => host.replace(/\/$/, "") + b) : [];
  const kept = new Set<string>();
  for (const raw of urls) {
    let u: URL;
    try {
      u = new URL(raw, origin);
    } catch {
      continue;
    }
    if (u.origin !== origin || own.includes(u.origin + u.pathname)) continue;
    kept.add(u.pathname + u.search);
  }
  if (kept.size === 0) return undefined;
  return fnv([...kept].sort().join("\n"));
}

/** FNV-1a of `text`: 32 bits as 8 hex characters. */
export function fnv(text: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16).padStart(8, "0");
}

/** The script and stylesheet URLs of the current page. */
export function pageAssets(): string[] {
  const urls: string[] = [];
  document.querySelectorAll<HTMLScriptElement>("script[src]").forEach((s) => urls.push(s.src));
  document.querySelectorAll<HTMLLinkElement>('link[rel~="stylesheet"][href]').forEach((l) => urls.push(l.href));
  return urls;
}
