// Runtime health (#610). Its own file, so its own process: the process
// start is sent once per process, and no other test has used it yet.
import { test } from "node:test";
import assert from "node:assert/strict";
import os from "node:os";
import { OpenLens, RUNTIME_WINDOW_MS } from "../src/index.js";

function client() {
  const got = [];
  const fetch = async (_url, init) => {
    got.push(...JSON.parse(init.body).batch);
    return { ok: true, status: 200 };
  };
  return { got, ol: new OpenLens({ host: "http://x", key: "k", release: "v1", flushInterval: 60000, fetch }) };
}

test("runtime stats: one process start per process, then sane, bounded values per window", async () => {
  const a = client();
  a.ol.captureRuntime();
  a.ol.captureRuntime(); // again: nothing more
  const b = client();
  b.ol.express(); // a second client in the same process: no second start
  // Give the event-loop histogram something to measure.
  const until = Date.now() + 120;
  while (Date.now() < until) await new Promise((r) => setTimeout(r, 5));
  // Close the window as if ten minutes had passed, then shut down.
  a.ol.runtime.start -= RUNTIME_WINDOW_MS;
  await a.ol.flush();
  await a.ol.shutdown();
  await b.ol.shutdown();

  const all = [...a.got, ...b.got].filter((e) => e.event === "$runtime_stats");
  const starts = all.filter((e) => e.properties["process.start"] === true);
  assert.equal(starts.length, 1, "once per process");
  assert.ok(starts[0].properties["process.memory.usage"] > 0);

  const windows = a.got.filter((e) => e.event === "$runtime_stats" && !e.properties["process.start"]);
  assert.ok(windows.length >= 2, "the closed window, then the open one at shutdown");
  assert.ok(new Date(windows[0].timestamp) < new Date(windows[1].timestamp), "stamped with the window start");
  const total = os.totalmem();
  for (const w of windows) {
    const p = w.properties;
    assert.equal(w.distinct_id, "$server");
    assert.equal(p.$window_s, RUNTIME_WINDOW_MS / 1000, "10-minute windows");
    assert.equal(p.$release, "v1");
    assert.ok(p["process.memory.usage"] > 0 && p["process.memory.usage"] < total, "RSS in bytes, bounded");
    assert.ok(p["v8js.memory.heap.used"] > 0 && p["v8js.memory.heap.used"] < total, "heap in bytes, bounded");
  }
  const timed = windows.find((w) => w.properties["nodejs.eventloop.delay.p99"] !== undefined);
  assert.ok(timed, "the loop delay was measured");
  const { "nodejs.eventloop.delay.p50": p50, "nodejs.eventloop.delay.p99": p99 } = timed.properties;
  assert.ok(p50 >= 0 && p50 <= p99, "p50 at most p99");
  assert.ok(p99 < 60, "seconds, not nanoseconds");
  assert.equal(a.ol.runtime, undefined, "stopped at shutdown");
});
