import { test } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { OpenLens, detectRelease, requestContext } from "../src/index.js";

function server(handler) {
  return new Promise((resolve) => {
    const s = http.createServer(handler).listen(0, () => resolve(s));
  });
}

test("batches events with key, release and role, and checks flags", async () => {
  const got = [];
  const s = await server((req, res) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      if (req.url.startsWith("/capture")) {
        got.push({ key: req.headers["x-openlens-key"], body: JSON.parse(body) });
        res.end('{"accepted":3}');
      } else if (req.url.startsWith("/flags")) {
        const u = new URL(req.url, "http://x");
        const flags = u.searchParams.get("role") === "admin" ? ["beta"] : [];
        if (u.searchParams.get("plan") === "pro" && u.searchParams.get("p.seats") === "12") flags.push("pro-only");
        res.end(JSON.stringify({ flags }));
      }
    });
  });
  const ol = new OpenLens({ host: `http://localhost:${s.address().port}/`, key: "pk_t", release: "v9", flushInterval: 60000 });
  ol.identify("u1", { role: "admin" });
  ol.capture("u1", "invoice_sent", { amount_bucket: "100-500" });
  ol.feature("u1", "export");
  await ol.shutdown();
  assert.equal(got.length, 1);
  assert.equal(got[0].key, "pk_t");
  const evs = got[0].body.batch;
  assert.deepEqual(evs.map((e) => e.event), ["$identify", "invoice_sent", "$feature"]);
  assert.equal(evs[0].properties.$role, "admin");
  assert.equal(evs[1].properties.$release, "v9");
  assert.equal(evs[2].properties.$feature, "export");
  assert.equal(await ol.isEnabled("beta", "u1", { role: "admin" }), true);
  assert.equal(await ol.isEnabled("beta", "u2", { role: "member" }), false);
  assert.equal(await ol.isEnabled("pro-only", "u1", { plan: "pro", properties: { Seats: 12 } }), true);
  assert.equal(await ol.isEnabled("pro-only", "u1", { plan: "free", properties: { seats: 12 } }), false);
  s.close();
});

test("never throws when the server is down", async () => {
  const errors = [];
  const ol = new OpenLens({ host: "http://127.0.0.1:9", key: "pk_t", onError: (e) => errors.push(e), flushInterval: 60000 });
  ol.capture("u1", "x");
  await ol.shutdown();
  assert.equal(await ol.isEnabled("beta", "u1"), false);
  assert.ok(errors.length >= 1);
});

test("release defaults to the host's commit, shortened, and an explicit one wins", () => {
  const sha = "0123456789ABCDEF0123456789abcdef01234567";
  const short = sha.slice(0, 12).toLowerCase();
  assert.equal(detectRelease({ GITHUB_SHA: sha }), short);
  assert.equal(detectRelease({ RENDER_GIT_COMMIT: "abc", GITHUB_SHA: sha, npm_package_version: "1.2.3" }), short, "earlier variable first");
  assert.equal(detectRelease({ SOURCE_COMMIT: "", npm_package_version: "1.2.3" }), "1.2.3");
  assert.equal(detectRelease({}), undefined);
  const saved = process.env.SOURCE_COMMIT;
  process.env.SOURCE_COMMIT = sha;
  try {
    const auto = new OpenLens({ host: "http://x", key: "k", flushInterval: 60000 });
    const explicit = new OpenLens({ host: "http://x", key: "k", release: "v2", flushInterval: 60000 });
    clearInterval(auto.timer);
    clearInterval(explicit.timer);
    assert.equal(auto.release, short);
    assert.equal(explicit.release, "v2");
  } finally {
    if (saved === undefined) delete process.env.SOURCE_COMMIT;
    else process.env.SOURCE_COMMIT = saved;
  }
});

function collector() {
  const got = [];
  return server((req, res) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      if (req.url.startsWith("/capture")) got.push(...JSON.parse(body).batch);
      res.end("{}");
    });
  }).then((s) => ({ s, got, host: `http://localhost:${s.address().port}` }));
}

