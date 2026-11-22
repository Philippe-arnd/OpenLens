# Roadmap

OpenLens is a method plus the tool that carries it out ([docs/method.md](docs/method.md)): **map → measure → focus → act → compare**, with every setup step done by agents. The milestones follow that loop. Each one has an exit check, and GitHub milestones mirror this file (this file wins if they disagree).

**North star.** A PM's first login shows a populated **Focus board**: KPIs by role and release, what makes users stick, where they churn, which journeys lead to value. Nobody configured anything by hand. We target the 80% of apps that are a web front end with logged-in users who have a role.

## Status (2026-11-22)

| Milestone | Status | Exit check |
| --- | --- | --- |
| M0 Foundations | ✅ | ✅ |
| M1 Measure | ✅ | ✅ smoke test in CI; events show within ~10 s (the flush interval) |
| M2 Map | ✅ | 🟡 2 of 3 apps (`examples/`, green `verify` nightly); Journal waits for the first deployment |
| M3 Focus | ✅ | ✅ planted patterns found (testkit); 🟡 "Journal's board makes sense to Phil" waits for the deployment |
| M4 Act | ✅ | ✅ a flagged cohort shows on the board |
| M5 See it | ✅ | ✅ churn card → replay of a session that churned there |
| M6 Production + dogfooding | 🚧 | ⏳ Journal and Kanban on OpenLens since 2026-11-22: 30 days without data loss (until 2026-12-22) |
| M7 The former Later list (D-016) | ✅ | every item shipped, optional ones off until configured |
| M8 Kubernetes (v0.2) | 🚧 | `helm install` with no value and `kubectl apply -k`, installed, upgraded and smoke-tested on kind in CI |

---

## M0: Foundations ✅
A layered workspace, `cargo xtask ci`, AGENTS.md, Docker and compose with the healthcheck, and CI on every PR (fast gate). Browser, image and smoke tests run on `main` and nightly.

## M1: Measure, the one-dependency pipeline ✅
- **`POST /capture`:** batched and gzip-compressed, with a key per project, and safe against hostile input. Events go to the WAL, then to **daily** Parquet partitions (`project=/date=`), with compaction.
- **Metadata:** projects, persons, role history (D-009).
- **`environment` and `release`** on every event (D-012).
- **`@openlens/web` SDK** (4.6 kB gz), served at `/c.js`. It covers autocapture (page views, SPA routes, clicks, submits, rage clicks, scroll depth), `identify(id, { role, account, plan })`, sessions, errors, Web Vitals, failed requests, environment and release detection, consent and DNT.
- **Query engine:** DataFusion over the partitions, with read-only SQL per project.
- **Dashboard:** login with the admin token or a one-time link.
- **Deployment:** the Coolify one-click template and the GHCR image, with no required environment variables.
- 📦 `@openlens/web` on npm: the package and `.github/workflows/npm.yml` are ready, and it publishes on `v*` tags once the repo is public and npm trusted publishing is configured. Until then, use the script tag or a `file:` dependency.

**Exit:** `docker compose up`, then a page with one script tag. Its events, sessions, errors and vitals show on the dashboard within ~10 s, and `scripts/smoke.sh` passes in CI.

## M2: Map, the agent toolkit ✅
- **`openlens.features.yaml`:** features (entry points, success event, optional steps), roles with match rules, the churn window, and a JSON Schema.
- **`openlens-cli`:** `init`, `features lint|push`, `import` (backfill), `verify`, `sourcemaps`, `flags`, `export`, `sql`, `login-link`, `guide`.
- **MCP server** (`openlens-cli mcp`) with every command, plus `insights`, `kpi_report`, `user_timeline`, `errors` and `board`.
- **Agent guides:** [wire.md](docs/agents/wire.md) (served at `/agents.md`) and [analyze.md](docs/agents/analyze.md).
- **Reference apps** in [`examples/`](examples/): `notes-ssr` (script tag) and `tasks-spa` (npm).

**Exit:** an agent following `docs/agents/wire.md` connects 3 apps (an npm SPA, a server-rendered app with the script tag, and Journal) to a green `verify` and a lint-clean map, with no hand edits. The first two are checked nightly; Journal comes with the first deployment.

