# Analysing an app with OpenLens (agent guide)

Start from the summaries and drill down only where something stands out. Every tool is available as an MCP tool (`openlens-cli mcp`) and over the HTTP API.

| PM question | Tool | Drill down with |
| --- | --- | --- |
| What should we focus on? | `insights`: ranked cards with evidence and next steps, plus paths to value | `sql` |
| Headline KPIs by role, environment or release | `kpi_report` (optional `role`, `env`, `release`) | `board` (everything) |
| Did the last release move the numbers? | `kpi_report`: `releases` holds the latest two | `kpi_report --release <name>` |
| Is the wiring complete? | `verify` | `feature_map_lint` |
| Anything else | `sql` (read-only DataFusion on `events`) | prune with `date >= 'YYYY-MM-DD'` |
| Deep, offline analysis | `openlens-cli export --from --to --out events.jsonl` (raw events, day by day) | your own tools |

Report findings with the numbers and the cohort sizes behind them, and say when a sample is too small to conclude: cards are hidden below 8 users per group for that reason. Backfilled events carry `"$backfill": true` in `properties`.
