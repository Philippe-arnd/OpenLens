# Changelog

All notable changes to OpenLens are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and OpenLens uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0.0, a minor version (0.x.0) may change behaviour; each such change is listed under **Changed**.

## [Unreleased]

### Added

- **Project settings page** (`/p/{id}/settings`, #219): rename, app address, time zone, capture key with install snippets, key rotation, a wiring check (what `openlens-cli verify` checks), a one-day NDJSON export, and deletion behind typing the project's id. **New project** on `/projects`. Everything agents do with the CLI is now doable from the browser.
- **Project icons from the app's favicon** (#214): the server reads the icon the app's home page declares (or `/favicon.ico`) from the app address, set in settings or detected from pageviews. Only public addresses are fetched, 128 kB at most, images only; refreshed weekly. The project's initial stays as the fallback.

- **Events page filters and families** (#217): an icon and a label per family (page views, clicks, forms, features, logins, errors, vitals, surveys, app events), filters on family, event, user, page and period next to environment, role and release, sorts by time, event or user, and a *next 100* link; all kept in the URL. A user's id links to their events.
- **Active users as a share** (#217): the daily active users tile shows the share of known users (identified in the last 90 days), and the monthly share next to it (docs/kpis.md).

### Changed

- The sidebar's project switch lost its arrow (#214), and gained *Settings*.

## [0.2.0] - 2026-11-22

### Added

- **Fresh base images:** Renovate pins the Dockerfile base images by digest and updates them. Every month CI rebuilds the newest stable image from the same code on a fresh base image, so OS security fixes reach installs on `latest` or a minor channel (#201).
- **Release train:** every Tuesday, CI releases the version in development when the changelog has entries, the nightlies are green and nothing is labelled `release-blocker` or `release-hold`. It then publishes the binaries, image and chart, and moves `main` to the next minor version. No human step (#197).
- **Safe upgrades:** on start, OpenLens backs up the data when the version changed, in `backups/upgrade/` (only the latest copy is kept), before it migrates anything. It refuses to start on data written by a newer release, which stops a rollback to an older image from damaging it. The data directory records both in `openlens-version.json` (#199).
- **A time zone per project** (D-017): days and weeks of every KPI (DAU series, active days, retention weeks, resurrected users, tasks per user-day) start in the team's zone. Set it with `openlens-cli init --timezone Europe/Paris`, `openlens-cli timezone <project> <zone>`, `PATCH /api/projects/{id}` or the MCP `set_timezone` tool. It is `UTC` until set, and the projects page shows it (#152).
- **Server SDKs for Go, Ruby and PHP** (`sdk/go`, `sdk/ruby`, `sdk/php`), standard library only, with the same calls as Node and Python: capture, feature, identify and flags. CI tests each one (#157).
- **Replay player:** the session's events as markers on the timeline (page views, successes, forms, clicks, rage clicks, errors), with a legend, a list of events that follows the playhead (click one to jump), ±10 s, speed 0.5× to 8×, keyboard shortcuts (Space, J/L, ←/→, +/−), and a phone layout (#170).
- **Releases ship Linux aarch64 binaries** next to x86_64 (built on GitHub's ARM runners, free for public repositories).
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

- **Security pass before v0.2:**
  - the capture rate limit reads the client address from the last `X-Forwarded-For` entry, the one the proxy added, so a client can no longer pick its own;
  - the theme and sidebar links refuse to return to a path a browser could read as another site (`/\host`);
  - CI checks dependencies against RustSec advisories (`cargo deny check advisories`).
  - SQLite 3.54.1, which fixes CVE-2026-119375 (RUSTSEC-2026-0459): crafted database and `-wal` files could make queries return stale memory. It is a local vector. rusqlite goes from 0.37 to 0.43.
- **Board on large projects:** the in-memory snapshot keeps only the columns the board reads, and `properties` only for Web Vitals. That is about 160 bytes per event, so the in-memory cap rises from 2 M to 4 M events in 90 days. At 2.2 M events, the board went from 6–7 s to 3.7 s on one vCPU (#190).
- **Faster board with identities:** the identified-users filter is decided once per board (from the `persons` count) instead of as an `OR` of subqueries that DataFusion could not plan as a semi-join, and the all-time totals skip the identity join. The board on 368 k events with anonymous visits went from 1.7 s to 0.8 s on one vCPU. `xtask bench` now sends realistic anonymous visits, and `RUST_LOG=openlens_query=debug` logs what each query costs (#159).
- **New events could stay out of the dashboards until a restart (v0.1.0).** DataFusion caches directory listings with no expiry, and the runtime shared by every query kept the first list of files it saw: events that arrived later, and compacted days, were not read. The listing cache is now off; Parquet footers are still cached. A regression test writes segments between queries of one engine.
- **Phones:** every dashboard page fits a 390 px screen. Grids never get wider than the screen, wide tables scroll inside their card, the navigation scrolls sideways, filters and tiles stack, and touch targets are 44 px. The nightly browser check now opens every page on a phone and fails if one scrolls sideways (#170).
- **Accessibility:** the light theme's muted text, links, badges and statuses now meet WCAG AA contrast (4.5:1), and links inside text are underlined. The nightly browser check runs axe-core on every page, light and dark, and fails on any serious or critical issue (#161).
- Form submits inside web components (open shadow roots) are captured, with their `data-feature` (#151).
- A resent batch is stored once: the server drops event uuids it stored recently (the last 100k), and compaction keeps one row per uuid per day (#150).
- The replay recorder and player bundles now carry rrweb's MIT notice, which minification had stripped (`sdk/js/licenses/rrweb-LICENSE.txt`).

### Changed

- **Image tags are update channels:** `:latest` is now the newest *stable* release, where until now it followed `main`. `:0.2` follows the patches of a minor version, and `:edge` follows `main`. A stable release can redeploy a Coolify instance by itself (secrets `COOLIFY_DEPLOY_URL` and `COOLIFY_TOKEN`), and the dashboard footer shows the version and *Update available* (one daily GET to GitHub, `OPENLENS_UPDATE_CHECK=off` to disable) (#198).
- **Build releases on every merge:** each code change on `main` is published as `vX.Y.Z-YYMMDD-sha` (version in development, UTC date, commit), with the binaries, SDK bundles, chart, checksums, image and notes. They are marked pre-release until `vX.Y.Z` is tagged.
- **Navigation is a sidebar on the left**, collapsible to icons only. Collapse and Expand links remember the choice in a cookie (no script), and phones always get the icon rail. It holds the project switcher, the project's pages (with Digest), the current page marked, the theme and Log out (#170).
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

[Unreleased]: https://github.com/Philippe-arnd/OpenLens/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/Philippe-arnd/OpenLens/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Philippe-arnd/OpenLens/releases/tag/v0.1.0
