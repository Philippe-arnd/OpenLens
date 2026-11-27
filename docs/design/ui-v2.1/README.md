# OpenLens UI v2.1: a Focus board built around the PM

v2 gave OpenLens its visual language. v2.1 changes **what the board shows and in what order**, using every feature on `main` plus #593 (backend signal) and #594 (backend in the verdict and alerts).

- Canvas, page "V2.1 proposal": https://claude.ai/artifact/TcC9zJP9wCAQjgJvcentce
- Mockups (static HTML, light and dark captures): `mockups/`. Each mockup has a **backend** switch on the canvas (Tweaks), to see the board with and without #593/#594 data.
- Tickets to open: [`TICKETS.md`](TICKETS.md).

Values marked `sample` are made up: #593 and #594 are not shipped, so no demo data exists for them. Every other number comes from the demo dataset (`cargo xtask demo`).

---

## 1. The PM's questions, in the order they ask them

A PM opens OpenLens with a short list of questions. The board answers them top to bottom, and each question gets one block:

| # | Question | Block | Method step |
|---|---|---|---|
| 1 | Is anything broken right now? | **Status strip** | Measure |
| 2 | What should I do next? | **What to focus on** (lead + ranked) | Focus |
| 3 | Is the product getting better or worse? | **Outcomes** (5 tiles) | Measure |
| 4 | Did what we shipped work? | **Lead verdict** when the release regressed, **In flight** otherwise | Act → Compare |
| 5 | Which features matter, and for whom? | **Features** table | Map |
| 6 | Are users coming back? | **Retention** summary line | Focus |

Rule: **the board shows exceptions and directions; pages show inventories.** If a block is the same every day, it does not belong on the board.

---

## 2. Focus board v2.1, block by block

### 2.1 Status strip: "Production right now"

One card with five cells. Each cell has a coloured dot, a one-word state, one line of detail, and a link.

