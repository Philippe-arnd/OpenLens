# Analysing an app with OpenLens (agent guide)

Start from the summaries and drill down only where something stands out. Every tool is available as an MCP tool (`openlens-cli mcp`) and over the HTTP API.

| PM question | Tool | Drill down with |
| --- | --- | --- |
| What should we focus on? | `insights`: ranked cards with evidence and next steps, plus paths to value | `sql` |
| Headline KPIs by role, environment or release | `kpi_report` (optional `role`, `env`, `release`) | `board` (everything) |
| Split by account, plan, page or feature | `kpi_report` with `by` (`role`, `account`, `plan`, `page`, `feature`) | `sql` |
| Did the last release move the numbers? | `kpi_report`: `releases` holds the latest two | `kpi_report --release <name>` |
| Is the new release live, still rolling out, or rolled back? | `releases` (optional `env`): state per environment from the share of daily users, first and last seen, users, time in staging | `kpi_report --release <name>` |
| How did a past period go (a date range or an old release)? | `board` with `from`/`to` or `since`; past 30 days of events it asks for `confirm` | `board_job { action: start }` to compute it in the background, then `status` (or `cancel`) |
| Did the release I just shipped break anything? | `release_report { release, env? }`: the verdict (healthy, regressed, inconclusive) with its reasons and every number compared with the release before; `releases` carries each verdict | `errors`, then `user_timeline` of an affected user |
| What shipped in this release? | `release_report { release, env? }` (its `notes` field): the merged pull requests of the release's commit range, read from GitHub once an hour and grouped Added/Fixed/Changed, with commit, PR and file counts; `state` and `reason` say why when they are not there (no GitHub link, no range recorded, not read yet, reads failed). `releases` carries the newest releases' notes | `releases` |
| Which features did this release change? | `release_report { release, env? }` (`notes.touched`): the features whose map `paths` the release's changed files are in, matched once with the map version in force for the release; `note` counts the changed files in no feature (add their folders to a feature's `paths`); `state: no_paths` means the map declares no code paths yet (`openlens-cli rules`). The verdict judges and lists these features first. A feature's releases: `feature_context { key }` (`touched_by`) | `releases` |
| Did OpenLens tell GitHub about the verdict? | `release_report { release }` (its `notes.write_back`): once a production release is healthy or regressed, the hourly pass sets the commit status `openlens/release` on the deployed commit, writes the verdict into its GitHub release, comments once on each shipped pull request (edited when the verdict changes) and opens an issue when it is regressed; `state` is `written`, `writing`, `waiting`, `off` (no GitHub link, or not production), `denied` (the token lacks a permission: `reason` names them) or `failed` | `tickets` (the issue is a ticket too) |
| How often do we ship, and how often does it break? | `delivery` (optional `env`, `days`): deploy frequency, lead time, change failure rate, time to restore per environment | `releases` |
| What did this user do (before churning, before an error)? | `user_timeline { distinct_id }`: role history, recordings, latest events | the replay link it returns |
| Is a server route failing or slower? | `routes` (optional `env`, `days`): requests, 5xx share, p50/p95 per route template and the key actions of the map; `release_report` judges the same routes per release | `errors` (server errors have kind `server`) |
| Does the new release crash-loop, leak memory or block the event loop? | `runtime` (optional `env`): restarts per day, largest RSS and its growth, event-loop delay p99 per release; only a significant rise in restarts per day makes a verdict regressed | `release_report` (its `runtime` field) |
| What breaks, for whom? | `errors`: groups with users, roles, releases, places and the original source line | `user_timeline` of an affected user |
| Are pages slow, and what do we fix first? | `vitals`: each Web Vital explained, p75, rating, thresholds, worst pages and ranked actions | `sql` on `$vital` events |
| Is the wiring complete? | `verify` (`release_layers`: build fingerprint, deploy hook, releases recorded, verdicts) | `feature_map_lint` |
| Did the last deploy break something? | `release_report` after each deploy: act on a `regressed` verdict | `releases`, `errors` with `release` |
| What does production use that the map misses? | `suggested_features`: proposed entries with evidence | `set_feature { feature }`, then commit the YAML it returns |
| Which flags can go? | `stale_flags`: `ready_to_remove` (off for 14 days, or on for everyone on 95% of page views) | `flags` |
| Turn a finding into a ticket | `create_ticket { kind, subject, to: markdown \| github }`: prefilled problem, evidence, hypothesis, success metric | `tickets` (learning log: what moved its metric) |
| Where do our goals stand? | `goals`: status, guardrails, what moved each goal (releases and changes), objectives | `set_goal { kpi, target, deadline \| releases, guardrails }` |
| Is the feature map any good? | `map_health`: score, findings with evidence and a YAML fix | `suggest_map_changes` (patches only) |
| Every feature at a glance | `map_overview`: status, uses, users, success (30 days vs the 30 before), areas, adoption ladder | `feature_context { key }` |
| Everything about one feature | `feature_context { key }`: definition, findings, users and success by role, top pages, ladder, area peers, features used with it, use per release with the map version in force | `sql` |
| Which features go together? | `feature_relations { feature? }`: lift, which comes first, leads to value (30 days of sessions, no cap) | `feature_context { key }` |
| Who leaves, and who is about to? | `churn`: churn rate, survival, where leavers were last, who is at risk now, and the win-back flags already made | `winback_flag { holdout? }`, then `flag_update { key, enabled: true }` |
| Where do users come from? | `acquisition { by?, touch? }`: users, value reached, returning and churned per channel, source, campaign or referrer | `user_timeline` of a user |
| Where do people give up on a form? | `forms`: starts, submits, the field they stop at | the abandoned session's replay |
| How do sessions move between features? | `flows { from?, to? }` | `feature_context { key }` |
| Who uses the app, on what? Who is on it now? | `audience` (devices, browsers, OS, countries, languages, screens), `live` (last 5 and 30 minutes) | `kpi_report` with `by` |
| Is the app up? Will alerts reach us? | `status` around a deploy; `alerts`: uptime, incidents, settings; `availability` for the SLA windows and the error budget, `uptime_days` for 90 days or a month by day, `incidents` for every incident, page by page | `alert_set { …, test: true }` |
| Promise an availability (SLO) | `availability_target { target: 0.999 }`, then `availability`: budget spent and left, burn rate; alerts when spent or burning fast | `alert_set { budget_spent, budget_burn }` |
| A deploy or migration will take the app down | `maintenance_set { action: schedule, minutes, text }` right before, `{ action: end, id }` after: no alert during it, not counted against availability | `maintenance` |
| Did the cron and background jobs run? | `jobs`: per job its state (learning, ok, missed, failed, slower), last check-in, schedule (declared or learned) and usual time | `check_in { slug }`, `job_set { remove }` |
| Anything else | `sql` (read-only DataFusion on `events`) | prune with `date >= 'YYYY-MM-DD'` |
| Deep, offline analysis | `openlens-cli export --from --to --out events.jsonl` (raw events, day by day) | your own tools |

