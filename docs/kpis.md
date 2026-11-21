# Key KPIs

These are the metrics OpenLens computes with no setup. **Every KPI can be split by role**, and also by **environment** and **release** (D-012), account, plan, page and feature. The default view is production, current release compared with the previous one. Definitions here are the contract: the UI, the MCP tools and the exports all use them, and a change to a definition is recorded in `docs/decisions.md`.

## Roles: defined, matched, tracked

Roles are the main way to slice every number, so they are declared explicitly, not guessed.

- **Declared:** the feature map (`openlens.features.yaml`) has a `roles:` section that lists the app's canonical roles (for example `admin`, `manager`, `member`, `guest`).
- **Matched:** each role lists the raw values the app actually sends in `identify(id, { role })`. These can be exact values, lists or patterns, for example `["owner", "superadmin"] → admin` or `"team_*" → member`. Raw values are always kept next to the resolved role.
- **Unmatched values** are never dropped silently. They show up as `unmatched:<value>`, and the Focus board and `features lint` both raise a "role gap" card until someone maps them.
- **Tracked over time:** role changes are stored with their date, and each event keeps the role the user had *at the time*. A user who moves from `member` to `admin` counts in the right bucket before and after the change. Reports state which rule they use, either "role at event time" (the default) or "current role".
- **Multiple roles:** a user with several roles has a primary role, which is the first listed match in `roles:` order. Their other roles stay queryable.

## Users

| KPI | Definition |
| --- | --- |
| **DAU / WAU / MAU** | Distinct identified users with at least one event on the day, in the last 7 days or in the last 30 days (UTC days by default; the project time zone is configurable). Anonymous visitors are counted separately. |
| **Stickiness** | DAU / MAU, as a ratio. |
| **Churn** | A user is *churned* after `churn_after_days` (14 by default, set in the feature map) without an event. **Churn rate** for a period is the number of users active at the start of the period who churned during it, divided by the number of users active at the start. **Resurrected** users (churned, then active again) are reported separately. |
| **Session** | A run of a user's events with gaps of less than 30 minutes (configurable). A session also ends at midnight in the project time zone. |
| **Average session duration** | Mean and median of the time from the first to the last event of each session. Sessions with a single event count as 0 s and are flagged separately as "bounces". |
| **Tasks performed** | The number of *completed tasks* (see Features), in total, per active user per day, and per session. |

## Performance

| KPI | Definition |
| --- | --- |
| **Errors** | Uncaught JS errors and unhandled promise rejections. Reported as an error count, **error rate** (sessions with at least one error divided by all sessions), affected users, and the top error groups. Each is split by page, role and feature. |
| **Core Web Vitals** | **LCP**, **INP** and **CLS**, plus **TTFB** and **FCP**, measured in the browser from the W3C Performance APIs. Reported at p75 per page, device class and role, and graded good / needs improvement / poor using the public thresholds. |
| **Failed requests** | `fetch`/XHR calls answering 5xx or failing on the network, as a rate per page and per feature. Only the URL path and status are captured, never bodies. |
| **Page load** | Navigation timing (time to load and time to interactive) at p50 and p75 per page. |

## Features

A **feature** is declared in the feature map. A **task** is one use of a feature, from its entry (route, `data-feature` element, or an `openlens.feature()` call) to its `success` event. A task with no success within the timeout (30 minutes by default) is *abandoned*.

| KPI | Definition |
| --- | --- |
| **Most / least used** | Ranked by tasks started and by distinct users. Ranking by users is the default so that a few power users don't hide a feature nobody else touches. Features declared in the map but never used are listed as **unused**. |
| **Adoption** | Distinct users who completed at least one task with the feature, divided by active users in the period. |
| **Task duration** | Time from entry to success. Median and p90 per feature, and per declared step when `steps:` exist. **Longest tasks** ranks features by median duration and points out the slowest step. |
| **Task completion rate** | Completed tasks divided by started tasks. For abandoned tasks, the last step reached is recorded. |
| **Frequency** | Completed tasks per adopting user per week. |

## Where they show up
- **Focus board** (home screen): a KPI strip across the top (DAU/MAU, churn, average session, tasks per user per day, error rate, CWV grade), with a role selector. Ranked insight cards sit underneath.
- **MCP:** `kpi_report { period, by: role | account | plan | page | feature }` returns all of the above in one compact payload.
- **Export:** the same metrics, pre-aggregated per day and role, sit next to the raw events.
