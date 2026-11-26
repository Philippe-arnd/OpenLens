// Environment detection (D-012, D-034) and the build fingerprint (#495),
// under `node --test`.
import assert from "node:assert/strict";
import { test } from "node:test";

import { buildFingerprint, detectEnvironment } from "../src/env";

test("a host no pattern matches is unknown, not production", () => {
  for (const h of ["app.example.com", "kanban-test.philapps.com", "Shop.Example.COM"]) {
    assert.equal(detectEnvironment(h), "unknown", h);
    assert.notEqual(detectEnvironment(h), "production", h);
  }
});

const ORIGIN = "https://app.example.com";
const HOST = "https://lens.example.com";
const PAGE = [
  `${ORIGIN}/assets/index-3f9a1c.js`,
  "/assets/index-77b2e0.css",
  "https://cdn.other.com/lib.js",
  `${HOST}/c.js`,
];

test("the build fingerprint changes with a deploy and with nothing else", () => {
  const fp = buildFingerprint(PAGE, ORIGIN, HOST);
  assert.ok(fp && /^[0-9a-f]{8}$/.test(fp), String(fp));
  // Same build: order, duplicates, other origins and OpenLens's own bundles
  // (here proxied on the app's origin) do not count.
  const same = [...PAGE].reverse().concat(PAGE[0] ?? "", "https://cdn.other.com/v2.js", `${ORIGIN}/_l/r.js`);
  assert.equal(buildFingerprint(same, ORIGIN, `${ORIGIN}/_l/`), fp);
  // A new content hash is a new build.
  const deployed = PAGE.map((u) => u.replace("3f9a1c", "a41d07"));
  assert.notEqual(buildFingerprint(deployed, ORIGIN, HOST), fp);
  // A cache-busting query string is part of the build too.
  assert.notEqual(
    buildFingerprint([...PAGE, "/app.js?v=2"], ORIGIN, HOST),
    buildFingerprint([...PAGE, "/app.js?v=1"], ORIGIN, HOST),
  );
});

test("a page with no file of its own has no fingerprint", () => {
  assert.equal(buildFingerprint([], ORIGIN), undefined);
  assert.equal(buildFingerprint(["https://cdn.other.com/lib.js", `${HOST}/c.js`, "http://[bad"], ORIGIN, HOST), undefined);
});
