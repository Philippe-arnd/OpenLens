export interface Dimensions { role?: string; account?: string; plan?: string }
export interface Options {
  host: string;
  key: string;
  release?: string;
  environment?: string;
  flushAt?: number;
  flushInterval?: number;
  onError?: (e: unknown) => void;
  fetch?: typeof fetch;
}
export declare class OpenLens {
  constructor(opts: Options);
  capture(distinctId: string, event: string, properties?: Record<string, unknown>): void;
  feature(distinctId: string, key: string, properties?: Record<string, unknown>): void;
  identify(distinctId: string, dims?: Dimensions): void;
  isEnabled(flag: string, distinctId: string, dims?: Dimensions): Promise<boolean>;
  flush(): Promise<void>;
  shutdown(): Promise<void>;
}
export default OpenLens;
