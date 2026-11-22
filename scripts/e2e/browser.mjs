// Browser end-to-end check: a page with one script tag reports to a real
// OpenLens server. Usage: node scripts/e2e/browser.mjs <server-url> <capture-key> <admin-token>
// Requires Playwright with Chromium installed next to this script (CI does
// `npm install playwright@1` in scripts/e2e; ESM ignores NODE_PATH).
import { chromium } from "playwright";
import http from "node:http";

const [server, key] = process.argv.slice(2);
const page = `<!doctype html><html><head><meta name="openlens-release" content="e2e-1"></head><body>
<button data-feature="export">Export</button>
<script src="${server}/c.js" data-key="${key}"></script>
<script>openlens.identify("e2e-user", { role: "owner" });</script>
</body></html>`;
const site = http.createServer((_, res) => res.end(page)).listen(0);
const port = site.address().port;

const browser = await chromium.launch();
const tab = await browser.newPage();
const sent = [];
tab.on("request", (r) => r.url().includes("/capture") && sent.push(r.postData()));
await tab.goto(`http://localhost:${port}/`);
await tab.click("button");
await tab.evaluate(() => { setTimeout(() => { throw new Error("e2e boom"); }, 0); });
await tab.waitForTimeout(200);
await tab.evaluate(() => openlens.flush());
await tab.waitForTimeout(500);
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
const need = ["$pageview", "$identify", "$click", "$error"];
const missing = need.filter((n) => !events.includes(n));
console.log("sent:", events.join(", "));
if (missing.length) {
  console.error("missing:", missing.join(", "));
  process.exit(1);
}
