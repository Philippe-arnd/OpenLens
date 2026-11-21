/**
 * Script-tag entry: `<script src="https://lens.example.com/c.js" data-key="pk_…" defer></script>`.
 * The host defaults to the script's own origin, so the key is the only attribute needed.
 */
import openlens from "./index";
declare global {
    interface Window {
        openlens: typeof openlens;
    }
}
