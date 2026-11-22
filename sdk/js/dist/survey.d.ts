/**
 * Micro-survey widget, a separate bundle served at `/s.js`, loaded only
 * when a survey is due. It reads `window.__openlensSurvey` (set by the core
 * SDK), shows one question in a small card (shadow DOM, so the page's CSS
 * and ours do not mix), and reports the answer or the dismissal back.
 * The card is excluded from replays.
 */
interface Def {
    key: string;
    question: string;
    kind: "rating" | "yes_no" | "choice" | "text";
    choices: string[];
}
interface Bridge {
    survey: Def;
    answer: (v: string | number) => void;
    dismiss: () => void;
}
declare global {
    interface Window {
        __openlensSurvey?: Bridge;
        __openlensSurveyShow?: () => void;
    }
}
export {};
