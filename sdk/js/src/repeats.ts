/**
 * One error and one failed request per session and fingerprint (#703,
 * D-097). The first is sent at once; the same one again in the session is
 * only counted, and the count goes out as one more event marked
 * `$repeat: true` with `$repeats` at the next flush or on unload (only when
 * there are repeats, at most once per fingerprint and flush).
 *
 * No DOM here, so it is tested under Node.
 */

/** A fingerprint already sent in session `sid`, its event and properties,
 * and the repeats counted since the last flush. */
interface Seen {
  sid: string;
  event: string;
  props: Record<string, unknown>;
  n: number;
}

/** A repeat event to send, in the session of its first event. */
export interface Repeat {
  sid: string;
  event: string;
  props: Record<string, unknown>;
}

/** Fingerprints remembered at most; past that, new ones are sent as before. */
const MAX = 200;

export class Repeats {
  private seen = new Map<string, Seen>();

  /** Is this the first `event` with fingerprint `key` in session `sid`?
   * Then send it; else it is counted. `props` are what its repeat event
   * will carry. */
  first(event: string, key: string, sid: string, props: Record<string, unknown>): boolean {
    const k = `${event} ${key}`;
    const s = this.seen.get(k);
    if (s && s.sid === sid) {
      s.n++;
      return false;
    }
    if (s || this.seen.size < MAX) this.seen.set(k, { sid, event, props, n: 0 });
    return true;
  }

  /** The repeat events to send now; the counts start again. */
  take(): Repeat[] {
    const out: Repeat[] = [];
    for (const s of this.seen.values()) {
      if (s.n > 0) {
        out.push({ sid: s.sid, event: s.event, props: { ...s.props, $repeat: true, $repeats: s.n } });
        s.n = 0;
      }
    }
    return out;
  }

  /** Forget everything (consent withdrawn). */
  clear(): void {
    this.seen.clear();
  }
}

/** The fingerprint of an error: its message and the first stack frame
 * with a position (the message alone without one). */
export function errorKey(kind: string, message: string, stack?: string): string {
  const frame = stack
    ?.split("\n")
    .map((l) => l.trim())
    .find((l) => /:\d+:\d+\)?$/.test(l));
  return `${kind} ${message.slice(0, 200)} ${frame ?? ""}`;
}
