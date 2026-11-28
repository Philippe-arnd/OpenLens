/** Environment and release detection (docs/decisions.md, D-012, D-034). */
export declare function detectEnvironment(host: string): string;
/** The release the page states: `data-release` on the script tag, else
 * `<meta name="openlens-release">`, else the generic `<meta name="release">`
 * some frameworks emit (#495). */
export declare function detectRelease(script?: HTMLScriptElement | null): string | undefined;
/**
 * A short fingerprint of the build a page runs (#495): FNV-1a (32 bits, 8 hex
 * characters) of the sorted, de-duplicated script and stylesheet URLs of the
 * page's own origin (path and query; other origins and OpenLens's bundles
 * under `host` left out). Builds put a content hash in file names, so every
 * deploy changes it. `undefined` when the page loads no such file.
 */
export declare function buildFingerprint(urls: readonly string[], origin: string, host?: string): string | undefined;
/** FNV-1a of `text`: 32 bits as 8 hex characters. */
export declare function fnv(text: string): string;
/** The script and stylesheet URLs of the current page. */
export declare function pageAssets(): string[];
