# Roadmap

OpenLens is a method plus the tool that carries it out ([docs/method.md](docs/method.md)): **map → measure → focus → act → compare**, with every setup step done by agents. The milestones follow that loop. Each one has an exit check, and GitHub milestones mirror this file (this file wins if they disagree).

**North star.** A PM's first login shows a populated **Focus board**: KPIs by role and release, what makes users stick, where they churn, which journeys lead to value. Nobody configured anything by hand. We target the 80% of apps that are a web front end with logged-in users who have a role.

## Status

Last updated: 2026-11-28. Latest stable release: v0.7.0 (2026-11-27); `main` is on its way to v0.8.0. Each release's notes are in [docs/releases/](docs/releases/), and everything else is in [CHANGELOG.md](CHANGELOG.md).

| Milestone | Status | Exit check |
| --- | --- | --- |
| M0 Foundations | ✅ | ✅ |
| M1 Measure | ✅ | ✅ smoke test in CI; events show within ~10 s (the flush interval) |
| M2 Map | ✅ | 🟡 2 of 3 apps (`examples/`, green `verify` nightly); Journal is wired outside this repo (OpenLens deployed 2026-11-22); its green `verify` and lint-clean map still need to be confirmed and recorded on #18 |
| M3 Focus | ✅ | ✅ planted patterns found (testkit); 🟡 "Journal's board makes sense to Phil" waits for the deployment |
| M4 Act | ✅ | ✅ a flagged cohort shows on the board |
| M5 See it | ✅ | ✅ churn card → replay of a session that churned there |
| M6 Production + dogfooding | 🚧 | ⏳ Journal and Kanban on OpenLens since 2026-11-22: 30 days without data loss (until 2026-12-22) |
| M7 The former Later list (D-016) | ✅ | every item shipped, optional ones off until configured |
| M8 Kubernetes (v0.2) | ✅ | ✅ `helm install` with no value and `kubectl apply -k`, installed, upgraded and smoke-tested on kind in CI (#148); shipped in v0.2.0 |
| M9 Maintenance on autopilot | 🚧 | ✅ dependencies, security checks, the release train and upgrades of running installs run without a person; ⏳ repository settings only the owner can change (#243) |
| M10 Polish (v0.3) | 🚧 | ✅ every item shipped in v0.3.0–v0.6.0 except the release cycle on autopilot (#237): its layers shipped in v0.6.0 and v0.7.0, the issue stays open |
| M11 Explore, category parity (v0.4) | ✅ | ✅ every item shipped in v0.3.0–v0.6.0 ([docs/target-app-parity.md](docs/target-app-parity.md)) |

---

## M0: Foundations ✅
A layered workspace, `cargo xtask ci`, AGENTS.md, Docker and compose with the healthcheck, and CI on every PR (fast gate). Browser, image and smoke tests run on `main` and nightly.

## M1: Measure, the one-dependency pipeline ✅
- **`POST /capture`:** batched and gzip-compressed, with a key per project, and safe against hostile input. Events go to the WAL, then to **daily** Parquet partitions (`project=/date=`), with compaction.
- **Metadata:** projects, persons, role history (D-009).
- **`environment` and `release`** on every event (D-012).
- **`@openlens-obs/web` SDK** (4.6 kB gz), served at `/c.js`. It covers autocapture (page views, SPA routes, clicks, submits, rage clicks, scroll depth), `identify(id, { role, account, plan })`, sessions, errors, Web Vitals, failed requests, environment and release detection, consent and DNT.
- **Query engine:** DataFusion over the partitions, with read-only SQL per project.
- **Dashboard:** login with the admin token or a one-time link.
- **Deployment:** the Coolify one-click template and the GHCR image, with no required environment variables.
- 📦 `@openlens-obs/web` on npm: it publishes on `v*` tags with `@openlens-obs/node`, from v0.8.0 on (`.github/workflows/npm.yml`). Until then, use the script tag or a `file:` dependency.

**Exit:** `docker compose up`, then a page with one script tag. Its events, sessions, errors and vitals show on the dashboard within ~10 s, and `scripts/smoke.sh` passes in CI.

## M2: Map, the agent toolkit ✅
- **`openlens.features.yaml`:** features (entry points, success event, optional steps), roles with match rules, the churn window, and a JSON Schema.
- **`openlens-cli`:** `init`, `features lint|push`, `import` (backfill), `verify`, `sourcemaps`, `flags`, `export`, `sql`, `login-link`, `guide`.
- **MCP server** (`openlens-cli mcp`) with every command, plus `insights`, `kpi_report`, `user_timeline`, `errors` and `board`.
- **Agent guides:** [wire.md](docs/agents/wire.md) (served at `/agents.md`) and [analyze.md](docs/agents/analyze.md).
- **Reference apps** in [`examples/`](examples/): `notes-ssr` (script tag) and `tasks-spa` (npm).

**Exit:** an agent following `docs/agents/wire.md` connects 3 apps (an npm SPA, a server-rendered app with the script tag, and Journal) to a green `verify` and a lint-clean map, with no hand edits. The first two are checked nightly; Journal is a real app wired outside this repo, so its result is recorded by hand on #18 rather than replayed in CI.

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

## M8: Kubernetes, packaged for agents (v0.2) ✅
Same philosophy as the Coolify template: one command, no required value, and a guide an agent can follow from an empty cluster to a wired app.
- ✅ Helm chart (`deploy/helm/openlens`): exactly one replica (single writer), PVC, a generated admin token kept across upgrades, an optional Ingress with TLS, probes, a hardened pod, optional features from Secrets (#145).
- ✅ Plain manifests and a Kustomize base rendered from the chart, plus an Ingress component and an example overlay (#146).
- ✅ [docs/deploy/kubernetes.md](docs/deploy/kubernetes.md): install, token, reach, wire, options, upgrade, backup, sizing (#147).
- ✅ CI on kind: lint, install, `helm test`, events in, upgrade with the events and the token kept, Kustomize base too (#148). The chart is published to `oci://ghcr.io/philippe-arnd/charts` on release tags.
- ✅ Scheduled backups in the cluster: `openlens backup --into --keep` and a CronJob next to the server (#149).

**Exit:** on a fresh kind cluster, an agent following the guide gets a running instance, a wired example app and a green `verify`, with no hand edits.

## M9: Maintenance on autopilot 🚧
Dependencies, CI, releases and upgrades of running installs happen without a person: bots propose, CI proves, automerge ships.
- ✅ Renovate: grouped updates, merged once CI is green, an issue when it is not (#195).
- ✅ Security: RustSec checks, and an issue on a new advisory that closes itself once the check is green again (#196).
- ✅ A release train: CI cuts a stable release every Tuesday when the nightlies are green (#197), with screenshots regenerated, a notes draft and post-release checks (#246, #258).
- ✅ Running installs update safely: `:latest`, minor channels and `:edge`; a backup on version change and a data-format guard in the binary (#199); Coolify, Docker, systemd and an opt-in Kubernetes auto-update (#198, #238).
- ✅ A scheduled agent picks up red builds and failed bumps (#200); toolchains, runners and base images stay current (#201).
- ⏳ Repository settings only the owner can change (#243).

## M10: Polish (v0.3) 🚧
Feedback from dogfooding v0.2.
- ✅ A replay player that fits the screen, and comments pinned to a timestamp (#212, #213).
- ✅ Times in the viewer's time zone (#215), project icons from the app's favicon (#214).
- ✅ Settings for users, projects, storage and backups, flags and surveys, and the feature map, all from the UI (#216, #218–#221).
- ✅ Readable board and events, a person profile and a data explorer (#217, #222).
- ✅ Scale by looking at the present (#223): a bench at 60 M events, daily rollups, the board read since the current release, past periods in the background, query guard rails and an S3 cold tier, automatic replay sampling (#321–#327).
- ✅ Self-hosting beyond Coolify: Docker and Compose, binary and systemd, panels, Nomad, Swarm, NixOS, with hands-off updates (#238).
- 🚧 The release cycle on autopilot (#237). Shipped so far: releases detected with no setup (#495), a Releases page and deploy hooks (#500, #504), a verdict per release (#522), `openlens-cli release` (#521), delivery metrics (#523), release notes read from GitHub and the verdict written back (#574, #575).

## M11: Explore, category parity (v0.4) ✅
A clean-room pass over the public feature lists of product and web analytics suites (2026-11-22). Every feature works with no configuration, from the feature map. The row-by-row status is in [docs/target-app-parity.md](docs/target-app-parity.md).
- ✅ Feature flow, audience, acquisition, live view, forms and revenue (#224–#226, #228, #231, #232).
- ✅ Alerts and a timeline of changes (#227, #267), uptime from real traffic with alerts (#256, #266).
- ✅ Share links and embeds (#229); erasure, access export, cookieless mode and an audit log (#230, #273); import from other tools' export formats (#233).
- ✅ Goals with guardrails (#235, #272), a full churn section (#247, #269), flag variants and property targeting (#252, #279).
- ✅ Feature map quality (#240), Ask OpenLens (#244), findings turned into tickets with their effect measured (#245).
- ✅ Navigation and settings: sections with tabs, breadcrumbs, setup progress, a page per feature, every board card opens its detail, KPI definitions on hover, AI settings, environments by host, Web Vitals explained, idle replays paused (#402–#406, #408–#411).

## Next
Not scheduled yet: the open issues of the [Backlog](https://github.com/Philippe-arnd/OpenLens/milestone/12) milestone, grouped in batches A to E (#720–#724), and the gaps in [docs/gaps.md](docs/gaps.md).
