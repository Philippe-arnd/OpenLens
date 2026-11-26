// Idle gaps in the player (#404), under `node --test`.
import assert from "node:assert/strict";
import { test } from "node:test";

import { IDLE_MS, gapAt, gaps, idleLabel, toActive, toReal, type Ev } from "../src/gaps";

const MIN = 60_000;
const input = (t: number): Ev => ({ type: 3, timestamp: t, data: { source: 2 } });
const clock = (t: number): Ev => ({ type: 3, timestamp: t, data: { source: 0 } });
const snapshot = (t: number): Ev => ({ type: 2, timestamp: t });
const idle = (t: number, ms: number): Ev => ({ type: 5, timestamp: t, data: { tag: "idle", payload: { ms } } });

test("an app left open 2 h with 3 min of use plays as about 3 min", () => {
  // An older recording: no idle events, a clock ticking while nobody is there.
  const events: Ev[] = [snapshot(0)];
  for (let t = 10_000; t <= 90_000; t += 10_000) events.push(input(t));
  for (let t = 2 * MIN; t < 118 * MIN; t += MIN) events.push(clock(t));
  for (let t = 118 * MIN; t <= 120 * MIN; t += 10_000) events.push(input(t));
  const gs = gaps(events);
  assert.equal(gs.length, 1);
  const total = 120 * MIN;
  const active = toActive(gs, total);
  assert.ok(active < 4 * MIN && active > 2 * MIN, `active ${active}`);
  // Playback reaching the gap jumps to its end.
  const g = gs[0];
  assert.ok(g && gapAt(gs, g.start + 1) === g);
  assert.ok(g && toReal(gs, toActive(gs, g.start + 1)) >= g.end);
});

test("a recorder pause is a gap even when shorter than the idle delay", () => {
  // The tab was hidden 30 s: the recorder stopped, then resumed.
  const events: Ev[] = [snapshot(0), input(5_000), idle(35_000, 30_000), snapshot(35_000), input(36_000)];
  const gs = gaps(events);
  assert.equal(gs.length, 1);
  assert.ok(toActive(gs, 36_000) < 36_000 - 20_000);
});

test("an active recording has no gap", () => {
  const events: Ev[] = [snapshot(0)];
  for (let t = 1000; t < 10 * MIN; t += IDLE_MS / 2) events.push(input(t));
  assert.deepEqual(gaps(events), []);
});

test("overlapping gaps merge and the mapping goes both ways", () => {
  // Idle for 2 min (recorded), then paused 1 h: one gap, not two.
  const events: Ev[] = [snapshot(0), input(1000), clock(60_000), clock(121_000), idle(3_721_000, 3_600_000), snapshot(3_721_000), input(3_722_000)];
  const gs = gaps(events);
  assert.equal(gs.length, 1);
  for (const a of [0, 500, 1500, 2500]) assert.equal(toActive(gs, toReal(gs, a)), a);
  assert.ok(toActive(gs, 3_722_000) < 5000);
});

test("idle labels read in minutes and hours", () => {
  assert.equal(idleLabel(112 * MIN), "1 h 52 min");
  assert.ok(idleLabel(40_000).endsWith(" s"));
  assert.ok(idleLabel(7 * MIN).endsWith(" min"));
});