| Cell | Source | States |
|---|---|---|
| App | uptime and traffic rules (`alerts.rs`) | Up · Traffic stopped · Down |
| Latest release | `verdict.rs` | Healthy · Regressed · Inconclusive · Measuring |
| Key actions (#594 B) | key-action routes in the feature map | All fine · *name* degraded · None declared (with a link to the map) |
| Incidents | open incidents | 0 · *n* open |
| Setup | setup checklist and role gaps | *n* of 3 done · *n* unmapped roles |

It replaces the signals line (alert pill and setup pill). When everything is fine, the strip still shows, all green: "fine" is information too.

### 2.2 What to focus on: severity first

- **When the latest release in the environment is Regressed**, the lead card *is* the verdict: a headline in plain words ("Since v2, one session in three hits an error"), the checks grouped by layer (Browser, Backend from #594, Features), the next step and Create ticket. The `release_regression` insight is then not repeated in the ranked list.
- **Otherwise**, the lead is the top-ranked insight, as in v2.
- The ranked list keeps 4 rows at most. "Show all *n*" links to an Insights page if there are more.

### 2.3 Outcomes: five questions, one number each

| Tile | Number | Replaces | Why |
|---|---|---|---|
| Reach | Active users **in the window** (+ known users, members/admins) | Daily active users, Who uses the app | A daily count on a 10-day window shows today's partial day: the demo reads "3". |
| Value | Tasks per session (vs the previous release) | Tasks per user per day | The same unit as the verdict, so the board and the Releases page agree. |
| Retention | Churn rate (+ "half are gone by day *n*", the Kaplan–Meier median) | Stickiness | DAU/MAU at 1.6% says nothing to a PM of a weekly-use B2B app. Churn is in the method, and its page already exists. |
| Quality | Sessions with errors, plus "0.7% of server requests answered 5xx" with #593 | Sessions with errors | Front and back in one tile. |
| Speed | LCP p75 with its grade | The Web Vitals card | One number, graded. The other vitals stay on Pages. |

Every tile says what it compares with ("vs 10 days before", "v1: 2.76", "good ≤ 2.5 s"). A tile with no comparison says so, and shows no delta arrow. **Average session** leaves the board: higher is not always better. It stays in the verdict as a neutral check.

### 2.4 Usage chart + In flight

- **Usage since v2:** the daily active users chart stays, with release and incident markers. It is the only time chart on the board.
- **In flight** closes the act → compare loop on the home screen:
  - **Goals:** progress against target. With none, it suggests one ("sessions with errors back to 2.9%, the best release so far": the Goals page already computes this suggestion).
  - **Tickets being measured:** each ticket from a card, the release that shipped it, and its measured effect 14 days later.
  - **Flags running:** cohort result and significance.
  - Empty rows teach the loop in one sentence each, with one action.

### 2.5 Features: one table instead of four blocks

Feature pulse, Who uses what, Never used and the feature-level insights merge into one table:

| Column | Note |
|---|---|
| Feature | Link to its page |
| Adoption | Bar + % of active users |
| Users, Completion, Uses / user | Numbers |
| Member · admin | Mini bars per role. A role at 0% shows a red dash. |
| Signals | Tags from the insights and analyses: *Stick driver 5.2×*, *Churn exit 72%*, *Admins 0%*, *Not used since v2* |

Hide a column whose value is the same on every row (completion is 100% everywhere in the demo), and show it on the Feature map. Task time "0s" means the success event fires on entry: show "—" with a link to the Map health finding instead.

### 2.6 Retention: one line

"Week 4: 63% of the 5 Oct cohort, 40% of the 19 Oct cohort", tagged *Falling* or *Rising*. The full cohort table moves to a new **People › Retention** tab.

### 2.7 What leaves the board, and where it goes

| Today on the board | Goes to |
|---|---|
| Web Vitals card | Speed tile; details on Pages / Quality › Web Vitals |
| Releases table | Lead verdict or status cell; the Releases page |
| Who uses the app | Reach tile |
| Who uses what | Features table (role mini bars); full matrix on the Feature map |
| Never used | Features table (Signals) |
| Do they come back? (table) | People › Retention; one line on the board |
| Paths to value | Behaviour › Feature flow ("Paths to success"). It also shows duplicate rows today. |
| Funnels, Journeys | Feature page (per feature) |
| Flag cohorts | In flight; Flags page |
| Role gaps | Status strip (Setup cell) |
| Revenue | A 6th Outcomes tile when revenue exists, else nothing |

---

## 3. Do the visualisations make sense? An audit of the board on `main`

| Visualisation | Verdict | Why / change |
|---|---|---|
| Lead insight with before/after bars | Keep | Two bars, one comparison, readable at a glance. |
| Ranked insights | Keep | Text is right for findings. |
| KPI tiles with sparkline | Change | Only DAU has a series. Sparklines on all five or on none; the deltas carry the rest. |
| DAU "3 · 1.0%" | Fix | A daily count on a window; see Reach. |
| Stickiness 1.6% | Remove | No action follows from it. |
| Active users bar chart | Keep | With markers. It is the board's only time chart. |
| Feature pulse adoption bars | Change | All at 100% in the demo: no information. Sort by signal, not by adoption. |
| Who uses what heat matrix | Move | Too big for its information; role mini bars carry the signal (Admins 0% on Reports). |
| Cohort heat table | Move | It is the right chart, on the wrong page: the board needs the trend, not the table. |
| Paths to value | Remove | "home → $click" twice: a bug, and weak as a visual. |
| Web Vitals table | Replace | By a graded tile. |
| Releases comparison table | Replace | By the verdict with checks: a table of numbers makes the PM do the comparison. |
| Sankey (Feature flow page) | Keep | Right for flows. Add "Exits" nodes for churn. |
| Churn: "How long they stay" | Change | Today a list of days and percentages; draw it as a survival curve (an SVG step line). |
| World map (Audience) | Keep, lower | Interesting, rarely actionable: put it under the KPI tiles. |

---

## 4. Other pages

| Page | v2.1 change |
|---|---|
| **Releases** (promoted to the top level, see 5) | One page per release: header with state and share; **Verdict** grouped by layer (Browser, Backend, Features, Guardrails); **Routes that got worse** (#594) with their top server error and the sessions that saw it (#593 C); Delivery (DORA). The list page keeps its table, with a verdict tag per release. |
| **Quality › Routes** (new, #594 D) | Summary line (requests, 5xx share, routes worse than the previous release, key actions degraded), then a table: route, requests, 5xx (with the previous value when it moved), p50, p95, a 10-day p95 sparkline, and a *Key action* tag. Sorted with key actions first, then by size of change. The empty state explains the Node SDK. |
| **Quality › Errors** | An origin tag per group (*Browser* or *Server*). A server group links to the browser sessions that made the request (traceparent, #593 C). |
| **People › Retention** (new tab) | The weekly cohort table, plus the survival curve from Churn. |
| **Product › Goals** (moved from People) | Goals sit next to Tickets and Flags: the "act" tools. |
| **Feature page** | Gets the feature's funnel and journeys from the board, and its server routes when they are declared. |
| **Alerts settings** | No new toggle (#594 decision 3). Show which routes are key actions and that they ignore quiet hours. |

---

## 5. Navigation v2.1

```
Focus board · Ask · Releases · Live · Quality · Behaviour · People · Product · Settings
```

| Section | Tabs |
|---|---|
| Releases (new top level: the "compare" step) | Releases · Timeline · Delivery |
| Live | Live · Uptime |
| Quality | Errors · Routes · Web Vitals · Replays |
| Behaviour | Events · Pages · Heatmap · Forms · Feature flow |
| People | Audience · Retention · Churn · Acquisition · Revenue |
| Product | Feature map · Goals · Flags · Surveys · Tickets · Digest |

The `accesskey`s follow the new order (1–9).

---

## 6. Constraints (unchanged from v2)

`AGENTS.md` wins. In particular:

- No client-side JS, and no `style=` attribute (CSP).
- No new setting (#594 decision 3 reuses `failing`).
- Our own wording.
- `prefers-reduced-motion` respected.
- A block with no data shows an explained empty state, never zeros dressed up as a result (rule 11).
