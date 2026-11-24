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
<button id="mail">jean@exemple.fr</button>
<lens-card data-feature="share"></lens-card>
<script>customElements.define("lens-card", class extends HTMLElement {
  constructor() { super(); this.attachShadow({ mode: "open" }).innerHTML =
    '<button id="share-btn">Share</button><span data-feature="like"><button id="like-btn">Like</button></span>' +
    '<form name="invite" data-feature="invite"><input id="invite-email" name="email"><button id="invite-send">Send</button></form>';
    this.shadowRoot.querySelector("form").addEventListener("submit", (e) => e.preventDefault()); }
});</script>
<script src="${server}/c.js" data-key="${key}"></script>
<script>openlens.reset(); openlens.identify("e2e-user", { role: "owner" });</script>
</body></html>`;
// A page that loads OpenLens through the site's own domain (/_l/ → server),
// as with an ad-blocker proxy: no host attribute, only the key.
const proxied = `<!doctype html><html><body><button data-feature="export">Export</button>
<script src="/_l/c.js" data-key="${key}" data-replay="off"></script></body></html>`;
const site = http
  .createServer((req, res) => {
    if (req.url.startsWith("/_l/")) {
      const up = http.request(new URL(req.url.slice(3), server), { method: req.method, headers: { "content-type": req.headers["content-type"] || "text/plain" } }, (r) => {
        res.writeHead(r.statusCode, r.headers);
        r.pipe(res);
      });
      req.pipe(up);
      return;
    }
    res.end(req.url === "/proxied" ? proxied : page);
  })
  .listen(0);
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
  // A reset link: its token must never leave the page; campaign tags may.
  tab.goto(`http://localhost:${port}/?token=abc123&utm_source=e2e`),
]);
await tab.evaluate(() => openlens.flush());
await tab.waitForTimeout(300);
await tab.click("button[data-feature=export]");
// Hash routers: a #/route change is a page view of that route.
await tab.evaluate(() => { location.hash = "#/notes"; });
await tab.click("#secret");
await tab.click("#mail");
// Web components: clicks inside an open shadow root keep their feature.
await tab.click("#share-btn");
await tab.click("#like-btn");
// and a form inside it: typing then Enter submits it.
await tab.fill("#invite-email", "x@example.com");
// Leaving the field sends a `$field` event (#232): name and time, never the value.
await tab.locator("#invite-email").blur();
await tab.press("#invite-email", "Enter");
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

// The replay player (#170): markers for the session's events, the event
// list, keyboard control, on a desktop and on a phone.
if (admin) {
  const sid = sent.flatMap((b) => JSON.parse(b).batch).find((e) => e.session_id)?.session_id;
  const b = await chromium.launch();
  for (const [name, opts] of [["desktop", { viewport: { width: 1280, height: 900 } }], ["phone", { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true }]]) {
    const ctx = await b.newContext(opts);
    const p = await ctx.newPage();
    await p.goto(`${server}/login`);
    await p.fill("input[name=token]", admin);
    await Promise.all([p.waitForNavigation(), p.click("button[type=submit]")]);
    // Events become queryable after the server's flush (~10 s).
    let marks = 0;
    for (let i = 0; i < 8 && marks === 0; i++) {
      await p.goto(`${server}/p/default/replays/${sid}`);
      await p.waitForSelector("#ol-player .ol-controls", { timeout: 15000 });
      await p.waitForTimeout(800);
      marks = await p.locator("#ol-player .ol-mark").count();
      if (marks === 0) await p.waitForTimeout(2000);
    }
    const rows = await p.locator("#ol-player .ol-log li").count();
    const legend = await p.locator("#ol-player .ol-legend li").count();
    if (!marks || !rows || !legend) {
      console.error(`player (${name}): markers ${marks}, events ${rows}, legend ${legend}`);
      process.exit(1);
    }
    const overflow = await p.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    if (overflow > 1) {
      console.error(`player (${name}): the page scrolls sideways by ${overflow}px`);
      process.exit(1);
    }
    if (name === "desktop") {
      await p.keyboard.press("l");
      const t = await p.locator("#ol-player input[type=range]").inputValue();
      if (Number(t) <= 0) {
        console.error("player: L did not jump forward");
        process.exit(1);
      }
    }
    await p.screenshot({ path: `${process.env.RUNNER_TEMP ?? "/tmp"}/player-${name}.png`, fullPage: true });
    await ctx.close();
    console.log(`player (${name}): ${marks} markers, ${rows} events, ${legend} legend entries`);
  }
  await b.close();
}
// Through the proxy: the SDK posts to /_l/capture on the site itself.
{
  const b = await chromium.launch();
  const t = await b.newPage();
  const via = [];
  t.on("request", (r) => r.url().includes("/capture") && via.push(r.url()));
  await t.goto(`http://localhost:${port}/proxied`);
  await t.click("button[data-feature=export]");
  const [ok] = await Promise.all([
    t.waitForResponse((r) => r.url().includes("/_l/capture"), { timeout: 10000 }).catch(() => null),
    t.evaluate(() => openlens.flush()),
  ]);
  await b.close();
  if (!ok || ok.status() !== 200 || !via.every((u) => u.startsWith(`http://localhost:${port}/_l/capture`))) {
    console.error("proxied capture failed:", ok ? ok.status() : "no request", via);
    process.exit(1);
  }
  console.log("proxied capture ok");
}
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
if (sent.some((b) => b.includes("abc123") || b.includes("jean@exemple.fr"))) {
  console.error("a query-string token or an e-mail shown on the page was sent");
  process.exit(1);
}
if (!sent.some((b) => b.includes("utm_source=e2e"))) {
  console.error("campaign tags were dropped from the page URL");
  process.exit(1);
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
const clicks = sent.flatMap((b) => JSON.parse(b).batch).filter((e) => e.event === "$click");
for (const [feature, text] of [["share", "Share"], ["like", "Like"]]) {
  if (!clicks.some((c) => c.properties.$feature === feature && c.properties.$tag === "button" && c.properties.$text === text)) {
    console.error(`shadow DOM click not captured with its feature: ${feature}`);
    process.exit(1);
  }
}
const submits = sent.flatMap((b) => JSON.parse(b).batch).filter((e) => e.event === "$submit");
if (!submits.some((e) => e.properties.$form === "invite" && e.properties.$feature === "invite")) {
  console.error("shadow DOM form submit not captured");
  process.exit(1);
}
if (!sent.flatMap((b) => JSON.parse(b).batch).some((e) => e.event === "$field" && typeof e.properties.$ms === "number")) {
  console.error("leaving a form field sent no $field event");
  process.exit(1);
}
if (sent.some((b) => b.includes("x@example.com"))) {
  console.error("an input value was sent");
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
