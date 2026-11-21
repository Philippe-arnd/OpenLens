/**
 * Script-tag entry: `<script src="https://lens.example.com/c.js" data-key="pk_…" defer></script>`.
 * The host defaults to the script's own origin, so the key is the only attribute needed.
 */
import openlens from "./index";
import { detectRelease } from "./env";

declare global {
  interface Window {
    openlens: typeof openlens;
  }
}

window.openlens = openlens;
const script = document.currentScript as HTMLScriptElement | null;
const key = script?.dataset.key;
if (script && key) {
  openlens.init({
    host: script.dataset.host || new URL(script.src).origin,
    key,
    release: detectRelease(script),
    environment: script.dataset.environment || undefined,
    consent: (script.dataset.consent as "granted" | "pending" | "denied" | undefined) || undefined,
  });
}
