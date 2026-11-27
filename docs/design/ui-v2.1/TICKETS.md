# Tickets to open for UI v2.1

**Opened on 2026-11-27: tracking issue #625** (issues #611–#624, in batches). B1 was already fixed by #555, and #593/#594 shipped in #600.

Each section below is one GitHub issue, ready to paste: the title is the heading, the labels and dependencies come first, and then the body. One issue is one PR (AGENTS.md rule 7).

Order: the bugs (B1–B4) first, then UI21-01 to UI21-08, which need no backend. UI21-09 to UI21-11 wait for #593/#594.

Reference for all of them: `docs/design/ui-v2.1/README.md` and the canvas page "V2.1 proposal".

---

## Bugs

### B1 · Board: "Paths to value" shows the same row twice
Labels: `bug`, `area:ui`

**Seen:** the demo board shows `home → ✓ $click · 186 users · 99.5%` twice.
**Expected:** one row per distinct path.
**Acceptance:** a test with two identical paths in the input renders one row (rule 27).
_Check on `main` first: it may be fixed already._

### B2 · Delivery: change failure rate ignores regressed verdicts
Labels: `bug`, `area:engine`

**Seen (demo):** Releases › Delivery shows *0.0%, 0 of 2 changes*, while both v1 and v2 have the verdict *regressed*.
**Expected (`docs/kpis.md`, Delivery):** "a change fails when … its release's verdict is *regressed*", so 2 of 2.
**Acceptance:** a scenario test where a regressed release counts as a failed change. If the demo is right (for example, because the verdict is computed after the delivery window), the definition in `kpis.md` says so instead.

### B3 · Feature pulse: task time "0s · p90 0s" when success fires on entry
Labels: `bug`, `area:ui`

**Seen:** Home shows a task time of 0s, because its success event (`$pageview`) fires on entry. Map health already reports this ("success always").
**Expected:** "—" with a link to that Map health finding, never a duration of 0.
**Acceptance:** a test where a feature whose success always equals its entry renders no duration.

### B4 · Board: the DAU tile counts today's partial day
Labels: `bug`, `area:ui`

**Seen:** with traffic stopped, the tile reads "3 · 1.0%" for a 10-day window.
**Expected:** on a window, the headline is the users active in the window. DAU can be a secondary line, labelled "today, so far".
**Acceptance:** a test where the window's active users and today's DAU differ, and the headline uses the window's.
_UI21-03 replaces this tile: do B4 in the same PR if it comes first._

---

## Focus board

### UI21-01 · Board: "Production right now" status strip
Labels: `enhancement`, `area:ui` · Depends on: none (the Key actions cell is added by UI21-10)

**Why:** the first question of a PM opening OpenLens is "is anything broken?". Today the answer is spread over a banner, an insight card and the Releases page.
**Scope:**
- A card with cells App (uptime and traffic), Latest release (verdict), Incidents (open count) and Setup (steps and role gaps). Each cell has a dot, a one-word state, one line of detail, and a link.
- It replaces the signals line (alert pill and setup pill).
- It stays visible when everything is fine.
**Acceptance:**
- It renders from the existing engine data, with no new query on the board path.
- A colour is never the only cue (a word in every cell).
- It works at 390 px (cells stack).
- Shared views hide Setup.

### UI21-02 · Board: severity first, a regressed release takes the lead as its verdict
Labels: `enhancement`, `area:ui` · Depends on: UI21-01

**Why:** a regression on the release that every user runs outranks any opportunity, and the verdict already holds the reasons.
**Scope:**
- When the latest release in the filtered environment has the verdict *Regressed*, the lead card shows: a plain-language headline, checks grouped by layer (Browser and Features now, Backend in UI21-10), the next step, Create ticket, Errors on vN, and Full verdict.
- The `release_regression` insight is not repeated in the ranked list.
- Otherwise, the lead is the top insight, as today.
- The ranked list shows at most 4 rows.
**Acceptance:**
- Testkit scenario: a planted regression in release N+1 is the lead.
- With a healthy release, the top insight is the lead.
- No duplicate card.

### UI21-03 · Board: "Outcomes" tiles replace "Key numbers"
Labels: `enhancement`, `area:ui` · Depends on: B4

**Scope:** five tiles, each with its question as an eyebrow:

| Tile | Number |
|---|---|
| Reach | active users in the window, with known users and the role split |
| Value | tasks per session, vs the previous release |
| Retention | churn rate, with the median lifetime |
| Quality | sessions with errors (the server line comes in UI21-10) |
| Speed | LCP p75 with its grade |

- Remove Stickiness and Average session from the board.
- A 6th tile, Revenue, only when revenue exists.
- Each tile states its comparison, or shows no delta.

**Acceptance:** definitions unchanged (`kpis.md`), and the glossary tests are updated; each tile links to its page; the definitions stay reachable from the ⓘ.

