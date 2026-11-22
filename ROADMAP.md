# Roadmap

Status: **working prototype** (2026-11-22). Progress: M0 ✓, M1 ✓, M2 ✓ (the reference-apps exit check is still open), M3 mostly done (KPIs, insight cards, release comparison, paths to value; MCP analysis tools and export remain), M4 flags, cohorts and funnels ✓, M5 ✓ (replay with masking, linked from events, errors and churn-point cards; error groups; page scroll depth and click hotspots; source maps remain). OpenLens is a method plus the tool that carries it out ([docs/method.md](docs/method.md)): **map → measure → focus → act → compare**, with every setup step done by agents. The milestones follow that loop. Each one has an exit check, and GitHub milestones mirror this file (this file wins if they disagree).

**North star.** A PM's first login shows a populated **Focus board**: KPIs by role and release, what makes users stick, where they churn, which journeys lead to value. Nobody configured anything by hand. We target the 80% of apps that are a web front end with logged-in users who have a role.

---

## M0: Foundations ✓
Layered workspace, `cargo xtask ci`, AGENTS.md, Docker + compose with the healthcheck, CI on every PR.

## M1: Measure, the one-dependency pipeline ✓
- `POST /capture`: batched, gzip, keys per project, hostile-input safe. Events go to the WAL, then to Parquet segments partitioned by project/date/hour, with compaction.
- Metadata: projects, persons, role history (D-009).
- Every event carries **`environment`** and **`release`** (D-012).
- `@openlens/web` SDK, served at `/c.js` (script tag) and published as an npm package. Under 10 kB gzipped. It covers:
  - autocapture of page views, clicks, form submits and SPA routes
  - `identify(id, { role, account, plan })`
  - sessions
  - errors
  - Core Web Vitals
  - failed requests
  - environment and release, detected automatically
  - consent switch
- Query engine: DataFusion over the partitions, with read-only SQL per project.
- Dashboard v0, server-rendered: login with the admin token or a one-time link, project overview, recent events.
- **Coolify one-click template** and a published image (GHCR), with no required env vars.

**Exit:** `docker compose up`, then a page with one script tag. Its events, sessions, errors and vitals show on the dashboard within 5 s, and `scripts/smoke.sh` passes in CI.

## M2: Map, the agent toolkit ✓ (exit check pending)
- `openlens.features.yaml`: features (entry points, success event, optional steps), roles with match rules, churn window. JSON Schema in `docs/schema/`. Parsing and linting live in `crates/featuremap`.
- `openlens-cli`, which talks to a server over HTTP with the admin token:
  - `init`: create the project and print the capture key and snippet
  - `features push` / `features lint`
  - `import`: backfill persons and past success events from JSONL/CSV that the agent extracts from the app DB
  - `verify`
  - `login-link`
- **MCP server** (`openlens-cli mcp`) exposing the same commands, plus `sql`. From here on, MCP is the main interface for agents.
- Agent guides: `docs/agents/wire.md` (connect an app) and the rules snippet for the app's own AGENTS.md.

**Exit:** an agent following `docs/agents/wire.md` connects 3 apps (an npm SPA, a server-rendered app with the script tag, and Journal) to a green `verify` and a lint-clean map, with no hand edits.

## M3: Focus, the board (mostly done)
- KPI engine per [docs/kpis.md](docs/kpis.md), split by role, environment and release. The MCP tool `kpi_report` returns it.
- Insights:
  - stick drivers and the value moment
  - churn points
  - journeys into each success event (retained vs churned)
  - the roles × features matrix
  - role gaps (unmatched role values)
- Focus board home screen: a KPI strip plus 3–7 ranked cards, each with its evidence and a next step. The empty state is honest about missing data and wiring gaps.
- Release comparison: the current release against the previous one, on every KPI and card.
- MCP tools: `stickiness_drivers`, `churn_points`, `journeys`, `cohort_compare`, `user_timeline`. Export to Parquet/JSONL.

**Exit:** on a synthetic dataset with planted patterns (testkit), the planted driver ranks first, the planted churn point is found, and a planted regression in release N+1 is flagged. Journal's board makes sense to Phil.

## M4: Act, flags and deeper analysis
- Feature flags with rollout by percentage, role or account, using deterministic hashing and `GET /flags`. The SDK exposes `isEnabled`. Each flag variant becomes a comparison dimension, like a release.
- Funnels (from the declared steps or ad hoc), retention cohorts, paths.

**Exit:** a flagged change for one role shows up as its own cohort on the board.

## M5: See it, replay, errors, heatmaps
- Session replay (rrweb format, lazy chunk, masking on by default, vendored player), linked from events, errors and insight cards.
- Error grouping by fingerprint, source maps, and the roles and features affected.
- Click and scroll heatmaps, built from data already captured.

**Exit:** from a churn-point card, one click opens a replay of a session that churned there.

## M6: Production-grade and dogfooding
- Fixed retention (events 400 days, replays 30 days), `openlens backup`, benchmarks (docs/benchmarks.md). OIDC and the S3 backend moved to Later (D-015).
- Move Journal and Kanban from OpenObserve RUM to OpenLens.

**Exit:** two of Phil's prod apps run on OpenLens for 30 days without data loss.

## Later (not planned until a real need shows)
Weekly digest, LLM rewording of cards, in-app micro-surveys, experiment significance, server SDKs (thin HTTP clients), multi-user permissions, OIDC login, S3 storage backend (D-015).
