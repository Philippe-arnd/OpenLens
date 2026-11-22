// M2 exit check, browser part: click through a reference app like a user.
// Usage: node scripts/e2e/reference.mjs <notes-ssr|tasks-spa> <app-url>
// Afterwards CI runs `openlens-cli features push` and `verify`, which must
// say ok with every feature fired (docs/agents/wire.md, step 6).
import { chromium } from "playwright";

const [app, url] = process.argv.slice(2);
const browser = await chromium.launch();
const problems = [];

async function as(user, steps) {
  const ctx = await browser.newContext();
  const tab = await ctx.newPage();
  tab.on("pageerror", (e) => problems.push(e.message));
  await steps(tab);
  // Closing a context does not reliably fire pagehide, so wait for the
  // SDK's periodic flush (every 5 s) instead.
  await tab.waitForTimeout(6000);
  await ctx.close();
}

if (app === "notes-ssr") {
  for (const user of ["u1:owner", "u2:member"]) {
    await as(user, async (tab) => {
      await tab.goto(`${url}/login?as=${user}`);
      await tab.click("a[data-feature=write-note]");
      await tab.fill("textarea[name=text]", "buy milk");
      await Promise.all([tab.waitForNavigation(), tab.click("button[data-feature=write-note]")]);
      await tab.goto(`${url}/search?q=milk`);
    });
  }
} else if (app === "tasks-spa") {
  for (const user of ["t1:manager", "t2:member"]) {
    await as(user, async (tab) => {
      await tab.goto(`${url}/tasks?as=${user}`);
      await tab.click("a[data-feature=create-task]");
      await tab.fill("input[name=title]", "ship it");
      await tab.click("button[data-feature=create-task]");
      await tab.click("a[data-feature=reports]");
    });
  }
} else {
  throw new Error(`unknown app ${app}`);
}
await browser.close();
if (problems.length) {
  console.error(problems.join("\n"));
  process.exit(1);
}
console.log(`${app}: clicked through`);
