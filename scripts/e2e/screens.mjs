// Dashboard check in a real browser, under the real CSP: every page, light
// and dark, must load with no console error (CSP violations are console
// errors in Chromium), and a full-page screenshot of each is saved.
// Usage: node scripts/e2e/screens.mjs <server-url> <admin-token> <out-dir>
// The server should hold demo data (`xtask demo`).
import { chromium } from "playwright";
import { mkdirSync } from "node:fs";
import { createRequire } from "node:module";

// axe-core is injected by this test only (the dashboard ships no script).
const axePath = createRequire(import.meta.url).resolve("axe-core/axe.min.js");

const [server, token, out] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
const pages = ["", "/events", "/pages", "/errors", "/replays", "/flags", "/surveys", "/digest", "/audience", "/flows", "/features", "/settings", "/breakdown?by=role", "/acquisition", "/churn", "/live", "/uptime", "/goals", "/revenue", "/forms"];
// Pages outside a project (the owner is an admin, so /users is allowed).
const global = ["/projects", "/users", "/welcome"];

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1360, height: 900 }, deviceScaleFactor: 2, reducedMotion: "reduce" });
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
    // Matrix cells must carry their heat class (one class attribute only).
    const heat = await tab.evaluate(() => document.querySelectorAll("table.matrix td[class*=heat-]").length);
    if (p === "" && heat === 0) problems.push(`${url}: roles × features cells have no heat class`);
    const spriteHeight = await tab.evaluate(() => document.querySelector("svg.sprite")?.getBoundingClientRect().height ?? 0);
    if (spriteHeight > 0) problems.push(`${url}: icon sprite is visible`);
    const name = (p || "/board").slice(1).replace(/[\/?=]/g, "-");
    await tab.screenshot({ path: `${out}/${name}-${theme}.png`, fullPage: true });
  }
}
// Sidebar collapsed to icons (#170): remembered by a cookie, no script.
await tab.goto(`${server}/theme/light`);
await tab.goto(`${server}/p/default/events`);
await tab.goto(`${server}/nav/collapsed`);
const rail = await tab.evaluate(() => document.querySelector(".side")?.getBoundingClientRect().width ?? 0);
if (rail === 0 || rail > 80) problems.push(`sidebar did not collapse (width ${rail})`);
const current = await tab.locator('.side-nav a[aria-current="page"]').count();
if (current !== 1) problems.push(`the current page is not marked in the sidebar (${current})`);
await tab.screenshot({ path: `${out}/events-collapsed-light.png`, fullPage: true });
await tab.goto(`${server}/nav/expanded`);
// The sidebar fits without scrolling at laptop sizes (#402): Collapse stays on screen.
for (const [w, h] of [[1366, 768], [1280, 720]]) {
  await tab.setViewportSize({ width: w, height: h });
  await tab.goto(`${server}/p/default/events`);
  const fit = await tab.evaluate(() => {
    const side = document.querySelector(".side");
    const c = document.querySelector(".side .collapse")?.getBoundingClientRect();
    return { scrolls: side ? side.scrollHeight > side.clientHeight + 1 : true, visible: !!c && c.bottom <= innerHeight && c.height > 0 };
  });
  if (fit.scrolls || !fit.visible) problems.push(`sidebar does not fit at ${w}x${h} (scrolls: ${fit.scrolls}, collapse visible: ${fit.visible})`);
  const tabs = await tab.locator('nav.section-tabs a[aria-current="page"]').count();
  if (tabs !== 1) problems.push(`the current tab is not marked at ${w}x${h} (${tabs})`);
}
for (const g of global) {
  const r = await tab.goto(`${server}${g}`);
  if (!r || r.status() !== 200) problems.push(`${g}: HTTP ${r ? r.status() : "none"}`);
  await tab.screenshot({ path: `${out}${g.replace("/", "/global-")}.png`, fullPage: true });
}

