# Analysing an app with OpenLens (agent guide)

Start from the summaries and drill down only where something stands out. Every tool is available as an MCP tool (`openlens-cli mcp`) and over the HTTP API.

| PM question | Tool | Drill down with |
| --- | --- | --- |
| What should we focus on? | `insights`: ranked cards with evidence and next steps, plus paths to value | `sql` |
| Headline KPIs by role, environment or release | `kpi_report` (optional `role`, `env`, `release`) | `board` (everything) |
| Split by account, plan, page or feature | `kpi_report` with `by` (`role`, `account`, `plan`, `page`, `feature`) | `sql` |
| Did the last release move the numbers? | `kpi_report`: `releases` holds the latest two | `kpi_report --release <name>` |
| Is the new release live, still rolling out, or rolled back? | `releases` (optional `env`): state per environment from the share of daily users, first and last seen, users, time in staging | `kpi_report --release <name>` |
| Did the release I just shipped break anything? | `release_report { release, env? }`: the verdict (healthy, regressed, inconclusive) with its reasons and every number compared with the release before; `releases` carries each verdict | `errors`, then `user_timeline` of an affected user |
| What did this user do (before churning, before an error)? | `user_timeline { distinct_id }`: role history, recordings, latest events | the replay link it returns |
| What breaks, for whom? | `errors`: groups with users, roles, releases, places and the original source line | `user_timeline` of an affected user |
| Are pages slow, and what do we fix first? | `vitals`: each Web Vital explained, p75, rating, thresholds, worst pages and ranked actions | `sql` on `$vital` events |
| Is the wiring complete? | `verify` | `feature_map_lint` |
| Turn a finding into a ticket | `create_ticket { kind, subject, to: markdown \| github }`: prefilled problem, evidence, hypothesis, success metric | `tickets` (learning log: what moved its metric) |
| Where do our goals stand? | `goals`: status, guardrails, what moved each goal (releases and changes), objectives | `set_goal { kpi, target, deadline \| releases, guardrails }` |
| Is the feature map any good? | `map_health`: score, findings with evidence and a YAML fix | `suggest_map_changes` (patches only) |
| Every feature at a glance | `map_overview`: status, uses, users, success (30 days vs the 30 before), areas, adoption ladder | `feature_context { key }` |
| Everything about one feature | `feature_context { key }`: definition, findings, users and success by role, top pages, ladder, area peers, features used with it, use per release with the map version in force | `sql` |
| Which features go together? | `feature_relations { feature? }`: lift, which comes first, leads to value (30 days of sessions, no cap) | `feature_context { key }` |
| Anything else | `sql` (read-only DataFusion on `events`) | prune with `date >= 'YYYY-MM-DD'` |
| Deep, offline analysis | `openlens-cli export --from --to --out events.jsonl` (raw events, day by day) | your own tools |

**Map health score:** 100, minus up to 40 for the share of pages and clicks no feature claims, minus 10 per error and 4 per warning, never below 0. A coverage gap of 10% or more of all pages and clicks is an error; every other finding is a warning. Rates are not judged below 20 users. Fix the map first when the score is low: every per-feature number depends on it.

**Suggestions from sessions (#283)**, last 30 days, each a warning with a YAML fix (nothing changes the map until you push it):

- `too_big`: the feature's pages and events fall into two or more groups that sessions do not mix (used together in fewer than 10% of the smaller group's sessions), each group in at least 25% of its sessions and with 20 users; pages in more than 80% of its sessions (a shared entry page) are left out. The fix splits it, one feature per group.
- `too_small`: two features used together in 90% or more of the sessions that use either, each with 20 users. The fix merges the second into the first (`renamed_from` keeps its history).
- `step_order`: two consecutive `steps` happen in the other order in most of the 20 or more sessions that do both. The fix is the observed order.
- `drift`: a route first seen with the newest release (the one whose first event is the latest, last 90 days), that no feature claims, and that weighs 1% of that release's pages and clicks. It replaces the `coverage` finding for that route.

**Relations** (`feature_relations`, `map_health.relations`, `feature_context.relations`): pairs of features used in the same session by at least 20 sessions, counted over every session of the 30 days (the first 20 features of each), from the session rollup for closed days (D-054). A feature succeeds in a session when one of its success events resolves to it there. `lift` is how many times more often than chance, `first_share` the share of shared sessions where `first` came first, and `leads_to_value` says the success rate of `then` after `first` beats its rate in sessions without `first` (two-proportion test, p < 0.05, 30 sessions per side). They follow the role and release filters.

**Map version in force:** `feature_context.releases` counts each release with the map version saved last before the release's first event (`map_version`), so pushing a new map does not rewrite earlier releases. A release older than every stored version is counted with the oldest (`before_first_map`).

Report findings with the numbers and the cohort sizes behind them, and say when a sample is too small to conclude: cards are hidden below 8 users per group for that reason. Backfilled events carry `"$backfill": true` in `properties`.

In SQL, `events` gives each user the events they sent before logging in: an anonymous id that `identify` later linked (`$anon_distinct_id`) is replaced by the user's id. The stored ids are in `raw_events`, and the links in `person_aliases(anon, person)` (present once at least one anonymous id was linked).
