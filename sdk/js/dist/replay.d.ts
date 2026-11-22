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
}
declare global {
    interface Window {
        __openlensReplay?: Bridge;
    }
}
