// The README and site screenshots (#246): the released UI on demo data,
// at a fixed viewport and scale, first screen only. Run by the
// "Screenshots" workflow after each release; never taken by hand.
// Usage: node scripts/e2e/readme-shots.mjs <server-url> <admin-token> <out-dir>
//
// Every view waits for its ready marker, the fonts and the icon sprite, and
// fails the run when it is missing, still loading or shows an error (#439):
// a half-loaded page is never published. A new view is one line in VIEWS.
import { chromium, devices } from "playwright";
import { mkdirSync } from "node:fs";

const [server, token, out] = process.argv.slice(2);
if (!server || !token || !out) {
  console.error("usage: readme-shots.mjs <server-url> <admin-token> <out-dir>");
  process.exit(2);
}
mkdirSync(out, { recursive: true });

const DESK = { viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 };
const PHONE = devices["iPhone 13"];

// name, path ("/p/default" prefix for project views), theme, ready marker,
// device. The marker is the content the picture is about, not the frame.
const VIEWS = [
  ["board-light", "/p/default", "light", "main .key-numbers", DESK],
  ["board-dark", "/p/default", "dark", "main .key-numbers", DESK],
  ["board-phone", "/p/default", "light", "main .key-numbers", PHONE],
  ["errors-light", "/p/default/errors", "light", "main table", DESK],
  ["pages-light", "/p/default/pages", "light", "main table", DESK],
  ["events-light", "/p/default/events", "light", "main table", DESK],
  ["features-light", "/p/default/features", "light", "main table", DESK],
  ["settings-light", "/p/default/settings", "light", "main form", DESK],
  ["ai-light", "/settings/ai", "light", "main form", DESK],
  ["login-light", "/login", "light", "main section.auth-card", DESK, { signedOut: true }],
];

const browser = await chromium.launch();

async function context(device, signedIn) {
  const ctx = await browser.newContext({ reducedMotion: "reduce", ...device });
  const tab = await ctx.newPage();
  if (signedIn) await login(tab);
  return tab;
}

// With a password account (#428) the token form is folded under
// "Use the admin token": open it first.
async function login(tab) {
  await tab.goto(`${server}/login`);
  const fold = tab.locator("details.token-login");
  if ((await fold.count()) > 0 && !(await fold.evaluate((d) => d.open))) {
    await fold.locator("summary").click();
  }
  await tab.fill("input[name=token]", token);
  await Promise.all([tab.waitForNavigation(), tab.locator("input[name=token]").press("Enter")]);
  if (new URL(tab.url()).pathname === "/login") throw new Error("login: the admin token was refused");
}

async function shoot(tab, [name, path, theme, marker]) {
  await tab.goto(`${server}/theme/${theme}`);
  const r = await tab.goto(`${server}${path}`);
  if (!r || r.status() !== 200) throw new Error(`${name} (${path}): HTTP ${r ? r.status() : "none"}`);
  // A streamed page (#301) finishes after `load`: wait for its content.
  await tab.waitForLoadState("networkidle");
  if ((await tab.locator(".board-waiting:visible").count()) > 0) throw new Error(`${name} (${path}): still loading`);
  // An error message, not the `alert` kind of a chart mark or timeline row.
  const alerts = (await tab.locator("main p.alert:visible, main [role=alert]:visible").allInnerTexts())
    .map((t) => t.trim())
    .filter(Boolean);
  if (alerts.length > 0) throw new Error(`${name} (${path}): shows an alert: ${alerts.join(" / ")}`);
  if ((await tab.locator(marker).count()) === 0) throw new Error(`${name} (${path}): no ${marker} on the page`);
  await tab.locator(marker).first().waitFor({ state: "visible", timeout: 10_000 });
  await tab.evaluate(() => document.fonts.ready);
  await tab.evaluate(() => document.activeElement?.blur()); // no autofocus ring
  if ((await tab.locator("svg.sprite symbol").count()) === 0) throw new Error(`${name} (${path}): no icon sprite`);
  await tab.screenshot({ path: `${out}/${name}.png` });
}

try {
  const tabs = new Map();
  for (const view of VIEWS) {
    const [, , , , device, opts = {}] = view;
    const key = `${device === PHONE ? "phone" : "desk"}-${opts.signedOut ? "out" : "in"}`;
    if (!tabs.has(key)) tabs.set(key, await context(device, !opts.signedOut));
    await shoot(tabs.get(key), view);
  }
  console.log(`${VIEWS.length} screenshots in ${out}`);
} finally {
  await browser.close();
}
