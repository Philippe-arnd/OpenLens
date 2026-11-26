// Environment detection (D-012, D-034), under `node --test`.
import assert from "node:assert/strict";
import { test } from "node:test";

import { detectEnvironment } from "../src/env";

test("a host no pattern matches is unknown, not production", () => {
  for (const h of ["app.example.com", "kanban-test.philapps.com", "Shop.Example.COM"]) {
    assert.equal(detectEnvironment(h), "unknown", h);
    assert.notEqual(detectEnvironment(h), "production", h);
  }
});
