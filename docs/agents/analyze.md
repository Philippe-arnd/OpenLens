# Analysing an app with OpenLens (agent guide)

Start from the summaries and drill down only where something stands out. Every tool is available as an MCP tool (`openlens-cli mcp`) and over the HTTP API.

| PM question | Tool | Drill down with |
| --- | --- | --- |
| What should we focus on? | `insights`: ranked cards with evidence and next steps, plus paths to value | `sql` |
| Headline KPIs by role, environment or release | `kpi_report` (optional `role`, `env`, `release`) | `board` (everything) |
| Split by account, plan, page or feature | `kpi_report` with `by` (`role`, `account`, `plan`, `page`, `feature`) | `sql` |
| Did the last release move the numbers? | `kpi_report`: `releases` holds the latest two | `kpi_report --release <name>` |
| What did this user do (before churning, before an error)? | `user_timeline { distinct_id }`: role history, recordings, latest events | the replay link it returns |
| What breaks, for whom? | `errors`: groups with users, roles, releases, places and the original source line | `user_timeline` of an affected user |
| Are pages slow, and what do we fix first? | `vitals`: each Web Vital explained, p75, rating, thresholds, worst pages and ranked actions | `sql` on `$vital` events |
| Is the wiring complete? | `verify` | `feature_map_lint` |
| Turn a finding into a ticket | `create_ticket { kind, subject, to: markdown \| github }`: prefilled problem, evidence, hypothesis, success metric | `tickets` (learning log: what moved its metric) |
| Where do our goals stand? | `goals`: status, guardrails, what moved each goal (releases and changes), objectives | `set_goal { kpi, target, deadline \| releases, guardrails }` |
| Is the feature map any good? | `map_health`: score, findings with evidence and a YAML fix | `suggest_map_changes` (patches only) |
| Every feature at a glance | `map_overview`: status, uses, users, success (30 days vs the 30 before), areas, adoption ladder | `feature_context { key }` |
| Everything about one feature | `feature_context { key }`: definition, findings, users and success by role, top pages, ladder, area peers | `sql` |
| Anything else | `sql` (read-only DataFusion on `events`) | prune with `date >= 'YYYY-MM-DD'` |
| Deep, offline analysis | `openlens-cli export --from --to --out events.jsonl` (raw events, day by day) | your own tools |

**Map health score:** 100, minus up to 40 for the share of pages and clicks no feature claims, minus 10 per error and 4 per warning, never below 0. A coverage gap of 10% or more of all pages and clicks is an error; every other finding is a warning. Rates are not judged below 20 users. Fix the map first when the score is low: every per-feature number depends on it.

Report findings with the numbers and the cohort sizes behind them, and say when a sample is too small to conclude: cards are hidden below 8 users per group for that reason. Backfilled events carry `"$backfill": true` in `properties`.

In SQL, `events` gives each user the events they sent before logging in: an anonymous id that `identify` later linked (`$anon_distinct_id`) is replaced by the user's id. The stored ids are in `raw_events`, and the links in `person_aliases(anon, person)` (present once at least one anonymous id was linked).
