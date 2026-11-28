# Parity with the product-analytics category

The reference is the public feature lists of hosted product-analytics suites, described in our own words. Status: `—` not started, `◐` partial, `●` done. Our own differentiators are marked ★. A milestone `—` with a release means the work shipped outside a milestone.

Last updated: 2026-11-28, against v0.7.0 (latest stable) and the closed issues on `main`.

| Area | Capability | Milestone | Status |
| --- | --- | --- | --- |
| Capture | Batched ingestion, WAL, Parquet, compaction | M1 | ● |
| Capture | Script tag and npm SDK: autocapture, identify, sessions, errors, Web Vitals, failed requests, scroll depth | M1 | ● (`@openlens-obs/web` publishes to npm on release tags from v0.8.0 on; until then the script tag or a `file:` dependency, #156) |
| Capture | Server-side events: Node, Python, Go, Ruby and PHP clients, plain HTTP | M7 | ● |
| Capture | Backend signal: server errors, route stats, outgoing dependencies, runtime health, job check-ins; OTLP/HTTP metrics | — (v0.7.0) | ● (#593, #594, #605, #608–#610; route stats and runtime health from the Node SDK, OTLP route and dependency metrics only, no traces or logs) |
| Capture | Native mobile SDKs (iOS, Android, React Native, Flutter) | — | — not planned: every page works on a phone instead (#162, #170) |
| Capture | ★ Environment and release on every event | M1 | ● |
| Platform | ★ Self-hosted with no required env vars: Docker, binary + systemd, Kubernetes, Coolify, Swarm, Nomad, NixOS, panel recipes, with hands-off updates | M1, M8, M10 | ● (#238, #198) |
| Setup | ★ Feature map with roles, lint and verify, maintained by agents | M2 | ● |
| Setup | ★ Backfill from the app's own data | M2 | ● |
| Agents | ★ MCP server: insights, KPI report, user timeline, errors, read-only SQL, flags, export | M2–M3 | ● (every engine command an agent may run, #525) |
| Agents | ★ Ask OpenLens: a chat on every project that answers from the board and SQL and only proposes changes | M11 | ● (#244; off until an admin sets a model) |
| Insights | KPIs by role (DAU/MAU, churn, resurrection, sessions, tasks, errors, vitals, feature use) | M3 | ● |
| Insights | ★ Focus board: stick drivers, value moment, churn points, blind spots, value journeys | M3 | ● |
| Insights | ★ Release vs release comparison on every KPI | M3 | ● |
| Insights | Funnels (declared steps), retention cohorts, paths | M4 | ● (ad hoc funnels through SQL) |
| Flags | Rollout by percentage, role or account; flag cohorts | M4 | ● |
| Flags | Variants with weights, targeting by any property | M11 | ● (#252, #279) |
| Experiments | Significance testing on flag cohorts | M7 | ● |
| Surveys | One-question in-app surveys after a success event, results by role | M7 | ● (no survey builder, by design) |
| Replay | Recording, masking, player, links from insights | M5 | ● |
| Errors | Grouping, source maps, linked replays | M5 | ● |
| Heatmaps | Clicks, rage clicks, scroll depth; visual click density over a recorded visit of the page | M5, M7 | ● |
| Insights | ★ Weekly digest (page, MCP, optional email) and optional LLM rewording of cards | M7 | ● |
| Platform | Retention, backups, benchmarks | M6 | ● |
| Platform | Users and roles (admin / viewer, per-project access), OIDC single sign-on | M7 | ● |
| Platform | S3 | M7, M10 | ● (a copy of closed days, rollups and metadata; an optional cold tier for event days older than 120 days, off until an admin turns it on, #326, #417) |
| Platform | Kubernetes: Helm chart and manifests, tested on a real cluster in CI | M8 | ● (shipped in v0.2.0, #145–#149) |
| Insights | ★ Feature flow: paths between features of the map, to the value moment | M11 | ● (#224) |
| Insights | Audience: world map, devices, browsers, OS, languages | M11 | ● (#225) |
| Insights | Acquisition: referrers, UTM, channels, judged by value-moment rate | M11 | ● (#226) |
| Insights | Automatic regression alerts and a timeline of changes | M11 | ● (#227, #267) |
| Insights | Goals on KPIs with guardrails and objectives | M11 | ● (#235, #272) |
| Insights | Churn: how many, after how long, where and why; at-risk and win-back | M11 | ● (#247, #269) |
| Insights | ★ Releases detected with no setup, a verdict per release, notes read from GitHub and the verdict written back | M10 | ◐ (#495, #522, #574, #575 shipped; #237 still open) |
| Insights | Live view | M11 | ● (#228) |
| Insights | Revenue on success events; form analytics | M11 | ● (#231, #232) |
| Platform | Read-only share links and embeds | M11 | ● (#229) |
| Platform | Erasure and access requests, cookieless mode, audit log | M11 | ● (#230, #273) |
| Platform | Uptime from real traffic and an optional probe, alerts, public status page | M11 | ● (#256, #266, #561) |
| Setup | Import from other analytics tools' public export formats | M11 | ● (#233) |
| Out of scope | Data warehouse, CDP pipelines, survey builder, LLM observability, multi-node clustering | — | not planned |
