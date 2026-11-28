/**
 * One error and one failed request per session and fingerprint (#703,
 * D-097). The first is sent at once; the same one again in the session is
 * only counted, and the count goes out as one more event marked
 * `$repeat: true` with `$repeats` at the next flush or on unload (only when
 * there are repeats, at most once per fingerprint and flush).
 *
 * No DOM here, so it is tested under Node.
 */
/** A repeat event to send, in the session of its first event. */
export interface Repeat {
    sid: string;
    event: string;
    props: Record<string, unknown>;
}
export declare class Repeats {
    private seen;
    /** Is this the first `event` with fingerprint `key` in session `sid`?
     * Then send it; else it is counted. `props` are what its repeat event
     * will carry. */
    first(event: string, key: string, sid: string, props: Record<string, unknown>): boolean;
    /** The repeat events to send now; the counts start again. */
    take(): Repeat[];
    /** Forget everything (consent withdrawn). */
    clear(): void;
}
/** The fingerprint of an error: its message and the first stack frame
 * with a position (the message alone without one). */
export declare function errorKey(kind: string, message: string, stack?: string): string;