### UI21-04 · Board: "In flight" block (goals, tickets being measured, flags)
Labels: `enhancement`, `area:ui` · Depends on: none

**Why:** the board shows the focus step but not whether the last actions worked. This closes the act → compare loop on the home screen.
**Scope:** three rows:
- goals with progress, or the suggested target when there is none;
- tickets from cards, with the release that shipped them and the effect measured at 14 days;
- running flags, with their cohort result.

An empty row has one sentence and one action.
**Acceptance:** each row reads existing engine data (goals, tickets, flags), with no new setting; the empty states are tested.

### UI21-05 · Board: one "Features" table replaces Feature pulse, Who uses what and Never used
Labels: `enhancement`, `area:ui` · Depends on: none

**Scope:**
- Columns: Feature, Adoption (bar), Users, Completion, Uses / user, role mini bars (SVG), Signals.
- Signals are tags taken from the insights (stick driver, churn exit, role blind spot) and from "not used in the window".
- Hide a column whose value is the same on every row.
- No `style=` attribute: widths go in SVG attributes.

**Acceptance:**
- An insight about a feature shows as a tag on its row.
- An unused feature is listed.
- WCAG: the role bars have a text alternative.

### UI21-06 · People › Retention tab; the board keeps one line
Labels: `enhancement`, `area:ui` · Depends on: none

**Scope:**
- A new Retention tab under People holds the weekly cohort table and the survival curve from Churn (an SVG step line instead of the list of days).
- The board shows one summary line (week-N return of the oldest and newest complete cohorts, tagged Falling or Rising) and a link.

**Acceptance:** the cohort numbers are identical to today's; the line picks complete cohorts only.

### UI21-07 · Board: move secondary sections to their pages
Labels: `enhancement`, `area:ui` · Depends on: UI21-03, UI21-05, UI21-06

**Scope:**
- Remove from the board: Web Vitals card, Releases table, Who uses the app, Paths to value, Funnels, Journeys, flag cohorts and Role gaps. Each lives on its page as in README §2.7.
- Paths to value joins Behaviour › Feature flow.
- Funnels and journeys join each Feature page.

**Acceptance:**
- No information is lost: every removed block is reachable in one click from the board.
- The board fits in 2 screens at 1440×900 on the demo.

### UI21-08 · Navigation: Releases at the top level, Goals under Product
Labels: `enhancement`, `area:ui` · Depends on: none

**Scope:**
- `NAV` in `lib.rs`: add Releases (Releases · Timeline · Delivery); Live keeps Live · Uptime.
- Move Goals to Product.
- Renumber the `accesskey`s.
- Old URLs keep working (same routes; only the menu changes).

**Acceptance:** the breadcrumbs and `aria-current` are right on every moved page; the e2e screens check passes.

---

## Backend (after #593 and #594)

### UI21-09 · Quality › Routes tab
Labels: `enhancement`, `area:ui` · Depends on: #594 (`route_q`, `routes.list`)

**Scope:**
- A summary line: requests, share answered 5xx, routes worse than the previous release, key actions degraded.
- A table: route (method tag), requests, 5xx (with its previous value when it moved), p50, p95, a 10-day p95 sparkline (SVG), and a *Key action* tag.
- Sort: key actions first, then the largest change.
- An empty state explains the Node SDK.

**Acceptance:**
- It reads `route_q` for closed days.
- The p95 comes from merged histograms, never an average of p95s (#594).
- WCAG checks pass.

### UI21-10 · Backend on the board and in the release verdict
Labels: `enhancement`, `area:ui` · Depends on: #594, UI21-01, UI21-02, UI21-03

**Scope:**
- A **Key actions** status cell: the worst key action and its success rate vs before, or "None declared" with a link to the feature map.
- A **Backend** group in the lead verdict and on the Release page: regressed routes (5xx, p95), new server error groups, and "*n* other routes unchanged".
- The **Quality** tile gets a second line: "*x*% of server requests answered 5xx".
- On the Release page: a **Routes that got worse** table with the top server error and the sessions that made the request.
- With no backend data, every one of these blocks says so in one line, and the verdict is unchanged (#594 A).

**Acceptance:**
- The #594 scenario (sign-in 5xx 0% → 100%) shows the release as regressed on the board, with the route named in the lead.
- No backend data → no Backend group and no empty numbers.

### UI21-11 · Errors: browser or server origin, and the link to the session
Labels: `enhancement`, `area:ui` · Depends on: #593 (server `$error`, traceparent)

**Scope:**
- An *Origin* tag per error group (Browser or Server), and a filter for it.
- A server group lists the browser sessions linked through `traceparent`, with their replays.
- Server `$error` events never count in user KPIs (#593 D).

**Acceptance:**
- A server error made from a recorded session links to that replay.
- `$server` pseudo-users are excluded from Reach, Retention and release share (test).
