# Key KPIs

These are the metrics OpenLens computes with no setup. **Every KPI can be split by role**, and also by **environment** and **release** (D-012), account, plan, page and feature. The default view is production, current release compared with the previous one: on the board pages, sessions, tasks, errors, the daily active users chart, features and roles are read since the current release (the newest live one, else the newest rolling out; 90 days at most), against the same length before it, or over the last 14 days against the 14 before when no release is sent (D-055). DAU, WAU, MAU and known users keep their windows; alerts, goals, the digest, Ask and shared boards read the fixed 7-, 14- and 30-day windows. Definitions here are the contract: the UI, the MCP tools and the exports all use them, and a change to a definition is recorded in `docs/decisions.md`.

## Roles: defined, matched, tracked

Roles are the main way to slice every number, so they are declared explicitly, not guessed.

- **Declared:** the feature map (`openlens.features.yaml`) has a `roles:` section that lists the app's canonical roles (for example `admin`, `manager`, `member`, `guest`).
- **Matched:** each role lists the raw values the app actually sends in `identify(id, { role })`. These can be exact values, lists or patterns, for example `["owner", "superadmin"] → admin` or `"team_*" → member`. Raw values are always kept next to the resolved role.
- **Unmatched values** are never dropped silently. They show up as `unmatched:<value>`, and the Focus board and `features lint` both raise a "role gap" card until someone maps them.
- **Tracked over time:** role changes are stored with their date, and each event keeps the role the user had *at the time*. A user who moves from `member` to `admin` counts in the right bucket before and after the change. Reports state which rule they use, either "role at event time" (the default) or "current role".
- **Multiple roles:** a user with several roles has a primary role, which is the first listed match in `roles:` order. Their other roles stay queryable.

## Plans: declared, matched, tracked (SaaS only)

Everything on this list shows only when the app sends `plan` in `identify(id, { plan })` **and** the feature map declares a `plans:` section (D-103). An internal tool that sends no plan sees nothing new; there is no setting.

- **Declared:** `plans:` lists the app's canonical plans **from lowest to highest** (for example `free`, `pro`, `team`). Each plan says whether it is paid: `paid: true` by default, so the free plan must say `paid: false` (nothing is guessed from a name).
- **Matched:** like roles, each plan lists the raw values the app sends (exact values or `*` patterns, case-insensitive; the first plan that matches wins). Raw values are always kept.
- **Unmatched values** count as `unmatched:<value>`, in no upgrade or downgrade. The map health (`features lint --live`, finding `plan_gap`), the Feature map page and the board's `plans.gaps` list them until someone maps them.
- **Tracked over time:** each plan change is stored with its date (`plan_history`, written when an `$identify` carries a plan different from the current one, by capture and by the import), and each event keeps the plan the user had *at the time* ("plan at event time", the default and only rule). A user who moves from `free` to `pro` counts as free before the change and as pro after it.
- **Without `plans:`** the plan history is still kept on raw values, but upgrades, downgrades, paid and free are unknown and not shown.

| KPI | Definition |
| --- | --- |
| **Upgrades** | Moves to a higher plan in the map's order, or from a free plan to a paid one, read from successive `$identify` plans of each user (90 days read). Counted over the last 30 days against the 30 days before, and per release (the release the user was on when the new plan was sent; the 5 newest). A change within one plan (`pro` → `pro_yearly`) is not a move. |
| **Downgrades** | The reverse: a lower plan, or paid to free. |

## Users

