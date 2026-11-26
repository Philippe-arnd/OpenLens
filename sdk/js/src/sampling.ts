/**
 * Replay sampling (#327, D-035).
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
export function decide(prev: Decision | undefined, sid: string, rate: number | undefined, rand: () => number): Decision | undefined {
  if (prev && prev.sid === sid) return prev;
  if (rate === undefined) return undefined;
  return { sid, on: rand() < rate };
}

/** A rate from the server: a number from 0 to 1, else unknown. */
export function validRate(v: unknown): number | undefined {
  return typeof v === "number" && v >= 0 && v <= 1 ? v : undefined;
}

/** A buffered recorder event and its size in characters. */
export interface Item {
  e: unknown;
  n: number;
}

const typeOf = (x: Item) => (x.e as { type?: unknown } | null)?.type;

/** The rolling buffer after a full snapshot arrived: from the snapshot
 * before the latest one (so it always holds at least one checkout period),
 * or from the latest one when that would exceed `cap` characters. A
 * snapshot's meta event, just before it, stays with it. */
export function trim(buf: Item[], cap: number): Item[] {
  const starts: number[] = [];
  buf.forEach((x, i) => {
    if (typeOf(x) === 2) starts.push(i > 0 && typeOf(buf[i - 1] as Item) === 4 ? i - 1 : i);
  });
  const last = starts[starts.length - 1];
  if (last === undefined) return buf;
  const prev = starts[starts.length - 2];
  if (prev !== undefined) {
    const kept = buf.slice(prev);
    if (size(kept) <= cap) return kept;
  }
  return buf.slice(last);
}

/** Characters held. */
export function size(buf: readonly Item[]): number {
  let n = 0;
  for (const x of buf) n += x.n;
  return n;
}

/** `buf` in chunks of at most `events` events and about `chars` characters
 * (an event larger than that is a chunk of its own). */
export function chunks(buf: readonly Item[], events: number, chars: number): unknown[][] {
  const out: unknown[][] = [];
  let cur: unknown[] = [];
  let n = 0;
  for (const x of buf) {
    if (cur.length && (cur.length >= events || n + x.n > chars)) {
      out.push(cur);
      cur = [];
      n = 0;
    }
    cur.push(x.e);
    n += x.n;
  }
  if (cur.length) out.push(cur);
  return out;
}
