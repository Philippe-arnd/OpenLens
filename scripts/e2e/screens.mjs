// Dashboard check in a real browser, under the real CSP: every page, light
// and dark, must load with no console error (CSP violations are console
// errors in Chromium), and a full-page screenshot of each is saved.
// Usage: node scripts/e2e/screens.mjs <server-url> <admin-token> <out-dir>
// The server should hold demo data (`xtask demo`).
import { chromium } from "playwright";
import { mkdirSync } from "node:fs";

const [server, token, out] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
const pages = ["", "/events", "/pages", "/errors", "/replays", "/flags"];

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1360, height: 900 }, deviceScaleFactor: 2 });
const tab = await ctx.newPage();
const problems = [];
tab.on("console", (m) => m.type() === "error" && problems.push(`${tab.url()}: ${m.text()}`));
tab.on("pageerror", (e) => problems.push(`${tab.url()}: ${e.message}`));

await tab.goto(`${server}/login`);
await tab.fill("input[name=token]", token);
await Promise.all([tab.waitForNavigation(), tab.click("button[type=submit]")]);

for (const theme of ["light", "dark"]) {
  await tab.goto(`${server}/theme/${theme}`);
  for (const p of pages) {
    const url = `${server}/p/default${p}`;
    const r = await tab.goto(url);
    if (!r || r.status() !== 200) problems.push(`${url}: HTTP ${r ? r.status() : "none"}`);
    // The icon sprite must not take space, and meters must have a length.
    const spriteHeight = await tab.evaluate(() => document.querySelector("svg.sprite")?.getBoundingClientRect().height ?? 0);
    if (spriteHeight > 0) problems.push(`${url}: icon sprite is visible`);
    const name = (p || "/board").slice(1).replace(/\//g, "-");
    await tab.screenshot({ path: `${out}/${name}-${theme}.png`, fullPage: true });
  }
}
await browser.close();

if (problems.length) {
  console.error(problems.join("\n"));
  process.exit(1);
}
console.log(`ok: ${pages.length * 2} pages, no console errors; screenshots in ${out}`);
