/**
 * Replay sampling (#327, D-035), and the daily budget of the events that
 * only feed distributions (#702, D-097).
 *
 * The server hands the SDK a rate with the flags (`replay_rate`: about 300
 * recordings a day). Once per session the SDK draws whether the session is
 * recorded from its start; the draw is stored with the session, so every
 * tab of it agrees. A session left out still records into a rolling buffer
 * in memory: the last one or two minutes, from a full snapshot. An error or
 * a rage click sends that buffer and records the rest of the session.
 *
 * No DOM here, so it is tested under Node.
 */
/** The draw of one session. */
export interface Decision {
    /** The session drawn for. */
    sid: string;
    /** Recorded (from the start, or since a trigger). */
    on: boolean;
}
/** The decision for session `sid`: the stored one while the session lasts,
 * else a new draw at `rate`, or none while the rate is not known yet. */
export declare function decide(prev: Decision | undefined, sid: string, rate: number | undefined, rand: () => number): Decision | undefined;
/** The draw of one session for the distribution-only events (#702,
 * D-097), with the rate `r` it was drawn at. */
export interface Draw extends Decision {
    r: number;
}
/** The stored draw while the session lasts, else a new one at `rate`
 * (every session while the rate is not known). */
export declare function drawEvents(prev: Draw | undefined, sid: string, rate: number | undefined, rand: () => number): Draw;
/** The properties to send for `event` in a session drawn `d`, or `null`
 * when it is not sent. A click is always sent, but outside a drawn session
 * without its position (`$x`, `$y`, `$vw`): heatmaps are a sample too.
 * Every other event is never sampled. */
export declare function sample(event: string, props: Record<string, unknown>, d: Draw): Record<string, unknown> | null;
/** A rate from the server: a number from 0 to 1, else unknown. */
export declare function validRate(v: unknown): number | undefined;
/** A buffered recorder event and its size in characters. */
export interface Item {
    e: unknown;
    n: number;
}
/** The rolling buffer after a full snapshot arrived: from the snapshot
 * before the latest one (so it always holds at least one checkout period),
 * or from the latest one when that would exceed `cap` characters. A
 * snapshot's meta event, just before it, stays with it. */
export declare function trim(buf: Item[], cap: number): Item[];
/** Characters held. */
export declare function size(buf: readonly Item[]): number;
/** `buf` in chunks of at most `events` events and about `chars` characters
 * (an event larger than that is a chunk of its own). */
export declare function chunks(buf: readonly Item[], events: number, chars: number): unknown[][];
