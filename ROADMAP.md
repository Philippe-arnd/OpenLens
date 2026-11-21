# Roadmap

Status: **pre-alpha** (2026-11-21). Each milestone has exit criteria. A milestone is done when its criteria pass in CI or in a written manual check linked from the milestone. GitHub milestones mirror this file, and this file wins if they disagree.

**North star.** A product manager adds **one dependency** to a web app, or more often asks the agent that maintains it to. Within an hour they can answer: *which features and journeys make users stick, where they churn, and which roles use what, how often, and whether they like it.* The UI stays very simple. The full data is available to agents for deep analysis. We target the 80% of apps that are a web front end with logged-in users who have a role.

The key KPIs are users (DAU/MAU, churn, average session duration, tasks performed), performance (errors, Core Web Vitals, failed requests) and features (most and least used, longest tasks). **All are split by user role.** Definitions are in [docs/kpis.md](docs/kpis.md).

Priorities: **M0–M3 deliver the north star.** M4–M8 add the deeper tools. M9 makes it production-grade.

---

## M0: Foundations
- Cargo workspace with layers, lints and a `cargo xtask ci` gate (fmt, clippy, tests, layers, cargo-deny)
- AGENTS.md, clean-room log, decisions log, parity and gaps docs
- Dockerfile (static binary, distroless) and a `docker-compose.yml` with the healthcheck inside the compose file, ready for Coolify
- Branch protection on `main`

**Exit:** CI is green on `main`, and `docker compose up` answers `/healthz`.

## M1: Capture pipeline and the one-line SDK
- `POST /capture` (batched, gzip, size limits, API key per project) and `/c.js`, which serves the SDK itself so a single `<script>` tag works with no build step
- Write-ahead log, then Parquet segments partitioned `project/date/hour`, with background compaction and local-disk storage
- `@openlens/web` (sdk/js), under 10 kB gzipped, with:
  - automatic capture of page views, clicks, form submits and route changes in single-page apps
  - `identify(id, { role, account, plan })`
  - batching with `sendBeacon` on unload
  - **sessions** (30-minute inactivity rule)
  - **errors**: uncaught errors and unhandled rejections, counted and grouped by message and top frame (full grouping comes in M7)
  - **Core Web Vitals** (LCP, INP, CLS, plus TTFB and FCP) measured from the W3C Performance APIs, our own implementation
  - **failed requests** (fetch/XHR 5xx and network errors, path and status only)
  - do-not-track and consent switches
- Event explorer in the UI: live tail and filters by event, person and property

**Exit:** a page with one `<script>` tag shows its events, sessions, errors and vitals in the explorer within 5 s, and 10k events/s are ingested on 1 vCPU (see the benchmarks).

## M2: Agent wiring kit and the feature map ("plug it in for me")
Most apps are vibecoded or maintained by agents, so wiring is written **for agents first**: explicit, declarative and verifiable.

- **Feature map:** `openlens.features.yaml` lives in the app's repo, and agents maintain it. Each feature declares:
  - `key`, `name`, a one-line `purpose`
  - **entry points**: routes, `data-feature` selectors or explicit `openlens.feature()` calls
  - a **success event**: what "the user got value" means, for example `invoice_sent`
  - optional **steps**: the expected journey inside the feature
  - an `owner`

  The map also has a **`roles:` section**: the app's canonical roles, and the raw `identify()` values that match each one (exact values, lists or patterns). Unmatched values are surfaced as role gaps and never dropped. Role changes are stored with their date, so each event keeps the role the user had at the time (see docs/kpis.md).

  The schema is published as JSON Schema (`docs/schema/features.schema.json`). The server ingests the map on deploy (`openlens-cli features push`) and versions it, so metrics stay comparable across renames.
- **Stack detection** (`crates/wire`):
  - Front ends: Next.js, React+Vite, Vue/Nuxt, SvelteKit, Angular
  - Server-rendered: Rails, Django, Laravel, Express, Phoenix
  - Plain HTML
- **Install plan:** typed edits covering the dependency or script tag, init, `identify()` with role/account/plan at the login point, a first draft of the feature map built from routes (including `roles:`, filled from the role values found in the code), and `data-feature` attributes on the main actions.
- `openlens-cli wire [--dry-run]` and the MCP tool `wire_project` return or apply the plan. `openlens-cli features lint` checks the map against the code: missing selectors, dead routes, features that never fire.
- **Rules for the app's agents:** a short snippet added to the app's own `AGENTS.md`/`CLAUDE.md` saying "when you add or change a user-facing feature, update `openlens.features.yaml` and run `openlens-cli features lint`". This keeps the map in sync as the app evolves.
- `openlens-cli verify`: sends a test event per feature entry point and confirms the round trip, including the role.
- Agent guide: `docs/agents/wire.md`.

**Exit:** on 8 sample apps (one per target stack), an agent following `docs/agents/wire.md` reaches a green `verify` and a lint-clean feature map with no hand edits on at least 6 of them.

## M3: Stick, churn, journeys (the PM view) and agent-grade data access
The UI stays very simple. **The first login lands on the Focus board**, a dashboard of ranked insights telling the PM where the value is and what to work on, with no setup. The depth goes to agents.

