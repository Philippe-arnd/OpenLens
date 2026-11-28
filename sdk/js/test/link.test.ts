// One visitor across the project's own hosts (#710), under `node --test`.
import assert from "node:assert/strict";
import { test } from "node:test";

import { ANON_PARAM, MAX_SISTERS, mayAdopt, takeAnon, validAnon, validSisters, withAnon } from "../src/link";

const ID = "3f2b8c1e-9a4d-4e7b-8c2a-1d5e6f7a8b9c";
const SISTERS = ["app.example.com"];

test("a link to a sister host carries the id, other links are left alone", () => {
  const out = withAnon("https://app.example.com/signup?plan=pro#top", "https://www.example.com/", ID, SISTERS);
  assert.ok(out, "sister host");
  const u = new URL(out);
  assert.equal(u.searchParams.get(ANON_PARAM), ID);
  assert.equal(u.searchParams.get("plan"), "pro", "the link's own parameters stay");
  assert.equal(u.hash, "#top", "and its fragment");
  assert.equal(withAnon("https://other.com/", "https://www.example.com/", ID, SISTERS), null, "foreign host");
  assert.equal(withAnon("/pricing", "https://www.example.com/", ID, ["www.example.com"]), null, "same host");
  assert.equal(withAnon("mailto:a@app.example.com", "https://www.example.com/", ID, SISTERS), null);
  assert.equal(withAnon("https://app.example.com/", "https://www.example.com/", ID, []), null, "no sisters");
  assert.equal(withAnon("https://app.example.com/", "https://www.example.com/", "$cookieless", SISTERS), null);
});

test("a landing URL gives back a valid id and loses the parameter", () => {
  const got = takeAnon(`https://app.example.com/signup?plan=pro&${ANON_PARAM}=${ID}#/step`);
  assert.equal(got?.id, ID);
  assert.ok(got && !got.clean.includes(ANON_PARAM), got?.clean);
  assert.ok(got?.clean.includes("plan=pro") && got.clean.endsWith("#/step"), got?.clean);
  assert.equal(takeAnon("https://app.example.com/signup?plan=pro"), null, "nothing to take: the address is left alone");
});

test("a forged or oversized id is dropped, and still removed from the address", () => {
  for (const bad of ["x", `${ID}0`, "<script>", "a".repeat(5000), ID.replace(/-/g, "")]) {
    const got = takeAnon(`https://app.example.com/?${ANON_PARAM}=${encodeURIComponent(bad)}`);
    assert.equal(got?.id, null, bad.slice(0, 20));
    assert.ok(got && !got.clean.includes(ANON_PARAM));
    assert.ok(!validAnon(bad));
  }
});

test("only a visitor with no history here adopts the landing's id", () => {
  assert.ok(mayAdopt({ last: 0 }));
  assert.ok(!mayAdopt({ last: 1_700_000_000_000 }), "already tracked on this host");
  assert.ok(!mayAdopt({ uid: "u1", last: 0 }), "identified");
});

test("sister hosts from the server are bounded and cleaned", () => {
  const many = Array.from({ length: MAX_SISTERS + 5 }, (_, i) => `h${i}.example.com`);
  assert.ok(validSisters(many).length <= MAX_SISTERS);
  assert.deepEqual(validSisters(["App.Example.com", 3, "bad host/", null]), ["app.example.com"]);
  assert.deepEqual(validSisters("app.example.com"), []);
});
