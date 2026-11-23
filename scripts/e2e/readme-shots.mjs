// The README and site screenshots (#246): the released UI on demo data,
// at a fixed viewport and scale, first screen only. Run by the
// "Screenshots" workflow after each release; never taken by hand.
// Usage: node scripts/e2e/readme-shots.mjs <server-url> <admin-token> <out-dir>
import { chromium, devices } from "playwright";
import { mkdirSync } from "node:fs";

const [server, token, out] = process.argv.slice(2);
mkdirSync(out, { recursive: true });

const browser = await chromium.launch();
async function session(opts) {
  const ctx = await browser.newContext({ reducedMotion: "reduce", ...opts });
  const tab = await ctx.newPage();
  await tab.goto(`${server}/login`);
  await tab.fill("input[name=token]", token);
  await Promise.all([tab.waitForNavigation(), tab.click("button[type=submit]")]);
  return tab;
}

// name, path, theme
const shots = [
  ["board-light", "", "light"],
  ["board-dark", "", "dark"],
  ["errors-light", "/errors", "light"],
  ["pages-light", "/pages", "light"],
  ["events-light", "/events", "light"],
  ["features-light", "/features", "light"],
];
const desk = await session({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 });
for (const [name, path, theme] of shots) {
  await desk.goto(`${server}/theme/${theme}`);
  const r = await desk.goto(`${server}/p/default${path}`);
  if (!r || r.status() !== 200) throw new Error(`${path}: HTTP ${r ? r.status() : "none"}`);
  await desk.screenshot({ path: `${out}/${name}.png` });
}
const phone = await session({ ...devices["iPhone 13"] });
await phone.goto(`${server}/theme/light`);
await phone.goto(`${server}/p/default`);
await phone.screenshot({ path: `${out}/board-phone.png` });
await browser.close();
console.log(`${shots.length + 1} screenshots in ${out}`);
