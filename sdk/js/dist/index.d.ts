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
    /** Store nothing on the device (no cookie, no localStorage): the server
     * derives anonymous visitors and sessions from a hash that changes every
     * day. Session replay is off in this mode. (#273) */
    cookieless?: boolean;
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
    /** Other origins (`https://api.example.com`) whose requests carry the
     * `x-openlens-session` header that ties a server error to this session
     * (#593, #601). The page's own origin always does; another origin must
     * allow that header in its CORS answer. */
    traceOrigins?: string[];
}
export interface Dimensions {
    role?: string;
    account?: string;
    plan?: string;
    /** The team's own account (staff, testers): left out of the board by default. */
    internal?: boolean;
    /** Other properties flag rules can target (`seats`, `beta_tester`…): sent
     * to `/flags` to decide, never stored on events. At most 20. */
    properties?: Record<string, string | number | boolean>;
}
/** A micro-survey the server says this user may be asked (see survey.ts). */
export interface SurveyDef {
    key: string;
    question: string;
    kind: "rating" | "yes_no" | "choice" | "text";
    choices: string[];
    trigger: string;
}
/** Record an event. */
export declare function track(event: string, props?: Props): void;
/** Mark a use of a feature (a task start), for features without a data-feature control. */
export declare function feature(key: string, props?: Props): void;
/** What the user says (#718): a free text (1,000 characters kept), optionally about a feature
 * key of the map and with a rating from 1 to 5. Never sampled; sent at once. */
export declare function feedback(text: string, opts?: {
    feature?: string;
    rating?: number;
}): void;
/** Is a feature flag on for this user? Uses the last known answer (cached across page loads). */
export declare function isEnabled(key: string): boolean;
/** The variant of an experiment flag this user gets (null when the flag is
 * off or has no variants). Events carry it, so arms can be compared. */
export declare function variant(key: string): string | null;
/** Identify the logged-in user and the dimensions OpenLens slices by. */
export declare function identify(id: string, dims?: Dimensions): void;
/** Forget the user (call on logout). A no-op while nobody is identified, so
 * calling it on every logged-out page load does not mint a new visitor. */
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
    feedback: typeof feedback;
    isEnabled: typeof isEnabled;
    variant: typeof variant;
    reset: typeof reset;
    flush: typeof flush;
    setConsent: typeof setConsent;
    shutdown: typeof shutdown;
};
export default _default;
