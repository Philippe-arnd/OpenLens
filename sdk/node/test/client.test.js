import { test } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { OpenLens } from "../src/index.js";

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
        res.end(JSON.stringify({ flags: u.searchParams.get("role") === "admin" ? ["beta"] : [] }));
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