test("a server exception is a $error of kind server, grouped by the app's own frame", async () => {
  const { s, got, host } = await collector();
  const ol = new OpenLens({ host, key: "k", flushInterval: 60000 });
  const err = new TypeError("boom 42");
  err.stack = "TypeError: boom 42\n    at x (node:internal/foo:1:2)\n    at y (/app/node_modules/lib/i.js:3:4)\n    at handler (/app/dist/routes/entries.js:12:5)";
  ol.captureException(err, { sessionId: "s1", traceId: "a".repeat(32), route: "/api/entries/:id" });
  ol.captureException(err); // the same object is reported once
  for (let i = 0; i < 10; i++) ol.captureException(new Error("same"));
  await ol.shutdown();
  s.close();
  const errs = got.filter((e) => e.event === "$error");
  const boom = errs.filter((e) => e.properties.$message.includes("boom"));
  assert.equal(boom.length, 1);
  assert.equal(boom[0].properties.$kind, "server");
  assert.equal(boom[0].properties.$source, "/app/dist/routes/entries.js:12:5");
  assert.equal(boom[0].session_id, "s1");
  assert.equal(boom[0].distinct_id, "$server");
  assert.ok(errs.filter((e) => e.properties.$message === "Error: same").length < 10, "the same message is damped");
});

test("requests are aggregated per route and status class, never one event each", async () => {
  const { s, got, host } = await collector();
  const ol = new OpenLens({ host, key: "k", flushInterval: 60000 });
  for (let i = 0; i < 50; i++) ol.recordRequest({ method: "get", route: "/api/entries/:id", status: 200, ms: i });
  ol.recordRequest({ method: "GET", route: "/api/entries/:id", status: 503, ms: 9000 });
  ol.recordRequest({ method: "POST", route: "/api/login", status: 500, ms: 3 });
  await ol.shutdown();
  s.close();
  const stats = got.filter((e) => e.event === "$request_stats");
  assert.ok(stats.length < 51, "aggregated");
  const ok = stats.find((e) => e.properties.$route === "/api/entries/:id" && e.properties.$status_class === "2xx");
  const bad = stats.find((e) => e.properties.$route === "/api/entries/:id" && e.properties.$status_class === "5xx");
  assert.equal(ok.properties.$count, 50);
  assert.equal(ok.properties.$buckets.reduce((a, b) => a + b, 0), 50);
  assert.ok(bad.properties.$errors > 0 && ok.properties.$errors === 0);
  assert.ok(bad.properties.$max_ms > ok.properties.$max_ms);
  assert.ok(stats.every((e) => e.distinct_id === "$server"));
});

test("the Express middleware records the route template and reads trace context", async () => {
  const { s, got, host } = await collector();
  const ol = new OpenLens({ host, key: "k", flushInterval: 60000 });
  const mw = ol.express({ user: () => "u7" });
  const handlers = {};
  const res = { statusCode: 500, on: (ev, f) => (handlers[ev] = f) };
  const req = {
    method: "POST",
    baseUrl: "/api",
    route: { path: "/entries/:id" },
    headers: { traceparent: `00-${"b".repeat(32)}-${"c".repeat(16)}-01`, baggage: "openlens-session=s9, openlens-distinct=anon%201, other=x" },
  };
  mw(req, res, () => {});
  assert.deepEqual({ ...req.openlens, user: undefined }, { traceId: "b".repeat(32), sessionId: "s9", distinctId: "anon 1", user: undefined });
  ol.expressErrors()(new Error("db down"), req, res, () => {});
  handlers.finish();
  // A raw URL never becomes a route.
  const req2 = { method: "GET", originalUrl: "/users/123", headers: {} };
  const h2 = {};
  ol.express()(req2, { statusCode: 404, on: (ev, f) => (h2[ev] = f) }, () => {});
  h2.finish();
  await ol.shutdown();
  s.close();
  const err = got.find((e) => e.event === "$error");
  assert.equal(err.distinct_id, "u7");
  assert.equal(err.session_id, "s9");
  assert.equal(err.properties.$trace_id, "b".repeat(32));
  assert.equal(err.properties.$route, "/api/entries/:id");
  const routes = got.filter((e) => e.event === "$request_stats").map((e) => e.properties.$route);
  assert.ok(routes.includes("/api/entries/:id") && routes.includes("(unmatched)"));
  assert.ok(!routes.some((r) => r.includes("123")));
});

test("hostile trace headers are ignored", () => {
  assert.deepEqual(requestContext({ traceparent: `00-${"0".repeat(32)}-${"c".repeat(16)}-01` }), {});
  assert.deepEqual(requestContext({ traceparent: "garbage", baggage: "openlens-session=%E0%A4%A" }), {});
  assert.equal(requestContext({ baggage: `openlens-session=${"x".repeat(5000)}` }).sessionId.length, 200);
});