## M3: Focus, the board ✅
- **KPIs** per [docs/kpis.md](docs/kpis.md), split by role, environment and release. `kpi_report` also splits them by account, plan, page or feature, and states its role rule ("role at event time").
- **Insight cards:** stick drivers, value moment, churn points, friction, role blind spots, release regressions, slow pages, funnel drops, flag wins and regressions. Each card shows its evidence, its cohort size and a next step.
- **Board sections:** feature pulse (adoption, frequency, active days, completion, task time p50/p90), *Who uses what* (roles × features), role gaps, journeys into each success event, and release comparison on every KPI.
- **Agents:** `insights`, `kpi_report`, `user_timeline`, read-only SQL, JSONL export.

**Exit:** on a synthetic dataset with planted patterns (testkit), the planted driver ranks first, the planted churn point is found, and a planted regression in release N+1 is flagged ✅. Journal's board makes sense to Phil ⏳.

## M4: Act, flags and deeper analysis ✅
- **Feature flags:** rollout by percentage, role or account, using deterministic hashing. The SDK has `isEnabled`, each event carries `$flags`, and every flag becomes a cohort (*with vs without*).
- **Funnels** from the declared steps, **weekly retention cohorts**, and paths (journeys). Ad hoc funnels are left to agents (`sql`), because there is no builder by design (D-014).

**Exit:** a flagged change for one role shows up as its own cohort on the board ✅.

## M5: See it, replay, errors, heatmaps ✅
- **Session replay:** rrweb recorder loaded lazily, inputs masked, zstd chunks, a player page, and links from events, errors and churn cards.
- **Errors:** grouped by fingerprint, split by role, release and place, with **source maps** per release.
- **Pages:** scroll depth, most-clicked elements, rage clicks.

**Exit:** from a churn-point card, one click opens a replay of a session that churned there ✅.

## M6: Production-grade and dogfooding 🚧
- ✅ Fixed retention (events 400 days, replays 30 days), `openlens backup` while running, benchmarks ([docs/benchmarks.md](docs/benchmarks.md)): about 80k events/s on one vCPU.
- ✅ A nightly check of every dashboard page in a real browser, under the real CSP, with screenshots.
- ✅ OpenLens deployed (2026-11-22). Journal and Kanban send their events to it, with OpenObserve RUM kept in parallel for the comparison (#42).
- ✅ An audit of the numbers against [docs/kpis.md](docs/kpis.md): anonymous visits linked to the user, identified users only, bounces, tasks, and first seen (#133–#136).
- ⏳ 30 days of dogfooding, until 2026-12-22.

**Exit:** two of Phil's prod apps run on OpenLens for 30 days without data loss.

## M7: The former Later list (D-016) ✅
- ✅ Experiment significance on flag cohorts (#122)
- ✅ Server SDKs: Node and Python, no dependencies ([sdk/SERVER.md](sdk/SERVER.md))
- ✅ Visual heatmaps: click density per page (SVG) and over a recorded visit of the real page
- ✅ Weekly digest: dashboard page, MCP `digest`, Monday email when SMTP is set
- ✅ Users and roles: admin / viewer, optional per-project access, invites by login link (`/users`, CLI, MCP)
- ✅ Single sign-on with OpenID Connect (optional: issuer, client id and secret; invited emails only)
- ✅ In-app micro-surveys: one question after a success event, sampled, once per user; results by role (`/p/{id}/surveys`, CLI, MCP `survey_set`)
- ✅ LLM rewording of cards (optional `OPENLENS_LLM_API_KEY`; numbers guarded, originals kept)
- ✅ S3 copy (optional): closed days and a daily metadata snapshot, signed with our own SigV4 (checked against the AWS example)

## M8: Kubernetes, packaged for agents (v0.2) 🚧
Same philosophy as the Coolify template: one command, no required value, and a guide an agent can follow from an empty cluster to a wired app.
- ✅ Helm chart (`deploy/helm/openlens`): exactly one replica (single writer), PVC, a generated admin token kept across upgrades, an optional Ingress with TLS, probes, a hardened pod, optional features from Secrets (#145).
- ✅ Plain manifests and a Kustomize base rendered from the chart, plus an Ingress component and an example overlay (#146).
- ✅ [docs/deploy/kubernetes.md](docs/deploy/kubernetes.md): install, token, reach, wire, options, upgrade, backup, sizing (#147).
- ✅ CI on kind: lint, install, `helm test`, events in, upgrade with the events and the token kept, Kustomize base too (#148). The chart is published to `oci://ghcr.io/philippe-arnd/charts` on release tags.
- ⏳ Scheduled backups in the cluster (#149).

**Exit:** on a fresh kind cluster, an agent following the guide gets a running instance, a wired example app and a green `verify`, with no hand edits.

