// Web Vitals attribution (#467), under `node --test`.
import assert from "node:assert/strict";
import { test } from "node:test";

import { MAX_SELECTOR, deviceClass, hidden, inpParts, lcpParts, resourcePath, selector, stable, type El } from "../src/attribution";

/** A fake element: tag, attributes, parent. */
function node(tag: string, attrs: Record<string, string> = {}, parent: El | null = null): El {
  return {
    tagName: tag.toUpperCase(),
    id: attrs.id ?? "",
    parentElement: parent,
    getAttribute: (n) => attrs[n] ?? null,
    hasAttribute: (n) => n in attrs,
  };
}

test("generated class names and ids are dropped, hand-written ones kept", () => {
  for (const c of ["hero", "btn-primary", "card__title", "heroImage"]) assert.ok(stable(c), c);
  for (const c of ["css-1x2y3z", "sc-AxjAm", "jsx-123", "Hero_title__aBcDe", "mt-4", "a1b2c3"]) assert.ok(!stable(c), c);
  const img = node("img", { class: "css-1x2y3z hero-img sc-AxjAm", id: "r12" }, node("div", { class: "hero" }));
  const s = selector(img);
  assert.ok(s.includes("hero-img") && !s.includes("css-") && !s.includes("sc-") && !s.includes("r12"), s);
});

test("a selector stops at an id and stays short on deep, long-classed trees", () => {
  const page = node("main", { id: "content" }, node("body"));
  const img = node("img", { class: "hero" }, node("div", { class: "banner" }, page));
  assert.ok(selector(img).startsWith("main#content"), selector(img));
  assert.ok(!selector(img).includes("body"));
  let deep: El = node("html");
  const long = "a".repeat(39);
  for (let i = 0; i < 20; i++) deep = node("section", { class: `${long}x ${long}y` }, deep);
  assert.ok(selector(deep).length <= MAX_SELECTOR, selector(deep));
});

test("the masking rules of autocapture apply", () => {
  const ignored = node("img", { class: "hero" }, node("div", { "data-openlens-ignore": "" }));
  assert.equal(selector(ignored), "");
  const masked = node("p", { id: "account", class: "balance" }, node("div", { "data-openlens-mask": "" }));
  const s = selector(masked);
  assert.ok(!s.includes("account") && !s.includes("balance") && s.endsWith("p"), s);
  // vitals.ts drops the resource path of both.
  assert.ok(hidden(ignored) > 0 && hidden(masked) > 0 && hidden(node("img")) === 0);
});

test("resource paths lose their query, hash and inline data", () => {
  const page = "https://app.example.com/pricing?token=secret";
  assert.equal(resourcePath("/img/hero.jpg?sig=abc#x", page), "/img/hero.jpg");
  const cdn = resourcePath("https://cdn.example.net/hero.webp?u=[EMAIL_REDACTED]", page);
  assert.ok(cdn.includes("cdn.example.net") && !cdn.includes("?") && !cdn.includes("@"), cdn);
  assert.equal(resourcePath("data:image/png;base64,AAAA", page), "data:");
});

test("the LCP parts add up to LCP, and a late image shows as load delay", () => {
  const late = lcpParts(5000, 400, 3500, 3900);
  assert.equal(late.reduce((a, b) => a + b, 0), 5000);
  assert.ok(late[1]! > late[0]! && late[1]! > late[2]! && late[1]! > late[3]!, String(late));
  // Text: no resource, the wait after the first byte is render delay.
  const text = lcpParts(3000, 500);
  assert.ok(text[1] === 0 && text[2] === 0 && text[3]! > 0, String(text));
  // Out-of-order timings never give a negative part.
  assert.ok(lcpParts(1000, 1500, 200, 100).every((x) => x >= 0));
});

test("the interaction parts add up to its duration", () => {
  const p = inpParts(100, 400, 450, 420);
  assert.equal(p.reduce((a, b) => a + b, 0), 420);
  assert.ok(p[0]! > p[1]! && p[0]! > p[2]!, String(p));
  assert.ok(inpParts(100, 50, 40, 30).every((x) => x >= 0));
});

test("a touch screen is a phone or a tablet by its size, a mouse is a desktop", () => {
  assert.equal(deviceClass(false, 400), "desktop");
  assert.notEqual(deviceClass(true, 390), deviceClass(true, 820));
  assert.notEqual(deviceClass(true, 820), "desktop");
});
