/**
 * Core Web Vitals from the W3C Performance APIs (our own implementation,
 * docs/kpis.md). Reported once per page load, when the page is hidden.
 */
type Report = (name: string, value: number, rating: string) => void;
export declare function trackVitals(report: Report): void;
export {};
