# Parity with the product-analytics category

The reference is the public feature lists of hosted product-analytics suites, described in our own words. Status: `—` not started, `◐` partial, `●` done. Our own differentiators are marked ★.

| Area | Capability | Milestone | Status |
| --- | --- | --- | --- |
| Capture | Batched ingestion, WAL, Parquet, compaction | M1 | ● |
| Capture | Script tag and npm SDK: autocapture, identify, sessions, errors, Web Vitals, failed requests, scroll depth | M1 | ● (npm publish once the repo is public) |
| Capture | ★ Environment and release on every event | M1 | ● |
| Platform | ★ One-click Coolify deploy with no required env vars | M1 | ● |
| Setup | ★ Feature map with roles, lint and verify, maintained by agents | M2 | ● |
| Setup | ★ Backfill from the app's own data | M2 | ● |
| Agents | ★ MCP server: insights, KPI report, user timeline, errors, read-only SQL, flags, export | M2–M3 | ● |
| Insights | KPIs by role (DAU/MAU, churn, resurrection, sessions, tasks, errors, vitals, feature use) | M3 | ● |
| Insights | ★ Focus board: stick drivers, value moment, churn points, blind spots, value journeys | M3 | ● |
| Insights | ★ Release vs release comparison on every KPI | M3 | ● |
| Insights | Funnels (declared steps), retention cohorts, paths | M4 | ● (ad hoc funnels through SQL) |
| Flags | Rollout by percentage, role or account; flag cohorts | M4 | ● |
| Experiments | Significance testing on flag cohorts | Later | — |
| Replay | Recording, masking, player, links from insights | M5 | ● |
| Errors | Grouping, source maps, linked replays | M5 | ● |
| Heatmaps | Clicks, rage clicks, scroll depth (as lists) | M5 | ◐ (no visual overlay) |
| Platform | Retention, backups, benchmarks | M6 | ● |
| Platform | OIDC, S3 backend, multi-user permissions | Later | — (D-015) |
| Out of scope | Data warehouse, CDP pipelines, survey builder, LLM observability | — | not planned |
