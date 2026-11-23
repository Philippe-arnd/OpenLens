# Changelog

All notable changes to OpenLens are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and OpenLens uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0.0, a minor version (0.x.0) may change behaviour; each such change is listed under **Changed**.

## [Unreleased]

### Added

- **From finding to ticket to measured effect** (#245, the DMAIC loop):
  - **Create ticket:** a link on insight cards, error groups, goals not yet on track, and feature-map findings.
  - **What the ticket holds:** the problem, the evidence, links, a hypothesis, a success metric with its target and check date, and acceptance criteria. It is written for a person or a coding agent.
  - **Where it goes:** copy it as Markdown into any tracker, or open a GitHub issue labelled `openlens`. The repository and a token, stored encrypted, are set on the new **Tickets** page. There is one issue per finding: a second click comments fresh numbers on the open issue instead of opening another.
  - **Follow-up:** each hour, OpenLens follows the issue until it is closed, finds the release that shipped it, and 14 days later measures the metric before and after. Shares get a significance test. It comments **improved**, **no effect** or **worse** on the issue.
  - **Learning log:** the Tickets page counts the changes that moved their metric.
  - **API and MCP:** `GET|POST /api/projects/{id}/tickets`, and MCP `create_ticket` and `tickets`.
  - **Control:** nothing is posted without a click or an admin call.

- **Ask OpenLens** (#244, docs/ai.md, D-020): an AI chat on every project (**Ask** in the menu).
  - **How it answers:** in your words, with the numbers behind each claim and links to the pages they come from. It knows the page you came from, and it suggests follow-up questions.
  - **Read tools:** KPIs, insights, a feature's context, errors, goals, and aggregate-only SQL.
  - **Changes:** setting a goal or switching or rolling out a flag is only a proposal. It is applied after an admin's click, through a single-use token from the server.
  - **Providers:** `anthropic` (with prompt caching) or any `openai-compatible` server (OpenAI, a LiteLLM proxy, OpenRouter, Azure, Mistral, vLLM, a local Ollama). Configure it by `OPENLENS_LLM_*` or in **Settings → AI**, where the key is stored encrypted and a *Test* button checks it.
  - **Data sent:** aggregates and definitions only; person-level detail must be allowed per project. *What is sent* shows the exact last request.
  - **Limits:** a daily token budget per project and 30 questions an hour per person. Tokens and cache hits are shown under each answer.
  - **History:** conversations are kept 30 days, can be exported to Markdown, and can be forgotten.

- **Release process, part 2** (#258):
  - **Notes draft:** a Monday PR adds a draft of `docs/releases/vX.Y.Z.md` in the house template, built from the CHANGELOG. Merge it before the train to publish it as the notes.
  - **Screenshots PR:** lists the images that changed, with before and after links. Changes under 0.5% of pixels are ignored.
  - **Release summary:** an hour after the train tags, a table checks the GitHub release, the publish, image and chart runs, the image on GHCR, the site, and the reference instance (when the `OPENLENS_PROD_URL` variable is set).
  - **`GET /version`:** a new public endpoint returns the running version. `/healthz` stays a bare `ok`.
- **Churn, part 2** (#269): the churn page now shows why users leave and who came back.
  - **Top reasons:** a ranked list, each reason with its numbers:
    - features that users who stay tried in their first 14 days (only users old enough to have had those 14 days count, so a longer life does not bias it);
    - friction on the last day (errors, rage clicks, failed requests, poor Web Vitals);
    - the place they left from;
    - the release they arrived on.
  - **Survival by role.**
  - **Win-back:** users who came back after churning, the release and feature they returned to, and how active they have been since.
  - **Explicit cancel:** the feature map may declare `churn_event` (D-019). A user who sends it counts as churned at once, and the answers of surveys triggered on it show as the exit survey.
  - **Users at risk:** admins can export them as CSV or JSON (audited, with CSV formulas neutralised). They can also turn the list into a flag that starts off, with a control group held out (`users` and `holdout` on flags). The churn page compares how active the nudged and held-out users were over the last 7 days.

- **Goals, part 2** (#272):
  - **Statuses for every KPI:** DAU, MAU, stickiness and feature success rates now have weekly series, so their status is no longer stuck at "not enough data". Web Vitals (LCP, INP, CLS at p75) can be goals too.
  - **Guardrails:** a goal can have up to two KPIs that must not get worse. The default limit is 5% worse than now, and it can be edited through the API. A breach is shown in red and raised as an alert (through the regression alerts toggle), and the alert closes once the KPI recovers.
  - **Objectives:** a sentence with 1–3 goals as key results and an end date. An objective takes the status of its weakest key result. When the period ends, it is archived as achieved or missed, with the final values. Open objectives that are at risk show at the top of the board.
  - **Deadlines in releases:** "in N releases", estimated from the median time between releases so far.
  - **What moved it:** the releases that changed the KPI most since the goal was set, with their effect, plus the flag, survey and map changes and notes in that period.
  - **Digest:** goals and objectives at risk now appear in the digest.
  - **API, MCP and CLI:** `POST /api/projects/{id}/goals` and the MCP tools `goals` and `set_goal`.

- **Flag targeting by property** (#279): a flag can carry up to 10 rules (`plan in pro, team`, `country not in FR`), all of which must match, on top of roles; accounts still bypass targeting. Rules test `plan`, `country` (from the address when not sent), `role`, `account` or any property passed to `identify(id, { properties })`; a user who did not send the property does not get the flag. The browser SDK and every server SDK (`plan`, `properties`) send them to `/flags`, which never stores them. Edit rules on the flag page, `openlens-cli flags set --rule '…'`, or MCP `flag_set { rules }`. The variants readout now compares each arm with the baseline (`control`, else the first variant): two-proportion p-values for the value and error rates, held to 0.05 shared between the arms (Bonferroni), and "too few" under 30.
- **Feature map quality** (#240): the map can declare `area` (one-level grouping), `requires` (features used first; unknown keys and loops are refused) and `renamed_from` (former keys, whose events count for the new one). The map page measures the pushed map on 30 days of usage: a score out of 100 and findings ranked by impact, each with evidence and a YAML fix (pages and clicks no feature claims, overlapping routes, dead features, success events that never fire or fire for everyone, undeclared `data-feature` values, `requires` usage contradicts), roll-ups by area and an adoption ladder. Versions keep who pushed them and why (`features push -m`). API: `GET /api/projects/{id}/feature-map/health`, `/feature-map/features/{key}`, `/feature-map/versions`; CLI: `features lint --live [--min-score N]`, `features context <key>`; MCP: `map_health`, `map_overview`, `feature_context`, `suggest_map_changes`. Push maps with the new fields only to an upgraded server: older ones refuse unknown fields.

- **Alerts, part 2** (#266): the app's TLS certificate is read hourly (a handshake on public addresses only) and warns at 14 days, an expired one is urgent; a spike of users hitting failing requests or errors in 15 minutes, against the same quarter-hour of the weekday over four weeks; quiet hours in the project's time zone hold non-urgent alerts until they end (*down* and an expired certificate always go out); an incident that lasts is repeated at most hourly; the board shows a banner while the app is down.
- **Experiments: flag variants** (#252): a flag can carry 2–6 weighted variants; users who get the flag are split between them deterministically (a user keeps their variant when the rollout grows). `/flags` returns `variants`, the SDK adds `openlens.variant(key)` and tags events with `$variants`, and the flag's page compares the arms (users, value reached, sessions with errors). Plain on/off flags are unchanged.
- **Import history from another tool's export** (#233): `openlens-cli import --format csv|jsonl --map event=…,distinct_id=…,timestamp=…[,url=…,referrer=…,session_id=…]` maps any per-event export onto OpenLens events (RFC 4180 CSV, ISO or Unix times, other columns kept as properties, unreadable rows reported); imported events are marked `$backfill` as before.
- **Cookieless mode** (#273): `data-cookieless` on the script tag (or `init({ cookieless: true })`) keeps the SDK's state in memory only; the server derives anonymous visitors from a daily rotating salted hash of the address and user agent (address not stored, salt overwritten daily) and sessions from 30 minutes of activity, and never links the daily id to a login. Replay is off in this mode. See docs/privacy.md.
- **Timeline of changes** (`/p/{id}/timeline`, #267): releases (first seen), flag and survey changes, feature map pushes and goals (from the audit log), alerts, and notes anyone signed in can add, last 90 days; the days with a change are dotted under the active users chart. `GET|POST /api/projects/{id}/timeline`, `openlens-cli annotate --note "…"`.
- **Forms** (`/p/{id}/forms`, #232): the SDK sends one `$field` event when a form field is left (its name or label, time spent, whether it changed; never the value; nothing for passwords or ignored/masked fields). Per form: starts, sends, abandonment, median time to fill, the field people leave from, fields they come back to, and a replay of a session that gave up.
- **Share links** (Settings → Share links, #229): a read-only link to the Focus board or the weekly digest for people without an account, with filters fixed when it is made, an expiry (30 days by default), optional embedding on given https origins (`frame-ancestors`), and revocation; the shared page has no sidebar, filters, SQL, people, replays or settings, and is cached for 5 minutes. Admins only; 50 active links per project; every link made or revoked is in the audit log.
- **Privacy** (#230): *Forget this person* on a profile (and `DELETE /api/projects/{id}/people/{who}/forget`) removes their events and those of their linked anonymous ids from the files at once, their recordings and profile, and drops what they send later; *Export their data* gives everything stored about them as JSON. An append-only **audit log** (`/audit`, `GET /api/audit`) records admin actions: users, keys, settings, flags, surveys, feature maps, storage, goals, exports and erasures. `docs/privacy.md` says what is stored, for how long, and what never is.
- **Goals on KPIs** (`/p/{id}/goals`, #235): pick a KPI (error rate, bounce, session length, tasks per session, stickiness, active users, a feature's success rate), accept the suggested target (the best release so far or a 10% step, whichever is closer) or change it, set a deadline and an owner. Each goal shows the value now, the gap, the way travelled, the trend projected to the deadline and a status (reached, on track, at risk under 10% short, off track, not enough data), with a per-release chart and the target line. `GET /api/projects/{id}/goals`, `openlens-cli goals`.
- **Revenue** (#231): when a feature's success event carries `revenue` (or `value`) and an optional `currency`, the board shows a revenue card (total, payers, per active user) and `/p/{id}/revenue` breaks it down by feature, role and release. One figure per currency unless the feature map sets a display currency with fixed rates (`revenue: { currency: EUR, rates: {...} }`). Nothing changes without amounts.
- **Churn** (`/p/{id}/churn`, #247): how many leave (churn rate, net change, churned per week), after how long (Kaplan–Meier survival and median lifetime, week of life when they leave), at which moment (the last feature or page against where users who stayed were last seen), friction (errors on the last day), and who is at risk now (quiet for over half the window and slowing down, linked to their profile). Suggests a `churn_after_days` from the gaps between active days, without changing it. `GET /api/projects/{id}/churn`.
- **Where users come from** (`/p/{id}/acquisition`, #226): channels grouped automatically (direct, search, social, email, paid, referral, AI assistants, internal) from the referrer and the campaign parameters, which the SDK now sends on the first page view of a visit (`utm_*`, `gclid`, `fbclid`, `msclkid`). By channel, source, campaign or referrer, first or last touch (direct visits don't replace the last touch): users, the share reaching the value moment, coming back, and churned; under 20 users is flagged. Only the referrer's host is stored, never its path or query. *Break down* gains *channel*.
- **Regression alerts** (#227): every hour, the board's regression cards (a release or a flag with more errors, pages turning slow) become alerts (one message when a card appears, one when it is gone), and a new error group hitting two users or more in the last hour is announced once; same channels as Uptime, switchable per project.
- **Is the app down? Uptime and alerts** (`/p/{id}/uptime`, Settings → Alerts, #256): with the app address set, OpenLens checks it every minute (down after two failed checks: no answer or HTTP 5xx; public addresses only) and keeps 7 days of checks; availability over 24 h and 7 days, response times, incidents. *Traffic stopped* is learned from four weeks of history at the same hour and weekday. One message when an incident starts (after a wait, 2 min by default) and one when it ends, by email, a Slack/Discord-compatible webhook or ntfy (phone push), with *Send a test alert*. Rules can be switched off or tuned per project. `GET /api/projects/{id}/status`, `openlens-cli status` (non-zero when down).
- **Live** (`/p/{id}/live`, #228): users active in the last 5 and 30 minutes, errors, and over 30 minutes the releases, roles, features and pages in use, plus the latest events; refreshed every 10 seconds by the browser (a `Refresh` header, no script). Handy right after a deploy.
- **Feature flow** (`/p/{id}/flows`, #224): a Sankey of how sessions move between the features of the map (repeats merged, 5 steps, the 8 widest per step, the rest as Other), ending in *success* or *exit* (drop-offs in red). Views: from the start, paths to success, after a feature; the board's filters apply; a table view. Server-rendered SVG, no script. `GET /api/projects/{id}/flows`, `openlens-cli flows [--from <feature>] [--to-success]`.
- **Audience** (`/p/{id}/audience`, #225): a world map of users per country (server-rendered SVG, one hue on a square-root scale, a table view), and devices, browsers, systems, languages and screen sizes. Page views get `$country`, `$device`, `$browser` and `$os` at capture; the client address is looked up in the DB-IP Lite country file shipped in the Docker image (CC BY 4.0) and never stored. The SDK adds `$lang` and `$sw` to the first page view. *Break down* gains country, device, browser and OS.
- **Storage and backups in the dashboard** (`/settings/storage`, #218): set the S3-compatible destination (bucket, region, endpoint, prefix, keys; the secret is sealed and never shown; `OPENLENS_S3_*` still win and show read-only), test it (write, read back, delete a probe), see the last copy and error. Schedule backups daily or weekly at an hour (UTC), keep the newest N locally and optionally on S3 as `.tar.gz`, see the history, back up now, download the latest copy. Failures email admins when SMTP is set. `openlens backup` and the CronJob are unchanged.
- **Screenshots retaken by each release** (#246): `screenshots.yml` takes the README and site images from the released UI on demo data and opens a PR with them; a `release` skill (`.claude/skills/release/`) is the agents' checklist to release, hold, write notes, hotfix, check and roll back.
- **Project settings page** (`/p/{id}/settings`, #219): rename, app address, time zone, capture key with install snippets, key rotation, a wiring check (what `openlens-cli verify` checks), a one-day NDJSON export, and deletion behind typing the project's id. **New project** on `/projects`. Everything agents do with the CLI is now doable from the browser.
- **Project icons from the app's favicon** (#214): the server reads the icon the app's home page declares (or `/favicon.ico`) from the app address, set in settings or detected from pageviews. Only public addresses are fetched, 128 kB at most, images only; refreshed weekly. The project's initial stays as the fallback.

- **Events page filters and families** (#217): an icon and a label per family (page views, clicks, forms, features, logins, errors, vitals, surveys, app events), filters on family, event, user, page and period next to environment, role and release, sorts by time, event or user, and a *next 100* link; all kept in the URL. A user's id links to their events.
- **Active users as a share** (#217): the daily active users tile shows the share of known users (identified in the last 90 days), and the monthly share next to it (docs/kpis.md).

- **Times in the reader's time zone** (#215, D-018): *My profile* (`/me`) sets the zone each person reads times and days in; the project's zone stays the default (and the digests' zone). The API takes `?tz=`, the CLI `--tz` / `OPENLENS_TZ`.
- **Team management** (#216): names, a status per user (active, invited with its expiry, expired, disabled), last sign-in, search, an edit page (name, email, role, projects; the id and sessions survive an email change), new login link or resent invitation, revoke, disable/enable sign-in (sessions end at once, also for SSO), and removal behind typing the email. The last admin can't be demoted, disabled or removed (the owner token always remains). `PATCH /api/users/{id}` and `openlens-cli users list|update|disable|enable|delete`.

- **The replay player fits the screen** (#212): the recording is scaled to the room left on screen (contain, up to 2×), so a desktop session fits a phone and a phone session is readable on a wide screen; the controls and timeline take the video's width; a line shows the device kind, the recorded size and the zoom; *1:1* (or Z) shows actual size. The event list sits beside the video from 1024 px wide.

- **Comments on replays** (#213): pause, write, and the comment is pinned to that moment: a marker on the timeline, an entry in the event list, a list under the player with links that open the replay there (`?t=<ms>#c<id>`). Viewers comment too; authors and admins delete. Session lists show the count. API `GET|POST /api/projects/{p}/replays/{s}/comments`, CLI `openlens-cli comments <session> [--at 12.4 --add "…"]`.

- **Flags and surveys managed from the dashboard** (#220): *New flag* / *New survey*, an edit page for every field (key, description, rollout, roles, accounts; question, answer kind, choices, trigger, sample, roles), renames, and deletion behind typing the key, with the models' own validation shown on the form. Admins only.

- **People and data explorer** (#222): a person's profile (role over time, recordings, errors, latest events), linked from events and errors; a *Break down* page by role, plan, account, environment, release, page or feature; read-only SQL for admins with the schema, the API's limits and a CSV download.
- **Feature map in the dashboard** (`/p/{id}/features`, #221): every feature with its entry, success event and live status (uses and users over 7 days, *not seen*, *no success*), lint errors and warnings, roles sent that no role matches, history of versions, YAML and JSON downloads. Admins edit the YAML, check it (lint and a diff against the stored map), then save it as a new version.

### Changed

- The sidebar's project switch lost its arrow (#214), and gained *Settings*.

### Fixed

- **Ask's SQL without person-level detail runs on an aggregate-only view** (D-021): no ids, URLs, properties or people tables; people are counted with `count(DISTINCT user_key)`; groups of fewer than 5 people are left out.
- **The board is computed once and shared** (#291): pages that show it reuse one result per project and filters until new data lands, concurrent requests wait for the same computation, and at most 2 boards are computed at once. A second visit answers from memory instead of recomputing for seconds.
- **Page URLs are stored without their query string** (#290), as `docs/privacy.md` promised: the query and the fragment are dropped at capture, except the campaign tags `utm_*`, `ref` and `gclid`. URLs already stored are cleaned once at startup.
- The churn page failed with a feature map pushed ("last place" query). It now works with and without a map.
- **Going back a release right after an upgrade works again**: two starts in the same second named their upgrade copy the same, and the second refused to start. Copies now take the next free second. (The upgrade check waits between steps for releases up to v0.2.0.)

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
