/**
 * Script-tag entry: `<script src="https://lens.example.com/c.js" data-key="pk_…" defer></script>`.
 * The host defaults to where the script came from (path prefix included), so the key is the
 * only attribute needed, also behind a proxy.
 */
import openlens from "./index";
import { detectRelease } from "./env";

declare global {
  interface Window {
    openlens: typeof openlens;
  }
}

/** The OpenLens server is where `c.js` came from, path included, so a proxy
 * on the app's own domain (`/_l/c.js` → OpenLens) needs no extra attribute. */
function serverOf(src: string): string {
  const u = new URL(src);
  const dir = u.pathname.endsWith("/c.js") ? u.pathname.slice(0, -"/c.js".length) : "";
  return u.origin + dir;
}

window.openlens = openlens;
const script = document.currentScript as HTMLScriptElement | null;
const key = script?.dataset.key;
if (script && key) {
  openlens.init({
    host: script.dataset.host || serverOf(script.src),
    key,
    release: detectRelease(script),
    environment: script.dataset.environment || undefined,
    replay: script.dataset.replay !== "off" && script.dataset.cookieless === undefined,
    cookieless: script.dataset.cookieless !== undefined && script.dataset.cookieless !== "false",
    consent: (script.dataset.consent as "granted" | "pending" | "denied" | undefined) || undefined,
  });
}
