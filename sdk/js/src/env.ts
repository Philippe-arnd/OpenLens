/** Environment and release detection (docs/decisions.md, D-012). */

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
  return "production";
}

export function detectRelease(script?: HTMLScriptElement | null): string | undefined {
  const fromScript = script?.dataset.release;
  if (fromScript) return fromScript;
  const meta = document.querySelector<HTMLMetaElement>('meta[name="openlens-release"]');
  return meta?.content || undefined;
}
