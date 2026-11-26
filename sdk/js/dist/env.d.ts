/** Environment and release detection (docs/decisions.md, D-012, D-034). */
export declare function detectEnvironment(host: string): string;
export declare function detectRelease(script?: HTMLScriptElement | null): string | undefined;
