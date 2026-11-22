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
| Is the wiring complete? | `verify` | `feature_map_lint` |
| Anything else | `sql` (read-only DataFusion on `events`) | prune with `date >= 'YYYY-MM-DD'` |
| Deep, offline analysis | `openlens-cli export --from --to --out events.jsonl` (raw events, day by day) | your own tools |

Report findings with the numbers and the cohort sizes behind them, and say when a sample is too small to conclude: cards are hidden below 8 users per group for that reason. Backfilled events carry `"$backfill": true` in `properties`.

In SQL, `events` gives each user the events they sent before logging in: an anonymous id that `identify` later linked (`$anon_distinct_id`) is replaced by the user's id. The stored ids are in `raw_events`, and the links in `person_aliases(anon, person)` (present once at least one anonymous id was linked).