// Phones (#170): every page at 390 px wide must not scroll sideways (wide
// tables scroll inside their card), with a screenshot of each.
const phone = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true, reducedMotion: "reduce" });
const pt = await phone.newPage();
await pt.goto(`${server}/login`);
await pt.fill("input[name=token]", token);
await Promise.all([pt.waitForNavigation(), pt.click("button[type=submit]")]);
for (const theme of ["light", "dark"]) {
  await pt.goto(`${server}/theme/${theme}`);
  for (const p of [...pages.map((x) => `/p/default${x}`), ...global]) {
    await pt.goto(`${server}${p}`);
    const over = await pt.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    if (over > 1) {
      // Name the outermost elements that stick out, to fix them quickly.
      const culprits = await pt.evaluate(() => {
        const w = document.documentElement.clientWidth;
        const out = [];
        for (const el of document.querySelectorAll("body *")) {
          const r = el.getBoundingClientRect();
          // Skip what a scrolling box clips (it does not widen the page).
          let clipped = false;
          for (let a = el.parentElement; a; a = a.parentElement) {
            const s = getComputedStyle(a);
            if (s.overflowX === "auto" || s.overflowX === "hidden" || s.overflowX === "scroll") { clipped = true; break; }
          }
          if (!clipped && r.right > w + 1 && !(el.parentElement && el.parentElement.getBoundingClientRect().right > w + 1)) {
            out.push(`${el.tagName.toLowerCase()}${el.className && typeof el.className === "string" ? "." + el.className.trim().replace(/\s+/g, ".") : ""} (${Math.round(r.right - w)}px)`);
          }
        }
        return out.slice(0, 4).join(", ");
      });
      problems.push(`${p} (phone, ${theme}): the page scrolls sideways by ${over}px: ${culprits}`);
    }
    await pt.screenshot({ path: `${out}/phone${p.replace("/p/default", "").replace(/[\/?=]/g, "-") || "-board"}-${theme}.png`, fullPage: true });
  }
}
await phone.close();

// Accessibility (#161): axe-core on every page, light and dark. Injecting a
// script needs a context that bypasses the CSP; the checks above ran under it.
const advice = [];
const a11y = await browser.newContext({ viewport: { width: 1360, height: 900 }, bypassCSP: true, reducedMotion: "reduce" });
const at = await a11y.newPage();
await at.goto(`${server}/login`);
const loginIssues = await audit(at, "/login");
await at.fill("input[name=token]", token);
await Promise.all([at.waitForNavigation(), at.click("button[type=submit]")]);
const found = [...loginIssues];
for (const theme of ["light", "dark"]) {
  await at.goto(`${server}/theme/${theme}`);
  for (const p of [...pages.map((x) => `/p/default${x}`), ...global]) {
    await at.goto(`${server}${p}`);
    found.push(...(await audit(at, `${p} (${theme})`)));
  }
}
// WCAG 2.2 AA (#303): every impact fails, minor included. Best practices
// are listed as warnings and do not fail the run.
async function audit(page, label) {
  await page.addScriptTag({ path: axePath });
  const res = await page.evaluate(() =>
    window.axe.run(document, { runOnly: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"], resultTypes: ["violations"] }),
  );
  const line = (v) => `${label}: ${v.id} (${v.impact}) x${v.nodes.length}: ${v.help}; e.g. ${v.nodes[0]?.target?.join(" ")}`;
  const wcag = (v) => v.tags.some((t) => /^wcag2(1|2)?a{1,2}$/.test(t));
  advice.push(...res.violations.filter((v) => !wcag(v)).map(line));
  return res.violations.filter(wcag).map(line);
}
// Keyboard (#303): every sidebar link and the board's filter button can be
// reached with Tab, and Enter on a sidebar link opens its page.
const keys = await a11y.newPage();
await keys.goto(`${server}/p/default`);
const targets = await keys.$$eval("nav.side-nav a, form.filters button", (els) => els.map((e) => e.getAttribute("href") || e.textContent.trim()));
const reached = new Set();
for (let i = 0; i < 80 && reached.size < targets.length; i++) {
  await keys.keyboard.press("Tab");
  const t = await keys.evaluate(() => { const e = document.activeElement; return e ? e.getAttribute("href") || e.textContent.trim() : ""; });
  if (targets.includes(t)) reached.add(t);
}
const missed = targets.filter((t) => !reached.has(t));
if (missed.length) problems.push(`keyboard: not reachable with Tab: ${missed.join(", ")}`);
await keys.focus("nav.side-nav a[href$='/events']").catch(() => {});
await Promise.all([keys.waitForNavigation().catch(() => {}), keys.keyboard.press("Enter")]);
if (!keys.url().endsWith("/events")) problems.push(`keyboard: Enter on the Events link stayed on ${keys.url()}`);
console.log(`keyboard: ${reached.size}/${targets.length} controls reached with Tab`);
if (advice.length) console.log("accessibility advice (best practice, not failing):\n" + advice.join("\n"));
if (found.length) {
  problems.push("accessibility (axe, WCAG 2.2 AA):", ...found);
} else {
  console.log("accessibility: no WCAG 2.2 AA axe violation");
}
await browser.close();

if (problems.length) {
  console.error(problems.join("\n"));
  process.exit(1);
}
console.log(`ok: ${pages.length * 2 + global.length} pages, no console errors; screenshots in ${out}`);
