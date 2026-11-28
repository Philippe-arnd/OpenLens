// The stats window the server advertises (#628, D-091). Its own file, so
// its own process: it uses the runtime stats, whose process start is sent
// once per process.
import { test } from "node:test";
import assert from "node:assert/strict";
import { OpenLens, RUNTIME_WINDOW_MS, STATS_WINDOW_HEADER, STATS_WINDOW_MS } from "../src/index.js";

const STATS = ["$request_stats", "$client_stats", "$runtime_stats"];
/** The stats that follow the stats window; runtime health has its own (#726). */
const ROUTE_STATS = ["$request_stats", "$client_stats"];

/** A client whose fake server advertises `advertised` (a header value, or none). */
function client(opts = {}) {
  const got = [];
  const server = { advertised: null };
  const fetch = async (_url, init) => {
    got.push(...JSON.parse(init.body).batch);
    const headers = new Headers();
    if (server.advertised !== null) headers.set(STATS_WINDOW_HEADER, server.advertised);
    return { ok: true, status: 200, headers };
  };
  const ol = new OpenLens({ host: "http://x", key: "k", flushInterval: 60000, fetch, ...opts });
  return { got, ol, server };
}

/** One request, one outgoing call, and a user event so a flush has something to send. */
function traffic(ol) {
  ol.recordRequest({ method: "GET", route: "/api/entries/:id", status: 200, ms: 12 });
  ol.recordCall({ method: "GET", host: "api.example.com", status: 200, ms: 30 });
  ol.capture("u1", "x");
}

/** The stats windows sent (process starts aside). */
function windows(got) {
  return got.filter((e) => STATS.includes(e.event) && !e.properties["process.start"]);
}

/** Every window says its own length, and starts on a multiple of it. */
function truthful(evs) {
  for (const e of evs) {
    const len = e.properties.$window_s * 1000;
    assert.ok([60000, 300000, 600000].includes(len), `${e.event}: ${e.properties.$window_s}`);
    assert.equal(Date.parse(e.timestamp) % len, 0, `${e.event} starts on its window`);
  }
}

test("the window follows the server's header from the next flush, each event with its own length", async () => {
  const { got, ol, server } = client();
  ol.captureRuntime();
  traffic(ol);
  server.advertised = "60";
  await ol.flush();
  // The header arrived with that flush: new windows are 1 minute long, and
  // the next flush closes the 10-minute ones, sent with their own length.
  traffic(ol);
  await ol.flush();
  const closed = windows(got);
  for (const name of ROUTE_STATS) {
    const e = closed.find((x) => x.event === name);
    assert.ok(e, `${name} of the 10-minute window sent on the next flush`);
    assert.equal(e.properties.$window_s, STATS_WINDOW_MS / 1000);
  }
  // Runtime health keeps its own 10-minute window (#726): not closed early.
  assert.ok(!closed.some((x) => x.event === "$runtime_stats"), "runtime window still open");
  await ol.shutdown();
  const all = windows(got);
  truthful(all);
  assert.ok(
    all.filter((e) => e.event === "$runtime_stats").every((e) => e.properties.$window_s === RUNTIME_WINDOW_MS / 1000),
    "runtime stats every 10 minutes whatever the stats window",
  );
  for (const name of ROUTE_STATS) {
    assert.ok(
      all.some((e) => e.event === name && e.properties.$window_s === 60),
      `${name} then in 1-minute windows`,
    );
  }
  // Nothing lost in the switch: both requests and both calls are counted.
  const count = (name) => all.filter((e) => e.event === name).reduce((n, e) => n + e.properties.count, 0);
  assert.equal(count("$request_stats"), 2);
  assert.equal(count("$client_stats"), 2);
});

test("statsWindowSeconds wins over the header, and an invalid one is ignored", async () => {
  const pinned = client({ statsWindowSeconds: 300 });
  pinned.server.advertised = "60";
  traffic(pinned.ol);
  await pinned.ol.flush();
  traffic(pinned.ol);
  await pinned.ol.shutdown();
  const sent = windows(pinned.got);
  assert.ok(sent.length > 0);
  truthful(sent);
  assert.ok(sent.every((e) => e.properties.$window_s === 300), "pinned to 5 minutes");

  const invalid = client({ statsWindowSeconds: 120 });
  invalid.server.advertised = "60";
  traffic(invalid.ol);
  await invalid.ol.flush();
  traffic(invalid.ol);
  await invalid.ol.shutdown();
  assert.ok(windows(invalid.got).some((e) => e.properties.$window_s === 60), "120 is ignored: the header applies");
});

test("a hostile or missing header keeps the window in force", async () => {
  const { got, ol, server } = client();
  for (const bad of ["120", "abc", "-60", "60000", ""]) {
    server.advertised = bad;
    traffic(ol);
    await ol.flush();
  }
  server.advertised = null;
  traffic(ol);
  await ol.flush();
  await ol.shutdown();
  const sent = windows(got);
  assert.ok(sent.length > 0);
  truthful(sent);
  assert.ok(sent.every((e) => e.properties.$window_s === STATS_WINDOW_MS / 1000));
});

test("runtime stats come every 10 minutes even with 1-minute stats windows (#726)", async () => {
  const realNow = Date.now;
  // A fake clock at the start of a 10-minute window, one minute a step.
  let now = Math.floor(realNow() / RUNTIME_WINDOW_MS) * RUNTIME_WINDOW_MS;
  Date.now = () => now;
  try {
    const { got, ol } = client({ statsWindowSeconds: 60 });
    ol.captureRuntime();
    for (let i = 0; i < 20; i++) {
      traffic(ol);
      now += 60000;
      await ol.flush();
    }
    const runtime = windows(got).filter((e) => e.event === "$runtime_stats");
    const requests = windows(got).filter((e) => e.event === "$request_stats");
    assert.ok(requests.length > runtime.length, "route stats follow the 1-minute window");
    assert.equal(runtime.length, 2, "one runtime window per 10 minutes");
    assert.ok(runtime.every((e) => e.properties.$window_s === RUNTIME_WINDOW_MS / 1000));
    await ol.shutdown();
  } finally {
    Date.now = realNow;
  }
});
