// Browser end-to-end check: a page with one script tag reports to a real
// OpenLens server. Usage: node scripts/e2e/browser.mjs <server-url> <capture-key> <admin-token>
// Requires Playwright with Chromium installed next to this script (CI does
// `npm install playwright@1` in scripts/e2e; ESM ignores NODE_PATH).
import { chromium } from "playwright";
import http from "node:http";

const [server, key, admin] = process.argv.slice(2);
// A micro-survey shown after any click, to check the widget end to end.
if (admin) {
  const r = await fetch(`${server}/api/projects/default/surveys/e2e`, {
    method: "PUT",
    headers: { authorization: `Bearer ${admin}`, "content-type": "application/json" },
    body: JSON.stringify({ key: "e2e", question: "How was it?", kind: "rating", trigger: "$click", enabled: true, sample: 100 }),
  });
  if (!r.ok) throw new Error(`survey setup failed: ${r.status}`);
}
const page = `<!doctype html><html><head><meta name="openlens-release" content="e2e-1"></head><body>
<button data-feature="export">Export</button>
<div data-openlens-mask><button id="secret">Dear diary, private words</button></div>
<script src="${server}/c.js" data-key="${key}"></script>
<script>openlens.reset(); openlens.identify("e2e-user", { role: "owner" });</script>
</body></html>`;
const site = http.createServer((_, res) => res.end(page)).listen(0);
const port = site.address().port;

const browser = await chromium.launch();
const tab = await browser.newPage();
const sent = [];
tab.on("request", (r) => r.url().includes("/capture") && sent.push(r.postData()));
// The first capture request fails (offline): its events must be sent again.
let dropped = null;
await tab.route("**/capture*", (r) => {
  if (dropped === null) {
    dropped = r.request().postData();
    return r.abort();
  }
  return r.continue();
});
// Wait for the flags answer (it carries the surveys) before clicking.
await Promise.all([
  tab.waitForResponse((r) => r.url().includes("/flags?"), { timeout: 10000 }).catch(() => null),
  tab.goto(`http://localhost:${port}/`),
]);
await tab.evaluate(() => openlens.flush());
await tab.waitForTimeout(300);
await tab.click("button[data-feature=export]");
// Hash routers: a #/route change is a page view of that route.
await tab.evaluate(() => { location.hash = "#/notes"; });
await tab.click("#secret");
await tab.evaluate(() => { setTimeout(() => { throw new Error("e2e boom"); }, 0); });
await tab.waitForTimeout(200);
await tab.evaluate(() => openlens.flush());
await tab.waitForTimeout(500);
if (admin) {
  // The survey card appears after the click; answer 4.
  const four = tab.locator("#openlens-survey").locator("button", { hasText: "4" });
  await four.click({ timeout: 10000 });
  await tab.evaluate(() => openlens.flush());
  await tab.waitForTimeout(500);
}
// Replay: the recorder loads lazily and sends its first chunk within ~10 s.
const chunk = await tab
  .waitForResponse((r) => r.url().includes("/r?"), { timeout: 20000 })
  .catch(() => null);
await browser.close();
if (!chunk || chunk.status() !== 200) {
  console.error("replay chunk not stored:", chunk ? chunk.status() : "no request");
  process.exit(1);
}
console.log("replay chunk stored");
site.close();

const events = sent.flatMap((b) => JSON.parse(b).batch.map((e) => e.event));
if (admin) {
  const answered = sent.flatMap((b) => JSON.parse(b).batch).some((e) => e.event === "$survey" && e.properties.$answer === 4);
  if (!answered) {
    console.error("survey answer not sent");
    process.exit(1);
  }
  console.log("survey answered");
}
if (sent.some((b) => b.includes("private words"))) {
  console.error("masked text was sent");
  process.exit(1);
}
const resent = sent.slice(1).flatMap((b) => JSON.parse(b).batch.map((e) => e.uuid));
const lost = dropped ? JSON.parse(dropped).batch.filter((e) => !resent.includes(e.uuid)) : [{ event: "no failed request" }];
if (lost.length) {
  console.error("events of a failed request were lost:", lost.map((e) => e.event).join(", "));
  process.exit(1);
}
if (!sent.flatMap((b) => JSON.parse(b).batch).some((e) => e.event === "$pageview" && e.properties.$pathname === "/notes")) {
  console.error("hash route change not counted as a page view");
  process.exit(1);
}
// identify links the anonymous visit (reset while anonymous is a no-op), so
// the events sent before login belong to the user.
const all = sent.flatMap((b) => JSON.parse(b).batch);
const ident = all.find((e) => e.event === "$identify");
const anon = ident?.properties?.$anon_distinct_id;
const early = all.find((e) => e.distinct_id !== "e2e-user");
if (!anon || (early && early.distinct_id !== anon)) {
  console.error("identify does not link the anonymous visit:", anon, early?.distinct_id);
  process.exit(1);
}
const need = ["$pageview", "$identify", "$click", "$error"];
const missing = need.filter((n) => !events.includes(n));
console.log("sent:", events.join(", "));
if (missing.length) {
  console.error("missing:", missing.join(", "));
  process.exit(1);
}
