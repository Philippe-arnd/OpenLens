/**
 * OpenLens browser SDK.
 *
 * One call (`init`) and everything a PM needs is captured: page views,
 * clicks, form submits and SPA routes, sessions, errors, Web Vitals, failed
 * requests, plus environment and release. `identify` adds the user's role.
 * No cookies. Input values are never captured.
 */
import type { Bridge } from "./replay";
declare global {
    interface Window {
        __openlensReplay?: Bridge;
    }
}
export type Props = Record<string, unknown>;
export interface Config {
    /** OpenLens server URL, e.g. https://lens.example.com */
    host: string;
    /** Project capture key (pk_…). */
    key: string;
    /** Release of the app (version or commit). */
    release?: string;
    /** Override the detected environment. */
    environment?: string;
    /** Capture page views, clicks, submits, errors, vitals and failed requests (default true). */
    autocapture?: boolean;
    /** "pending" queues events until consent(true); "denied" captures nothing. Default "granted". */
    consent?: "granted" | "pending" | "denied";
    /** Record sessions for replay, inputs masked (default true). Loads `/r.js` lazily. */
    replay?: boolean;
}
export interface Dimensions {
    role?: string;
    account?: string;
    plan?: string;
}
/** Record an event. */
export declare function track(event: string, props?: Props): void;
/** Mark a use of a feature (a task start), for features without a data-feature control. */
export declare function feature(key: string, props?: Props): void;
/** Is a feature flag on for this user? Uses the last known answer (cached across page loads). */
export declare function isEnabled(key: string): boolean;
/** Identify the logged-in user and the dimensions OpenLens slices by. */
export declare function identify(id: string, dims?: Dimensions): void;
/** Forget the user (call on logout). */
export declare function reset(): void;
/** Grant or withdraw consent at runtime. */
export declare function setConsent(granted: boolean): void;
/** Send queued events now. */
export declare function flush(beacon?: boolean): void;
/** Start OpenLens. Safe to call once; later calls are ignored. */
export declare function init(config: Config): void;
/** Stop timers (tests, single-page teardown). */
export declare function shutdown(): void;
declare const _default: {
    init: typeof init;
    identify: typeof identify;
    track: typeof track;
    feature: typeof feature;
    isEnabled: typeof isEnabled;
    reset: typeof reset;
    flush: typeof flush;
    setConsent: typeof setConsent;
    shutdown: typeof shutdown;
};
export default _default;
