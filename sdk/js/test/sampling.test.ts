// Replay sampling (#327), under `node --test`.
import assert from "node:assert/strict";
import { test } from "node:test";

import { type Item, chunks, decide, drawEvents, sample, size, trim, validRate } from "../src/sampling";

test("a session is drawn once: later calls and other tabs keep the draw", () => {
  let draws = 0;
  const rand = () => {
    draws++;
    return 0.5;
  };
  // The rate is not known yet: no decision, nothing drawn.
  assert.equal(decide(undefined, "s1", undefined, rand), undefined);
  const first = decide(undefined, "s1", 0.9, rand);
  assert.ok(first?.on);
  // A lower rate arriving later does not change this session's draw.
  assert.equal(decide(first, "s1", 0.1, rand), first);
  // A trigger stored by another tab is kept too.
  const triggered = { sid: "s1", on: true };
  assert.equal(decide(triggered, "s1", 0, rand), triggered);
  assert.equal(draws, 1);
  // A new session draws again.
  const next = decide(first, "s2", 0.1, rand);
  assert.equal(next?.sid, "s2");
  assert.equal(next?.on, false);
  assert.equal(draws, 2);
});

test("the share of sessions drawn follows the rate", () => {
  let seed = 1;
  const rand = () => {
    seed = (seed * 16807) % 2147483647;
    return seed / 2147483647;
  };
  const share = (rate: number) => {
    let on = 0;
    for (let i = 0; i < 10_000; i++) if (decide(undefined, `s${i}`, rate, rand)?.on) on++;
    return on / 10_000;
  };
  const low = share(0.1);
  const high = share(0.5);
  assert.ok(low < high, `${low} < ${high}`);
  assert.equal(share(1), 1);
  assert.equal(share(0), 0);
});

test("only a rate from 0 to 1 is taken", () => {
  for (const bad of [-0.1, 1.5, Number.NaN, "0.5", null, undefined]) assert.equal(validRate(bad), undefined);
  assert.equal(validRate(0.25), 0.25);
});

const meta = (): Item => ({ e: { type: 4 }, n: 10 });
const full = (n = 100): Item => ({ e: { type: 2 }, n });
const inc = (n = 10): Item => ({ e: { type: 3 }, n });

test("the rolling buffer keeps the last two checkouts, a snapshot's meta event with it", () => {
  const buf = [meta(), full(), inc(), inc(), meta(), full(), inc(), meta(), full()];
  const kept = trim(buf, 1_000_000);
  // From the second-to-last snapshot's meta event.
  assert.deepEqual(kept, buf.slice(4));
  assert.ok(size(kept) < size(buf));
  // Every kept buffer starts on something a player can rebuild from.
  const first = kept[0]?.e as { type: number };
  assert.ok(first.type === 4 || first.type === 2);
});

test("over the cap, only the latest checkout is kept", () => {
  const buf = [meta(), full(500), inc(400), meta(), full(200)];
  const kept = trim(buf, 1_000);
  assert.deepEqual(kept, buf.slice(3));
  assert.ok(size(kept) <= 1_000);
});

test("a buffer sent after a trigger is cut in chunks within the limits, in order", () => {
  const buf: Item[] = [full(25_000)];
  for (let i = 0; i < 700; i++) buf.push({ e: { type: 3, i }, n: 100 });
  const out = chunks(buf, 300, 30_000);
  assert.ok(out.length > 1);
  for (const c of out) assert.ok(c.length <= 300);
  assert.deepEqual(out.flat(), buf.map((x) => x.e), "nothing lost or reordered");
  // An event larger than a chunk is sent alone.
  assert.equal(chunks([inc(10), full(50_000), inc(10)], 300, 30_000).length, 3);
});

test("distribution-only events: drawn once per session, every session while the rate is unknown", () => {
  let draws = 0;
  const rand = () => {
    draws++;
    return 0.5;
  };
  const unknown = drawEvents(undefined, "s1", undefined, rand);
  assert.ok(unknown.on);
  assert.equal(drawEvents(unknown, "s1", 0.1, rand), unknown, "kept for the session");
  const out = drawEvents(unknown, "s2", 0.1, rand);
  assert.equal(out.on, false);
  assert.equal(drawEvents(out, "s2", 1, rand), out, "other tabs agree");
  assert.equal(draws, 1);
});

test("only vitals, scroll and fields are sampled, and they carry the rate", () => {
  const out = { sid: "s", on: false, r: 0.25 };
  const drawn = { sid: "s", on: true, r: 0.25 };
  for (const e of ["$vital", "$scroll", "$field"]) {
    assert.equal(sample(e, { a: 1 }, out), null, e);
    assert.equal(sample(e, { a: 1 }, drawn)?.$sample, 0.25, e);
    assert.equal(sample(e, { a: 1 }, { ...drawn, r: 1 })?.$sample, undefined, "no tag at rate 1");
  }
  for (const e of ["$identify", "$pageview", "$submit", "$feature", "$survey", "$error", "$request_failed", "signup"]) {
    assert.deepEqual(sample(e, { a: 1 }, out), { a: 1 }, e);
  }
  // A click is always sent; only its position is sampled.
  const click = { $tag: "button", $x: 0.5, $y: 10, $vw: 1280, $rage: true };
  assert.deepEqual(sample("$click", click, out), { $tag: "button", $rage: true });
  assert.deepEqual(sample("$click", click, drawn), click);
});