| KPI | Definition |
| --- | --- |
| **DAU / WAU / MAU** | Distinct identified users with at least one event on the day, in the last 7 days or in the last 30 days. Days are the reader's days: their own time zone, else the project's (D-017, D-018; `UTC` until set). Robots are never counted: the SDK skips them in browsers, and capture files anything posted with a crawler or monitor user agent under the `bot` environment. Anonymous visitors are counted separately ("not logged in"); a project that never calls `identify` counts every visitor. Users identified with `internal: true` (the team) are left out of every number unless *Include the team* is ticked (`internal=1` in the API). Events sent before login belong to the user once `identify` links the anonymous id, so a visit that starts logged out is not counted twice. |
| **Signup** | When a person's account was created: the `created_at` the app sent with `identify` or `openlens-cli import` (RFC 3339; the earliest one wins; a date in the future or before 1990 is ignored), else their first `identify` ever. A customer created a year ago who is first seen today signed up a year ago, not today. In an internal tool it is the day the colleague joined the tool (#707, D-111). |
| **Stickiness** | DAU / MAU, as a ratio. |
| **Revenue** | The sum of the `revenue` (else `value`) property of the success events declared in the feature map, last 30 days, per `currency`. Amounts on other events are ignored. Currencies are never summed unless the map sets `revenue: { currency: EUR, rates: { USD: 0.92 } }` (fixed rates, no live rates). Per active user: the total over identified users active in the window (#231). |
| **Known users** | Identified users seen in the last 90 days (the team excluded unless asked). The base of the active shares: DAU ÷ known users on the headline tile, MAU ÷ known users next to it (#217). |
| **Churn** | A user is *churned* after `churn_after_days` without an event (14 by default; the one optional setting, in the feature map). **Churn rate** for a period is the number of users active at the start of the period who churned during it, divided by the number of users active at the start. **Resurrected** users (churned, then active again) are reported separately: these are users active in the last 30 days whose previous active day was more than `churn_after_days` earlier. A user who sends the map's optional `churn_event` (an explicit cancel) is churned at once. |
| **Weekly retention** | Users grouped by the week of their first visit ever (Monday first, in the project's time zone), which is their first `identify` (not their first event in the window, so a returning user is not a new cohort). Each cell is the share of the cohort active *k* weeks later. The last column of each row is the current week, still in progress. |
| **Session** | A run of a user's events with gaps of less than 30 minutes. A session also ends at midnight UTC (the SDK does not know the project's zone), and it counts on the local day it starts in. |
| **Average session duration** | Mean and median of the time from the first to the last event of each session. A session with at most one *interaction* (a page view, click, submit, feature use, survey answer or custom event; `$scroll`, `$vital`, `$identify` and `$error` don't count) is a **bounce**; bounces are left out of the mean and counted on their own. |
| **Tasks performed** | The number of *completed tasks* (see Features): success events of the features in the map, or feature uses when there is no map. Given in total, per active user-day (a user active on 3 days counts 3), and per session. |

## Performance

| KPI | Definition |
| --- | --- |
| **Errors** | Uncaught JS errors and unhandled promise rejections. Reported as an error count, **error rate** (sessions with at least one error divided by all sessions), affected users, and the top error groups. Each is split by page, role and feature. |
| **Core Web Vitals** | **LCP**, **INP** and **CLS**, plus **TTFB** and **FCP**, measured in the browser from the W3C Performance APIs. Reported at p75 per page, device class and role, and graded good / needs improvement / poor using the public thresholds. |
| **Failed requests** | `fetch`/XHR calls answering 5xx or failing on the network, as a rate per page and per feature. Only the URL path and status are captured, never bodies. |
| **Page load** | Navigation timing (time to load and time to interactive) at p50 and p75 per page. |

### Web Vitals explained

Generated from `crates/engine/src/vitals.rs`; a test fails when this section and the table differ. The board, Ask and the `vitals` MCP tool all read the same table.

| Vital | What the visitor feels | Good | Poor |
| --- | --- | --- | --- |
| **LCP** (Largest Contentful Paint) | How long until the main content (the largest image or block of text) is on screen: when the visitor feels the page has loaded. | ≤ 2.5 s | > 4.0 s |
| **INP** (Interaction to Next Paint) | How long the page takes to show a response after a click, tap or key press, for about the slowest interaction of the visit: lag. | ≤ 200 ms | > 500 ms |
| **CLS** (Cumulative Layout Shift) | How much the content jumps while the visitor reads, over the worst burst of shifts: the reason people click the wrong thing. | ≤ 0.10 | > 0.25 |
| **FCP** (First Contentful Paint) | How long until anything at all (text, an image, a background) appears: the time the visitor stares at a blank screen. | ≤ 1.8 s | > 3.0 s |
| **TTFB** (Time to First Byte) | How long until the server starts answering the page request: everything else on the page waits for it. | ≤ 800 ms | > 1.8 s |

A vital that is not good opens *Why and what to do* on the board: for LCP, INP and CLS, the worst pages at p75 (pages with at least 8 measurements in the last 7 days), then the causes and actions below, most likely first. For LCP, when TTFB is measured on the same pages and is more than half of LCP, the server cause comes first.

The SDK also says where each vital comes from (raw facts, no text): the element (a short CSS selector), LCP's resource path and four parts, INP's interaction type and three parts, the element that moved most for CLS, and the device class. When at least 8 slow measurements of the worst pages carry the parts, the part that weighs most picks the cause, and the elements most often behind them are listed:

- LCP waiting for the first byte: The server is slow to send the first byte of the page.
- LCP load delay (the element's resource starts downloading late): The main image is found late: added by a script, lazy-loaded, or queued behind other downloads.
- LCP load time (the element's resource downloads): The main image takes long to download.
- LCP render delay (loaded, not yet drawn): Styles and scripts keep the browser from drawing the page.
- INP input delay (the page is busy when the visitor acts): The page is busy running scripts when the visitor acts.
- INP processing (the event handlers run): The code that answers a click or key press does too much at once.
- INP presentation (drawing the next frame): Drawing the next frame takes long.

**LCP: likely causes and what to do**

1. The main image is found late: added by a script, lazy-loaded, or queued behind other downloads. Preload the main image (or put it in the HTML itself, not added by a script), do not lazy-load it, and give it fetchpriority="high".
2. The main image takes long to download. Make the main image lighter: a modern format, sized for the screen, served from a CDN with long caching.
3. Styles and scripts keep the browser from drawing the page. Inline the styles the first screen needs and defer the scripts it does not need. Render the first screen on the server instead of in the browser.
4. The server is slow to send the first byte of the page. Cache the page's HTML (or put a CDN in front of it), remove redirects, and speed up the server work behind the page.

**INP: likely causes and what to do**

1. The page is busy running scripts when the visitor acts. Split long scripts into small tasks that hand control back to the browser, and load third-party scripts later.
2. The code that answers a click or key press does too much at once. Make the visible change first and move the rest of the work after the next paint (or to a worker).
3. Drawing the next frame takes long. Keep the page's DOM small, do not read layout right after changing styles, and skip rendering off-screen content (content-visibility).

**CLS: likely causes and what to do**

1. Images, videos, ads or embeds arrive without space kept for them. Give every image, video and iframe a width and height (or a CSS aspect-ratio), and keep space for ads and embeds.
2. Content is inserted above what the visitor is reading (banners, late results). Keep the space before the content arrives, or insert it below the visible part or only after a visitor action.
3. Web fonts swap in and re-flow the text. Preload the main fonts and pick a fallback font of the same size (or font-display: optional).
4. Animations move elements by changing their position or size. Animate with transform and opacity only.

**FCP: likely causes and what to do**

1. Styles and scripts in the page head block the first paint. Inline the critical styles, defer or async the scripts, and drop the CSS the page does not use.
2. The first byte of the page arrives late. Speed up the server response first (see TTFB).
3. Text stays invisible until the web fonts load. Preload the main fonts and show a fallback font meanwhile (font-display: swap).

**TTFB: likely causes and what to do**

1. The server does slow work (database, rendering) before it answers. Cache the page's HTML or the slow queries behind it, and measure where the server time goes.
2. Visitors are far from the server. Serve the pages from a CDN or a region closer to the visitors.
3. Redirects run before the page. Link straight to the final address (no http to https or trailing-slash hops).

## Lifecycle

Where people are on the way from a first visit to a customer who stays (#708, D-111). Read over the board's window: since the current release, a date range when asked, else the last 30 days. Each stage is shown only when its data exists, with no setting (D-014): an internal tool, where everyone logs in and nobody pays, sees *account created → activated → retained*.

| Stage | Definition |
| --- | --- |
| **Visitor** | Anonymous visitors seen in the window who were never linked to a user, plus the accounts created in the window whose linked anonymous ids were seen in it. Shown when the window has anonymous traffic. |
| **Account created** | People whose **signup** (see Users) falls in the window and who have an event in it under the filters. |
| **Activated** | Of the accounts created, those who reached a success event of the feature map (the value moment). Shown when the map declares success events. |
| **Paid** | Of the accounts created, those with revenue (see Users) on a success event in the window. Shown when a success event carries revenue; a `plan` alone does not make anyone paid yet (#712). |
| **Retained / churned** | Of the accounts created in the window before the churn window started (old enough to judge), those active in the last 7 days, and those inactive for longer than the churn window (`churn_after_days`). Shown for a window that ends now. |
| **Visitor → account rate** | Accounts created in the window with at least one linked anonymous id seen in it ÷ (never-linked anonymous visitors of the window + those accounts). A **lower bound**: a signup on another device, after cleared storage or from an email link leaves no link. The upper bound, every account created ÷ (never-linked visitors + every account created), is given with it. In cookieless mode (most visitors carry the server's daily ids, which are never linked) the rate is *not available*. |

`lifecycle` (MCP and `openlens-cli lifecycle`, `GET /api/projects/{id}/lifecycle`) returns the same numbers as the board for the same filters, optionally per first-touch channel, role, plan or release (`by`).

**Visitors** (the board's block, shown only when the window has anonymous traffic, #709): per entry page (the first page view of each anonymous session) and per channel (that page view's landing channel), the top 10 by visitors, with sessions, bounce rate and **signup rate** = visitors linked to an account created in the window ÷ visitors (a lower bound). A session is anonymous when it was sent before login: it still counts after its id was linked.

## Features

A **feature** is declared in the feature map. A **task** is one use of a feature, from its entry (route, `data-feature` element, or an `openlens.feature()` call) to its `success` event. A task with no success within 30 minutes is *abandoned*.

| KPI | Definition |
| --- | --- |
| **Most / least used** | Ranked by tasks started and by distinct users. Ranking by users is the default so that a few power users don't hide a feature nobody else touches. Features declared in the map but never used are listed as **unused**. |
| **Adoption** | Distinct users who completed at least one task with the feature, divided by active users in the period. |
| **Task duration** | Time from entry to success. Median and p90 per feature, and per declared step when `steps:` exist. **Longest tasks** ranks features by median duration and points out the slowest step. |
| **Task completion rate** | Completed tasks divided by started tasks. For abandoned tasks, the last step reached is recorded. |
| **Frequency** | Completed tasks per adopting user per week. |

## Delivery

How a project ships, per environment, over a period (the last 90 days by default, 1 to 90 on request; D-068). Shown on the *Releases* page, and returned by `GET /api/projects/{id}/delivery` and the MCP `delivery` tool. A **change** is a succeeded or failed deploy from the deploy hook (D-053) when the environment has one in the period; otherwise a release that reached live (95 % of a day's users, D-049) or was rolled back, shipped on its first day with the majority of the users.

| KPI | Definition |
| --- | --- |
| **Deploy frequency** | Changes shipped per week: succeeded deploys, or releases that reached live or were rolled back. |
| **Lead time** | Median time from a release's first commit to live (its succeeded deploy, or its first day at 95 %). The first commit's time comes from the deploy step: `openlens-cli release --commit` (and the `openlens-release` action) reads the author time of the oldest commit of the release's range (`from..commit`) from the app's git history and records it with the release (#633). For a release recorded with its range but without that time (MCP `record_release`, a shallow checkout), the hourly GitHub read of the range (#574, when the project has a GitHub link) gives it instead. Releases recorded without it (no commit, a shallow checkout, the first release of an environment) leave lead time *unknown*, with why. |
| **Change failure rate** | Failed changes ÷ changes. A change fails when its deploy fails, a later deploy reports a rollback, an older release takes the users back (rolled back), or its release's verdict is *regressed*. Verdicts are those of the Releases list, judged in one environment at a time: the environment asked for, else production. |
| **Time to restore** | Median time from a failed change to the rollback that put the previous release back, else to the next shipped change that did not fail. Failures with neither are counted as *not restored yet*. |

## Where they show up
- **Focus board** (home screen): ranked insight cards, then a KPI strip (DAU/WAU/MAU and users who came back, stickiness, average session, tasks per user per day, sessions with errors) and Web Vitals at p75. A filter bar sets role, release and environment.
- **Focus board sections:** feature pulse (adoption, uses per user, active days, completion, task time), *Who uses what* (roles × features), weekly retention cohorts, journeys, funnels, flag cohorts and release comparison.
- **Plans tile** (only with `plans:` in the map): upgrades in the last 30 days against the 30 before, with the downgrades and the number of plan gaps.
- **MCP:** `kpi_report { env, role, release, by: role | account | plan | page | feature }` returns the headline KPIs plus a 30-day breakdown in one compact payload, and `plans` (upgrades, downgrades, per release, gaps; `plan_rule: "plan at event time"`) when the map declares plans. `insights` returns the cards, and `user_timeline` returns one person's story.
- **Export:** `openlens-cli export` writes the raw events as JSONL, day by day, for analysis elsewhere.
