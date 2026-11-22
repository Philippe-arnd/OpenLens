# Changelog

All notable changes to OpenLens are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and OpenLens uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0.0, a minor version (0.x.0) may change behaviour; each such change is listed under **Changed**.

## [Unreleased]

### Added

- **Upgrade check in CI:** before an image is published, `scripts/upgrade-check.sh` starts the last release on a fresh volume, writes events, a flag and a feature map, then starts the new image on that volume and requires the same events, people, sessions, projects, keys, flags and feature map, and a board that loads (#158).
- **Delete a project:** `DELETE /api/projects/{id}?confirm={id}` or `openlens-cli delete-project <id> --confirm <id>` removes its events, replays, source maps, people, flags, surveys and feature map. The id must be given twice.
- **Rate limits on `/capture`** for traffic coming through a proxy: 50 requests/s per client address (burst 500) and 20,000 events/s per project key (burst 200,000), with `429` and `Retry-After`, which the SDK honours. Direct connections (server SDKs in the cluster, benchmarks) are not limited (#160).
- **Sign-in by email:** with SMTP and the public URL set, the login page offers "Email me a login link" to invited users. Links work once, within 15 minutes. The answer is the same whether the address exists, the link is built from the configured URL (never the request's Host), and requests are rate-limited (#155).
- **Robots at capture:** events posted by crawlers, link previews and uptime monitors (by user agent) are stored in the `bot` environment, out of every production number. `verify` reports `robots_last_day` (#154).
- **Internal users:** `identify(id, { internal: true })` marks the team's own accounts. The board leaves them out unless *Include the team* is ticked (`internal=1` in the API) (#153).
- **Kubernetes:**
  - a Helm chart (`deploy/helm/openlens`): one replica, PVC, a generated admin token kept across upgrades, an optional Ingress with TLS, a hardened pod, and optional features from Secrets;
  - a Kustomize base rendered from the chart;
  - the agent guide [docs/deploy/kubernetes.md](docs/deploy/kubernetes.md);
  - a CI job installing and upgrading both on kind;
  - the chart published to `oci://ghcr.io/philippe-arnd/charts` on release tags.
- **Backups:** `openlens backup --into <dir> --keep N` names each copy after the UTC time and keeps the newest N. The chart's `backup.enabled` runs it nightly as a CronJob next to the server, onto the data volume or `backup.existingClaim`.
- **CLI:** plain HTTP is accepted for cluster-internal servers (`http://<service>:8686`, `*.svc`, `*.cluster.local`, private addresses), so an agent inside the cluster can use it.

### Fixed

- Form submits inside web components (open shadow roots) are captured, with their `data-feature` (#151).
- A resent batch is stored once: the server drops event uuids it stored recently (the last 100k), and compaction keeps one row per uuid per day (#150).
- The replay recorder and player bundles now carry rrweb's MIT notice, which minification had stripped (`sdk/js/licenses/rrweb-LICENSE.txt`).

### Changed

- **No more automatic `default` project.** Agents create one project per app (`openlens-cli init --name <app>`). A `default` project is created only when `OPENLENS_PROJECT_KEY` is set (CI, demos, templates that want a key at once). With no project, the home page points to `/agents.md`. The admin token is logged once, when it is generated.
- **README** redesigned: wordmark, section links, architecture diagram, how it works, dated performance numbers, known limits and releases.
- **Releases** are built by CI (`publish.yml`): every `v*` tag gets Linux binaries, the SDK bundles, the Helm chart and `SHA256SUMS`, with notes from `docs/releases/` or the CHANGELOG and a *Full changelog* link. Pre-release snapshots of `main` can be made by hand; they are not published to npm.
- Docs: the clean-room source log lists every public source used so far; `target-app-parity.md` and `gaps.md` match v0.1.0.

## [0.1.0] - 2026-11-22

The first release. OpenLens is a self-hosted product analytics tool in one Rust binary. Agents wire it into an app, and the product manager opens a board that already says where the value is.

### Added

#### Ingestion and storage

- `POST /capture`: batched and gzip-aware ingestion with a key per project, size and depth limits, clock-skew correction and safe handling of hostile input.
- A write-ahead log, then daily Parquet partitions (zstd), with compaction and crash recovery.
- SQLite metadata: projects, keys, persons, role history, flags, surveys and users.
- Every event carries its environment and release (D-012).
- Fixed retention: events are kept for 400 days, replays for 30 days.
- `openlens backup <dir>` makes a consistent copy while the server runs.
- An optional S3 copy of closed days and a daily metadata snapshot, signed with our own SigV4.

#### Browser SDK `@openlens/web` (4.6 kB gzipped)

- One script tag (`/c.js`) or the npm package.
- **Autocapture:**
  - page views, including History API and `#/` routes, back/forward cache and prerendering;
  - clicks named by `data-feature`, also inside web components;
  - form submits and rage clicks;
  - scroll depth, errors, Web Vitals and failed requests.
- **Identity:** `identify(id, { role, account, plan })`, which links the anonymous visit before login; `reset()`; sessions shared across tabs.
- **Delivery:** failed sends are retried with the same event ids. Crawlers and monitoring robots are not captured.
- **Privacy:**
  - no cookies;
  - input values are never captured;
  - Do Not Track and consent modes are respected;
  - `data-openlens-mask`, `-block` and `-ignore` give finer control.
- **Session replay** (rrweb, MIT) loads lazily from `/r.js`, with every input masked.
- **Feature flags:** `isEnabled`, plus `$flags` on every event.
- **Micro-surveys:** `/s.js`, one question after a success event.

#### Server SDKs

- Node and Python thin HTTP clients with no dependencies ([sdk/SERVER.md](sdk/SERVER.md)).

#### The method and the Focus board

- **Feature map** (`openlens.features.yaml`): features, entry points, success events, steps and canonical roles. The CLI lints it against the code.
- **KPIs by role, account, plan, page, feature, environment and release** ([docs/kpis.md](docs/kpis.md)):
  - DAU/WAU/MAU, stickiness and resurrected users;
  - sessions, bounce rate and tasks;
  - error rate and Core Web Vitals.
- **Insight cards**, each with its evidence, cohort size and next step:
  - stick drivers and the value moment;
  - churn points, friction and role blind spots;
  - release regressions, slow pages and funnel drops;
  - flag wins and regressions, with significance tests.
  - A card shows only once a group has 8 or more users.
- **Board sections:**
  - feature pulse (adoption, frequency, active days, completion, task time p50/p90);
  - roles × features;
  - weekly retention cohorts;
  - journeys into each success event, and funnels from the declared steps;
  - release vs release, environment vs environment.
- **Errors:** grouped by fingerprint and split by role, release and page, with source maps per release and links to replays.
- **Pages:** scroll depth, the most-clicked elements, rage clicks, and a visual click heatmap over a recorded visit of the real page.
- **Weekly digest:** a page, the MCP `digest` tool, and an optional Monday email over SMTP.
- **LLM rewording of cards:** optional. Numbers are guarded and the originals are kept.

#### Agents

- **`openlens-cli`:** `init`, `features lint|push`, `import` (backfill), `verify`, `sourcemaps`, `flags`, `surveys`, `export`, `sql`, `login-link`, `rules` and `guide`.
- **MCP server** (`openlens-cli mcp`) with every command, plus `insights`, `kpi_report`, `user_timeline`, `errors`, `board` and `digest`.
- **Agent guides** ([wire.md](docs/agents/wire.md), also served at `/agents.md`, and [analyze.md](docs/agents/analyze.md)):
  - framework recipes for Next.js, Nuxt, SvelteKit, Astro, Remix, Vite SPAs, Angular and server-rendered apps;
  - a strict Content Security Policy;
  - an ad-blocker proxy through the app's domain.
- **Reference apps** in `examples/` (a script-tag app and an SPA), clicked through nightly in a real browser until `verify` is green.

#### Dashboard and access

- Server-rendered pages with no JavaScript and a strict CSP. Only the replay player runs a script, under its own CSP.
- Light and dark themes, and a project switcher.
- **Sign-in:** the admin token, one-time login links, and optional single sign-on with OpenID Connect (code flow with PKCE; invited emails only).
- **Accounts:** admin and viewer users, optional access per project, and invites by login link. The first login creates your admin account.

#### Operations

- A Docker image on GHCR (`ghcr.io/philippe-arnd/openlens`) and a one-click Coolify template, with no required environment variables.
- A healthcheck and a benchmark (about 80k events/s on one vCPU).
- **CI:**
  - a fast PR gate (fmt, clippy, tests, layers, licences);
  - the browser e2e and dashboard screens under the real CSP;
  - the image smoke test;
  - an issue opened automatically when `main` breaks.

### Changed

These changes concern instances that ran pre-release builds:

- **SQL:** `events` is now a view over the stored `raw_events`, where anonymous ids that `identify` linked are replaced by the user. `persons` and `person_aliases` are new tables.
- **User counts:** DAU/WAU/MAU, retention and cohorts count identified users only. Visitors who never logged in show as "not logged in". A project that never calls `identify` still counts everyone.
- **Script tag:** the server defaults to the directory `c.js` came from, path included, instead of its origin. That is the same for `https://lens.example.com/c.js`, and makes a proxied `/_l/c.js` work.
- **SDK `reset()`** does nothing while nobody is identified.

### Fixed

- A visit that starts before login no longer counts as two users.
- A single-page session counts as a bounce: `$scroll`, `$vital` and `$identify` no longer count as activity.
- A task counts once, as its success event, instead of once per event of the feature.
- Retention and stick-driver cohorts use the first `identify` ever, so returning users are no longer taken for new ones.
- Clicks on masked elements send no text or link.

### Security

- Every input is treated as hostile: bounded sizes, a rejection list per event, no `unwrap`/`panic` in production code, and read-only SQL scoped to one project with time, memory and row caps.
- HMAC session cookies (`HttpOnly`, `SameSite=Lax`), and origin checks on dashboard forms.

[Unreleased]: https://github.com/Philippe-arnd/OpenLens/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Philippe-arnd/OpenLens/releases/tag/v0.1.0