- **KPI strip** at the top of the Focus board, with a role selector:
  - **Users:** DAU/WAU/MAU, stickiness, churn and resurrection, average session duration, tasks per user per day
  - **Performance:** error rate, CWV grade at p75, failed-request rate
  - **Features:** most and least used (by users), unused features, longest tasks (median and p90 duration, slowest step), task completion rate

  Every number is split by role. The MCP tool `kpi_report` returns the same data.
- **Focus board (home screen).** It holds 3–7 insight cards ranked by impact, each written in plain language, backed by numbers and cohort sizes, and with a drill-down. For example:
  - "Users who save 3 entries in week 1 stay 2.4× more"
  - "Admins never reach *AI insights*"
  - "40% of churned users dropped at step *fill_section*"
  - "*Export PDF* is popular but rated 2/5"

  Each card links to its evidence and to a suggested next step (improve onboarding toward the value moment, fix a drop-off step, promote an under-used sticky feature). Before enough data exists, the board says so honestly, shows progress toward the first insights, and lists wiring gaps found by `features lint`. The cards are generated by the same engine commands the MCP tools use. An optional LLM summary pass (bring your own key) only rewrites wording and never changes the numbers.

- **What makes users stick.** For each feature in the map, compare the retention of users who used it (or reached its success event) early on with users who did not, and rank features by lift. Also detect the **value moment**: the combination of "did X N times in the first D days" that best predicts staying.
- **Where users churn.** The last features and steps seen before users go inactive, drop-off inside each feature's declared steps, and abandoned success events. Churn is defined per app as inactivity longer than N days (configurable).
- **Which journeys lead to value.** The most frequent paths into each success event, and the paths of retained users compared with churned users.
- **Feature pulse** for each feature: frequency, adoption, stickiness, retention and conversion, plus a **roles matrix** (a heat grid of roles × features).
- **Appreciation:** an optional built-in micro-survey (thumbs, or a 1–5 score with an optional comment) triggered after the Nth use of a feature, plus passive signals (repeat use, rage clicks, abandoned flows).
- **For agents (as important as the UI):**
  - MCP tools `feature_report`, `stickiness_drivers`, `churn_points`, `journeys`, `cohort_compare`, `user_timeline` and `sql` (read-only DataFusion). Each returns a compact summary first, with ids for drilling down.
  - `openlens-cli export`: Parquet or JSONL per project and date range, with a documented, stable schema (`docs/schema/`) and the feature map joined in.
  - An analysis guide, `docs/agents/analyze.md`, gives the standard questions and the tool calls that answer them.
- A weekly digest by email or webhook, which is itself an agent-readable JSON report.

**Exit:** on first login to a project with data, the Focus board shows ranked insight cards with evidence links. On a dogfood app (Journal), using only the M1 SDK, an `identify` call carrying the role and a feature map, the three-question screen and `stickiness_drivers`/`churn_points` return results that hold up against a synthetic dataset with planted patterns in testkit (the planted driver ranks first and the planted churn point is found).

## M4: Funnels, retention, paths
- Funnels with conversion and time-to-convert, breakdowns, and a conversion window
- Retention cohorts, user paths, saved insights, dashboards
- DataFusion aggregate functions `funnel()`, `retention()`, `paths()`

**Exit:** results on generated datasets match a reference implementation in testkit, and a funnel over 10M events returns in under 2 s.

## M5: Feature flags and experiments
- Boolean and multivariate flags, percentage rollouts, targeting by role, account or property
- Deterministic hashing; `GET /flags` and a local-evaluation payload for server SDKs
- Experiments: flag variant × goal metric, with a significance readout

**Exit:** flag evaluation p99 is under 5 ms server-side, and rollout percentages hold within ±1% over 100k identifiers.

## M6: Session replay
- Recording based on the rrweb format, loaded lazily (not in the base bundle), with privacy masking on by default
- Chunked zstd storage, and a player page that embeds the rrweb player (MIT, vendored, not handwritten; see D-011)
- Links: event to the moment in the replay, and error to its replay

**Exit:** a 10-minute session replays faithfully and costs under 1 MB stored.

## M7: Error analysis (deep)
- Capture and error rate already ship in M1. This milestone adds: a server SDK hook,
- Fingerprint grouping, uploaded source maps, trends, first and last seen, affected roles and features

**Exit:** the same error thrown from 3 call paths groups as one issue, and minified stacks resolve to the original source.

## M8: Heatmaps
- Click, rage-click and scroll-depth overlays per URL and viewport bucket, from data already captured

**Exit:** the overlay lines up within 5 px on 3 viewport sizes of a fixture page.

## M9: Hardening and dogfooding
- Auth (local accounts plus OIDC), multiple projects, retention and TTL, S3 backend, backups
- `openlens-cli mcp` with the full command set; benchmarks published
- Server SDKs: Node, Python, Rust (thin HTTP clients)
- Move Journal and Kanban from OpenObserve RUM to OpenLens

**Exit:** two of Phil's prod apps run on OpenLens for 30 days without data loss.
