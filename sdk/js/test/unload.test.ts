// The unload budget and the parked requests (#433), under `node --test`.
import assert from "node:assert/strict";
import { test } from "node:test";

import { PARK_CAP, PARK_TTL_MS, UNLOAD_BUDGET, UnloadBudget, bytes, park, takeParked, type Sender, type Store } from "../src/unload";

function memStore(): Store & { data: Map<string, string> } {
  const data = new Map<string, string>();
  return {
    data,
    getItem: (k) => data.get(k) ?? null,
    setItem: (k, v) => void data.set(k, v),
    removeItem: (k) => void data.delete(k),
  };
}

function sender(opts: { beacon?: boolean; store?: Store }) {
  const sent: string[] = [];
  const kept: string[] = [];
  const s: Sender = {
    beacon: opts.beacon === undefined ? undefined : (_u, b) => (opts.beacon ? (sent.push(b), true) : false),
    keepalive: (_u, b) => void kept.push(b),
    store: opts.store,
    now: () => 1_000_000,
  };
  return { s, sent, kept };
}

test("what fits the budget goes, what does not is parked for the next visit", () => {
  const store = memStore();
  const { s, sent } = sender({ beacon: true, store });
  const budget = new UnloadBudget();
  const events = "e".repeat(UNLOAD_BUDGET / 2);
  const replay = "r".repeat(UNLOAD_BUDGET / 2 + 10);
  assert.equal(budget.send(s, "https://h/capture", events), "sent");
  assert.equal(budget.send(s, "https://h/r", replay), "parked");
  assert.deepEqual(sent, [events]);
  const back = takeParked(store, 1_000_000 + 1);
  assert.equal(back.length, 1);
  assert.equal(back[0]?.body, replay);
  assert.equal(back[0]?.url, "https://h/r");
  // Taken once: nothing is sent twice by later visits.
  assert.equal(takeParked(store, 1_000_000 + 2).length, 0);
  // The page came back: a new unload has the whole budget again.
  budget.reset();
  assert.equal(budget.send(s, "https://h/r", replay), "sent");
});

test("a beacon the browser refuses is parked, not lost", () => {
  const store = memStore();
  const { s } = sender({ beacon: false, store });
  assert.equal(new UnloadBudget().send(s, "https://h/capture", "{}"), "parked");
  assert.equal(takeParked(store, 1_000_001).length, 1);
});

test("without storage (cookieless, no consent) nothing is written", () => {
  const { s, kept } = sender({ beacon: false });
  assert.equal(new UnloadBudget().send(s, "https://h/capture", "{}"), "dropped");
  assert.deepEqual(kept, ["{}"]);
});

test("parked requests stay under the cap, oldest first out, and expire", () => {
  const store = memStore();
  const chunk = "x".repeat(PARK_CAP / 4 + 1);
  for (let i = 0; i < 6; i++) park(store, { url: `https://h/r?${i}`, body: chunk, at: 1000 + i });
  const kept = takeParked(store, 2000);
  const total = kept.reduce((n, p) => n + bytes(p.body), 0);
  assert.ok(total <= PARK_CAP);
  assert.ok(kept.length < 6);
  assert.equal(kept[kept.length - 1]?.url, "https://h/r?5", "the newest is kept");
  park(store, { url: "https://h/r", body: "old", at: 0 });
  assert.equal(takeParked(store, PARK_TTL_MS + 1).length, 0);
});

test("sizes are counted in bytes, not characters", () => {
  assert.ok(bytes("é") > "é".length);
  assert.equal(takeParked({ getItem: () => "not json", setItem: () => {}, removeItem: () => {} }, 0).length, 0);
});
