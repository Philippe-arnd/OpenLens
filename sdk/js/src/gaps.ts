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
export const IDLE_MS = 2 * 60 * 1000;
/** Shorter pauses are not worth a marker. */
const MIN_GAP_MS = 1000;

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

/** A full snapshot (page load or resume) or input: pointer, click, scroll,
 * typing, touch. DOM changes the page makes on its own are not input. */
function isInput(e: Ev): boolean {
  if (e.type === 2) return true;
  if (e.type !== 3) return false;
  const source = (e.data as { source?: unknown } | undefined)?.source;
  return source === 1 || source === 2 || source === 3 || source === 5 || source === 6;
}

function isIdle(e: Ev): boolean {
  return e.type === 5 && (e.data as { tag?: unknown } | undefined)?.tag === "idle";
}

/** The gaps of a recording, sorted and disjoint. */
export function gaps(events: readonly Ev[]): Gap[] {
  const first = events[0]?.timestamp;
  if (first === undefined) return [];
  const found: Gap[] = [];
  for (let i = 1; i < events.length; i++) {
    const e = events[i];
    const prev = events[i - 1];
    if (e && prev && isIdle(e)) found.push({ start: Math.min(prev.timestamp, e.timestamp) - first, end: e.timestamp - first });
  }
  const inputs = events
    .filter(isInput)
    .map((e) => e.timestamp)
    .sort((a, b) => a - b);
  for (let i = 1; i < inputs.length; i++) {
    const a = inputs[i - 1] ?? 0;
    const b = inputs[i] ?? 0;
    if (b - a > IDLE_MS) found.push({ start: a - first, end: b - first });
  }
  found.sort((a, b) => a.start - b.start);
  const out: Gap[] = [];
  for (const g of found) {
    const last = out[out.length - 1];
    if (last && g.start <= last.end) last.end = Math.max(last.end, g.end);
    else out.push({ ...g });
  }
  return out.filter((g) => g.end - g.start >= MIN_GAP_MS);
}

/** Active time at real offset `t`: `t` less the gaps before it. */
export function toActive(gs: readonly Gap[], t: number): number {
  let a = t;
  for (const g of gs) {
    if (g.start >= t) break;
    a -= Math.min(t, g.end) - g.start;
  }
  return Math.max(0, a);
}

/** Real offset of active time `a`; a position on a gap lands after it. */
export function toReal(gs: readonly Gap[], a: number): number {
  let t = Math.max(0, a);
  for (const g of gs) {
    if (t < g.start) break;
    t += g.end - g.start;
  }
  return t;
}

/** The gap that real offset `t` is in, if any. */
export function gapAt(gs: readonly Gap[], t: number): Gap | undefined {
  return gs.find((g) => t >= g.start && t < g.end);
}

/** "1 h 52 min", "7 min", "40 s". */
export function idleLabel(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s} s`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min`;
  return m % 60 ? `${Math.floor(m / 60)} h ${m % 60} min` : `${m / 60} h`;
}
