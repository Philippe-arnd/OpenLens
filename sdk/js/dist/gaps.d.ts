/**
 * Idle gaps in a recording (#404): the player's seek bar covers active
 * time only and playback jumps each gap.
 *
 * A gap is either a pause of the recorder (a custom `idle` event: from the
 * event before it to the event itself) or, for any recording, a stretch of
 * more than [`IDLE_MS`] without user input, the rule the server uses for
 * the list's active time. Overlapping gaps are merged.
 *
 * No DOM here, so it is tested under Node.
 */
/** The recorder pauses after this long without input; the same rule
 * finds gaps in recordings made before it paused. */
export declare const IDLE_MS: number;
/** The fields of an rrweb event this module reads. */
export interface Ev {
    type: number;
    timestamp: number;
    data?: unknown;
}
/** A stretch to jump, in ms from the first event. */
export interface Gap {
    start: number;
    end: number;
}
/** The gaps of a recording, sorted and disjoint. */
export declare function gaps(events: readonly Ev[]): Gap[];
/** Active time at real offset `t`: `t` less the gaps before it. */
export declare function toActive(gs: readonly Gap[], t: number): number;
/** Real offset of active time `a`; a position on a gap lands after it. */
export declare function toReal(gs: readonly Gap[], a: number): number;
/** The gap that real offset `t` is in, if any. */
export declare function gapAt(gs: readonly Gap[], t: number): Gap | undefined;
/** "1 h 52 min", "7 min", "40 s". */
export declare function idleLabel(ms: number): string;
