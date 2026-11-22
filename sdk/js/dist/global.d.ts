/**
 * Script-tag entry: `<script src="https://lens.example.com/c.js" data-key="pk_…" defer></script>`.
 * The host defaults to where the script came from (path prefix included), so the key is the
 * only attribute needed, also behind a proxy.
 */
import openlens from "./index";
declare global {
    interface Window {
        openlens: typeof openlens;
    }
}
