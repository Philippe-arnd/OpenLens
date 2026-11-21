# The OpenLens method

OpenLens is a **method for product managers to monitor and improve a web app**. The software is how the method is carried out. Agents do all the setup, so the PM's first contact is a dashboard that already holds data and insights.

## The loop

```
   ┌──────────► 1. MAP ──────────► 2. MEASURE ──────────┐
   │      features, success,     one dependency,        │
   │      roles (agent-kept)     backfill, releases     ▼
5. COMPARE                                         3. FOCUS
   release vs release,                         ranked insights:
   env vs env                                  stick, churn, journeys
   ▲                                                    │
   └──────────── 4. ACT ◄───────────────────────────────┘
          change shipped as a release
          (optionally behind a flag)
```

1. **Map.** The app declares what matters in `openlens.features.yaml`, which its agents keep current:
   - its **features**, where each one starts, and its **success event** (the moment the user got value)
   - its **roles**, and how the raw role values map to them

   No map, no insight. Every metric is "per feature" and "per role".
2. **Measure.** One dependency (a script tag or the npm package) captures usage, sessions, errors, Web Vitals, the **environment** and the **release**. History is **backfilled** from the app's own database, so the first day already has months of signal.
3. **Focus.** The home screen ranks a handful of insights:
   - what makes users stick (features and the *value moment*)
   - where they churn
   - which journeys lead to value
   - which roles use what

   Every card shows the numbers and the cohort size behind it, plus a suggested next step.
4. **Act.** The PM picks a card. An agent turns it into a change (fix the drop-off step, guide new users to the value moment, surface an unused sticky feature) and ships it as a new release, optionally behind a flag for one role.
5. **Compare.** OpenLens compares the new release with the previous one, and an environment with another, on the same KPIs. The board says whether the change moved the number. Then the loop starts again.

## Day zero: no human setup

| Step | Who | What happens |
| --- | --- | --- |
| Deploy | Agent (or one Coolify click) | OpenLens starts with **no required environment variables**. Keys and the admin token generate themselves. |
| Connect | Agent | `openlens-cli init` in the app repo creates the project and writes the capture key into the app's existing config. It adds the SDK, the `identify()` call with the role, and draft `features` and `roles` read from the code. |
| Backfill | Agent | `openlens-cli import` loads users (role, signup, last seen) and past successes (rows in domain tables) from the app's database or an export. |
| Verify | Agent | `openlens-cli verify` sends test events and checks that the round trip, the roles and the feature entry points all work. `openlens-cli features lint` checks the map against the code. |
| Hand over | Agent → PM | `openlens-cli login-link` creates a one-time link, and the agent sends it to the PM. |
| First login | PM | The Focus board, already populated. |

From then on, the app's agents keep the feature map in sync (a rule in the app's own `AGENTS.md`/`CLAUDE.md`). Every deploy tags events with its release.

## What the PM sees, and nothing more

- **A KPI strip**, by role, per environment, and for the current release against the previous one:
  - *Users:* DAU/MAU, churn, average session duration, tasks per user
  - *Performance:* error rate, Web Vitals, failed requests
  - *Features:* most and least used, longest tasks
- **Insight cards** (3–7), ranked by impact.
- **Drill-down** from every number to the people, sessions and events behind it.

Everything deeper is for agents: MCP tools, read-only SQL and full exports. The PM asks their agent; the agent asks OpenLens.

Definitions are in [kpis.md](kpis.md). The feature map format is in [schema/features.example.yaml](schema/features.example.yaml).
