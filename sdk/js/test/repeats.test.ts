// One error and one failed request per session and fingerprint (#703),
// under `node --test`.
import assert from "node:assert/strict";
import { test } from "node:test";

import { Repeats, errorKey } from "../src/repeats";

test("an error thrown 1,000 times sends one event and one count", () => {
  const r = new Repeats();
  const key = errorKey("error", "boom", "Error: boom\n    at f (https://app.example.com/a.js:1:20)");
  let sent = 0;
  for (let i = 0; i < 1000; i++) if (r.first("$error", key, "s1", { $message: "boom" })) sent++;
  assert.equal(sent, 1);
  const out = r.take();
  assert.equal(out.length, 1);
  assert.deepEqual(out[0], { sid: "s1", event: "$error", props: { $message: "boom", $repeat: true, $repeats: 999 } });
  // Nothing more until it happens again.
  assert.equal(r.take().length, 0);
  assert.equal(r.first("$error", key, "s1", {}), false);
  assert.equal(r.take()[0]?.props.$repeats, 1);
});

test("a new session, another fingerprint or another event is sent again", () => {
  const r = new Repeats();
  assert.ok(r.first("$error", "a", "s1", {}));
  assert.ok(r.first("$error", "b", "s1", {}));
  assert.ok(r.first("$request_failed", "a", "s1", {}));
  assert.ok(r.first("$error", "a", "s2", {}));
  assert.equal(r.take().length, 0, "no repeat, no count event");
});

test("the fingerprint is the message and the first frame", () => {
  const at = (line: number) => `Error: boom\n    at f (https://app.example.com/a.js:${line}:2)\n    at g (a.js:9:9)`;
  assert.equal(errorKey("error", "boom", at(1)), errorKey("error", "boom", at(1)));
  assert.notEqual(errorKey("error", "boom", at(1)), errorKey("error", "boom", at(2)));
  assert.notEqual(errorKey("error", "boom"), errorKey("error", "bang"));
});

test("memory is bounded: past the cap new fingerprints are sent every time", () => {
  const r = new Repeats();
  for (let i = 0; i < 1000; i++) r.first("$error", `k${i}`, "s1", {});
  assert.ok(r.first("$error", "k999", "s1", {}), "not remembered");
  assert.equal(r.first("$error", "k0", "s1", {}), false, "remembered");
});
