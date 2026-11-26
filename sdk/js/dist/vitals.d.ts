export type Facts = Record<string, string | number>;
type Report = (name: string, value: number, rating: string, facts: Facts) => void;
export declare function trackVitals(report: Report): void;
export {};