**Map health score:** 100, minus up to 40 for the share of pages and clicks no feature claims, minus 10 per error and 4 per warning, never below 0. A coverage gap of 10% or more of all pages and clicks is an error; every other finding is a warning. Rates are not judged below 20 users. Fix the map first when the score is low: every per-feature number depends on it.

**Suggestions from sessions (#283)**, last 30 days, each a warning with a YAML fix (nothing changes the map until you push it):

- `too_big`: the feature's pages and events fall into two or more groups that sessions do not mix (used together in fewer than 10% of the smaller group's sessions), each group in at least 25% of its sessions and with 20 users; pages in more than 80% of its sessions (a shared entry page) are left out. The fix splits it, one feature per group.
- `too_small`: two features used together in 90% or more of the sessions that use either, each with 20 users. The fix merges the second into the first (`renamed_from` keeps its history).
- `step_order`: two consecutive `steps` happen in the other order in most of the 20 or more sessions that do both. The fix is the observed order.
- `drift`: a route first seen with the current release (from the release states: the newest live release of the environment, or the newest rolling out when none is live, as the board's window takes it), that no feature claims, and that weighs 1% of that release's pages and clicks. It replaces the `coverage` finding for that route.

**Relations** (`feature_relations`, `map_health.relations`, `feature_context.relations`): pairs of features used in the same session by at least 20 sessions, counted over every session of the 30 days (the first 20 features of each), from the session rollup for closed days (D-054). A feature succeeds in a session when one of its success events resolves to it there. `lift` is how many times more often than chance, `first_share` the share of shared sessions where `first` came first, and `leads_to_value` says the success rate of `then` after `first` beats its rate in sessions without `first` (two-proportion test with 30 sessions per side, under the Benjamini–Hochberg cutoff of all the relations tested at a 5% false discovery rate, D-099). They follow the role and release filters.

**Map version in force:** `feature_context.releases` counts each release with the map version saved last before the release's first event (`map_version`), so pushing a new map does not rewrite earlier releases. A release older than every stored version is counted with the oldest (`before_first_map`). The version is recorded the first time a release is counted and kept from then on (`recorded: true`), so it does not move when the release's first event leaves the 90 days.

**Days not available:** `board.unavailable_days` and `map_health.unavailable_days` list the closed UTC days the numbers read that count nobody: their events are gone (cold tier, retention) and their daily rollups were not made with the map in force. Say so instead of reading a drop in those days as a drop in use.

Report findings with the numbers and the cohort sizes behind them, and say when a sample is too small to conclude: cards are hidden below 8 users per group for that reason. The board tests many features, roles and flags at once, so the card families that rest on a test hold it to a 5% false discovery rate per family (Benjamini–Hochberg, D-099): a stick driver (its retention test over every feature), a role blind spot (over every role and feature), a flag regression and a flag win (over every flag shown) appear only when their p-value passes their family's cutoff, on top of their size and lift thresholds. Churn points, friction, funnel drops, slow pages and the release regression card still decide on fixed thresholds alone; read a single one of them as a lead to check, not a proof. In `release_report`, a feature's success rate and a route's 5xx share count as worse only under the cutoff of all the features (routes) compared. Backfilled events carry `"$backfill": true` in `properties`.

In SQL, `events` gives each user the events they sent before logging in: an anonymous id that `identify` later linked (`$anon_distinct_id`) is replaced by the user's id. The stored ids are in `raw_events`, and the links in `person_aliases(anon, person)` (present once at least one anonymous id was linked).
