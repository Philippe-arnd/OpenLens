export interface Bridge {
    host: string;
    key: string;
    /** Current session and person, read at every flush. */
    ids(): {
        sid: string;
        uid: string;
    };
    /** False once consent is withdrawn. */
    allowed(): boolean;
    /** The page's own `fetch`, before the SDK wrapped it: the recorder's
     * requests stay out of the host app's error monitoring (#433). */
    fetch?: typeof fetch;
    /** Send while the page goes away, within the budget the core SDK shares
     * with its own last batch, or park it for the next visit (#433). */
    unload?: (url: string, body: string) => unknown;
}
declare global {
    interface Window {
        __openlensReplay?: Bridge;
    }
}
