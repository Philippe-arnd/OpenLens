// Outgoing calls (#608). Its own file, so its own process: the
// diagnostics channels are global to the process.
import { test } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { OpenLens } from "../src/index.js";

function listen(handler, host) {
  return new Promise((resolve) => {
    const s = http.createServer(handler);
    if (host) s.listen(0, host, () => resolve(s));
    else s.listen(0, () => resolve(s));
  });
}

function httpGet(url) {
  return new Promise((resolve, reject) => {
    http
      .get(url, (res) => {
        res.resume();
        res.on("end", () => resolve(res.statusCode));
      })
      .on("error", reject);
  });
}

test("outgoing calls: fetch and http per host, never the path or query, never OpenLens's own", async () => {
  // The OpenLens server, called as `localhost` (every interface, so
  // whichever address `localhost` resolves to first).
  const got = [];
  const ol = await listen((req, res) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      if (req.url.startsWith("/capture")) got.push(...JSON.parse(body).batch);
      res.end(req.url.startsWith("/flags") ? '{"flags":[]}' : "{}");
    });
  });
  // An external API, on `127.0.0.1`: fetch gets 200, http gets 503.
  const api = await listen((req, res) => {
    res.statusCode = req.url.startsWith("/fail") ? 503 : 200;
    res.end("ok");
  }, "127.0.0.1");
  // A port nobody listens on: a call without a response.
  const gone = await listen(() => {}, "127.0.0.1");
  const gonePort = gone.address().port;
  await new Promise((r) => gone.close(r));

  const client = new OpenLens({ host: `http://localhost:${ol.address().port}`, key: "k", release: "v1", flushInterval: 60000 });
  client.captureOutgoing();
  client.captureOutgoing(); // again: nothing more

  const base = `http://127.0.0.1:${api.address().port}`;
  const r = await fetch(`${base}/v1/charges/ch_123?secret=s3cr3t`);
  await r.text();
  assert.equal(r.status, 200);
  assert.equal(await httpGet(`${base}/fail/acct_42?token=t0k3n`), 503);
  await assert.rejects(fetch(`http://127.0.0.1:${gonePort}/x`));
  // OpenLens's own calls: a flag check and a flush.
  await client.isEnabled("beta", "u1");
  client.capture("u1", "paid");
  await client.flush();
  await client.shutdown();
  ol.close();
  api.close();

  const calls = got.filter((e) => e.event === "$client_stats");
  assert.ok(calls.length >= 3, `fetch, http and the failed call: ${JSON.stringify(calls)}`);
  for (const e of calls) {
    const p = e.properties;
    assert.equal(e.distinct_id, "$server");
    assert.equal(p.$metric, "http.client.request.duration");
    assert.equal(p["server.address"], "127.0.0.1", "the host only, and never OpenLens's own");
    assert.equal(p.bucket_counts.reduce((a, b) => a + b, 0), p.count);
  }
  const text = JSON.stringify(calls);
  for (const secret of ["/v1/charges", "ch_123", "s3cr3t", "/fail", "acct_42", "t0k3n", "localhost"]) {
    assert.ok(!text.includes(secret), `${secret} never leaves the app`);
  }
  const status = (s) => calls.filter((e) => e.properties["http.response.status_code"] === s);
  assert.ok(status(200).length > 0, "fetch is captured");
  assert.ok(status(503).length > 0, "http is captured");
  const failed = status(0);
  assert.ok(failed.length > 0, "a call without a response is counted");
  assert.ok(failed.every((e) => typeof e.properties["error.type"] === "string" && e.properties["error.type"].length <= 64));
  assert.ok(status(200).every((e) => e.properties["error.type"] === undefined));
});
