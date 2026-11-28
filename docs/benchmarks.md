# Benchmarks

Numbers to keep OpenLens honest about "lean". Re-run them when ingestion, storage or the board changes, and update this page in the same PR.

## Ingestion

**Target (M1):** 10,000 events/s on one vCPU. **Measured:** about **80,000 events/s**, with no failed requests.

| Batch × connections | Events/s | p50 | p95 | p99 |
| --- | --- | --- | --- | --- |
| 50 × 4 | 83,300 | 1.8 ms | 3.4 ms | 4.9 ms |
| 50 × 8 | 80,800 | 3.8 ms | 6.5 ms | 8.8 ms |
| 20 × 8 (SDK batch size) | 67,500 | 1.9 ms | 3.4 ms | 4.9 ms |

- **Storage:** 2.3 M events took 25 MB on disk (WAL plus zstd Parquet), about 11 bytes per event.
- **What one event is:** realistic SDK events (page views, clicks with a feature, Web Vitals, feature uses, submits) for 5,000 users, each about 250 bytes of JSON.
- **Setup:** the server ran pinned to one core (`taskset -c 0`) of a 4-vCPU VPS (Intel Haswell, 2026-11-22), using the `perf` profile. The load generator ran on the other cores, over keep-alive HTTP/1.1 to `/capture`.

## Footprint targets (D-097)

What one small server promises for **4 M events in 90 days**, with no setting, measured every night by `perf.yml` (the `scale` job runs 4 M events, realistic profile, under a 1 GiB cap):

| Target | Limit |
| --- | --- |
| Peak memory | ≤ 900 MB (the query pool stops at 768 MiB on any host) |
| Board opened | ≤ 1 s (the default board is computed ahead, D-072) |
| Board computed | ≤ 4 s |
| One filtered board | ≤ 3 s |
| Capture p99 while boards are read | ≤ 50 ms |
| Capacity | ≥ 20,000 events/s |

Since D-072 the page's "cold" load is a cache hit (about 1 ms): what costs is computing the board, which the `scale` summary reports first (`board_compute_ms`). Measured on 2026-11-26 under 1 GiB ([run 45855092155](https://github.com/Philippe-arnd/OpenLens/actions/runs/45855092155)):

| Events | Board computed | Role-filtered board | 4 filtered boards, max | Peak RSS |
| --- | --- | --- | --- | --- |
| 1 M | 1.3–4.3 s | 1.3 s | 4.4 s | 350–390 MB |
| 2 M | 1.4–4.2 s | 1.4 s | 3.6 s | 490–510 MB |
| 4 M | 3.0–7.2 s | 3.0 s | 7.2 s | 650–790 MB |

At 4 M the slowest queries are the success journeys (four 2-s queries a board) and the release adoption scan (a 90-day read of raw events): they are the next work.

## Board

| Data | Board (API, one vCPU) |
| --- | --- |
| 21 k events (demo app, planted patterns) | about 0.4 s (slowest query 41 ms) |
| 368 k events, with identities (2026-11-22) | about **0.8 s** (1.7 s before #159) |
| 2.2 M events, with identities | about **3.7 s** (6–7 s before #190), peak memory 800 MB |

### At scale, under a memory cap (#460, 2026-11-26)

The `scale` job of `perf.yml`, on GitHub's hosted runners (a quiet 4-vCPU machine), `main` at `3b0d8ec` (lot A of #442 and DataFusion 55). Realistic profile of `xtask bench-gen` (#443), 90 days, map pushed. The server is capped with a cgroup v2 `memory.max` (what a container limit does; the query budget follows it, D-033), left to finish its rollups, then the cold board is measured three times, each after a restart. [Run 45730285319](https://github.com/Philippe-arnd/OpenLens/actions/runs/45730285319).

| Events (people) | Cap | Cold board (3 runs) | Server CPU for it | Peak RSS | Data on disk |
| --- | --- | --- | --- | --- | --- |
| 1 M (5,420) | 1 GiB | 0.81 / 0.81 / 0.87 s | 3.2 s | 394–419 MB | 52 MB |
| 1 M (5,420) | 2 GiB | 1.25 / 1.25 / 1.26 s | 4.9 s | 381–387 MB | 52 MB |
| 2 M (11,037) | 1 GiB | 2.64 / 2.54 / 2.50 s | 9.8–10.2 s | 601–613 MB | 102 MB |
| 2 M (11,037) | 2 GiB | 1.82 / 1.86 / 1.85 s | 7.1–7.2 s | 609–627 MB | 102 MB |
| 4 M (22,230) | 1 GiB | 7.16 / 2.12 / 4.31 s | 8.1–13.5 s | 1,051–1,070 MB | 193 MB |
| 4 M (22,230) | 2 GiB | 3.19 / 3.21 / 3.23 s | 12.4–12.7 s | 1,046–1,070 MB | 193 MB |

Every board answered (HTTP 200); none ran out of memory. Memory grows by about 220 MB per million events in the window. At 4 M under 1 GiB the process works at its limit and the times scatter (the kernel reclaims file pages); with 2 GiB they are steady. On most rows the job's second request took about as long as the first, so it did not hit the board's cache; why is not established yet, and warm times are left out here.

**With identities:** `xtask bench` sends realistic visits since #159. A third of the users browse anonymously before logging in, `$identify` links that visit, and one event in seven comes from a visitor who never logs in. So the board pays for the `persons` and `person_aliases` joins like a real app does.

Until #502 the board took one in-memory snapshot of the 90-day projection, then filtered it for every query (#75): about 160 bytes per event (#190), up to 4 M rows, and the files above that. The figures above were measured with it. Since D-051 it keeps no copy: the people, sessions and releases blocks read the daily rollups for closed days (D-040, D-047), success events and Web Vitals are 30- and 7-day scans of their own events, and the other queries read their own window of days from the files. The `scale` job re-measures time and memory; the tables here are updated from its runs.

**Watch out for subqueries in `OR`.** DataFusion turns `x IN (SELECT …)` into a semi-join, but not when it sits in an `OR` with `NOT EXISTS (…)`. The "identified users" filter written that way cost 20× more: 0.7 s instead of 35 ms per query. A scalar-subquery variant was fast, but it sometimes returned rows out of `ORDER BY` order. The engine now decides in Rust, from the cached `persons` count, and writes either the plain `IN` or `TRUE`. To see what each query costs, run with `RUST_LOG=openlens_query=debug`.

## Event mix (#701, 2026-11-28)

What the stored events are made of, on the two apps that send their production events to the owner's instance (Journal and Kanban, from 2026-11-22 to 2026-11-28 10:41 UTC, `raw_events`, production). Both are small (1 to 6 users a day), so the browser numbers are a first look, not a profile; the server numbers do not depend on traffic.

**Browser events per active user-day** (an active user-day is one user with events on one UTC day):

| Event | Journal | Kanban | Kind |
| --- | --- | --- | --- |
| `$click` | 3.9 | 8.0 | interaction (kept) |
| `$vital` | 3.2 | 5.2 | distribution only |
| `$scroll` | 2.8 | 5.6 | distribution only |
| `$pageview` | 2.5 | 2.9 | interaction (kept) |
| `$identify` | 1.4 | 7.0 | sent on every page load, mostly unchanged |
| success and custom events | 1.6 | 4.2 | the signal |
| `$submit`, `$request_failed` | 0.6 | 1.7 | kept |
| total | 16 (225 events, 14 user-days) | 35 (346 events, 10 user-days) | |

**Server events per day**, whatever the traffic (Node SDK, stats window of 60 s on a host with disk to spare, D-091), extrapolated from the first 10 h 41 of 2026-11-28:

| Event | Journal | Kanban | What it held |
| --- | --- | --- | --- |
| `$runtime_stats` | ~1,440 | ~1,440 | one a minute, idle or not |
| `$request_stats` | ~1,490 | ~2,920 | mostly `/health` and `/api/health` probes |
| `$client_stats` | ~2,900 | — | one app calling `localhost` 6 times a minute and refused |

Server telemetry was about **99%** of the events stored on 2026-11-28 (2,598 server events against 28 browser events for Journal, 1,940 against 36 for Kanban).

**Targets** (what the lot A changes aim at):
- **Browser:** at most about **25 stored events per active user-day** on an app of the persona's size (tens of thousands of users), with the distribution-only events (`$vital`, `$scroll`, `$field`) capped by a daily budget (#702, D-097) and `$identify` sent only when something changed (#703).
- **Server:** an idle instance sends **a few hundred events a day** (runtime health every 10 minutes), then grows with the routes really used, never with health checks (#726, D-100).

To measure it again (MCP `sql` or `openlens-cli sql`):

```sql
SELECT event, count(*) AS n,
       count(DISTINCT distinct_id || CAST(date AS VARCHAR)) AS user_days
FROM raw_events
WHERE date >= DATE '2026-11-22' AND environment = 'production'
GROUP BY event ORDER BY n DESC
```
### Persona size (#706)

The target persona is a product manager whose app has tens of thousands of active users. `cargo xtask bench-gen <data-dir> --profile persona --map-out map.yaml` writes that app as it is stored **after** the event budget (D-097), over 90 days:

- **People:** 120,000 known users, each active on a share of days drawn from a Pareto law (α = 1.6) with a mean of about 25 %, so about **30,000 daily active users**. Six raw roles (as the realistic profile), plans free (80 %) and pro (20 %).
- **Browser events:** about **25 stored per active user-day**: one or two sessions a day, each opened by a page view, then clicks (about 25 % of the stored events), page views (12 %), success and custom events (15 %), `$submit` and `$request_failed` (5 %), feature uses and a few errors for the rest, and one `$identify` per active user-day (the SDK sends at most one a UTC day when nothing changed).
- **Distribution-only events, capped as the budget does:** `$vital`, `$scroll` and `$field` only on the user-days the budget draws, about 2,000 a day (share `min(1, 2000 / daily active users)`, so 1 in 15 here), each carrying the share as `$sample`. Clicks carry their position (`$x`, `$y`, `$vw`) only on those days.
- **Anonymous visitors:** one known user in three is first seen anonymous on the landing page (`/`, `/pricing`, `/signup`), and its first `$identify` links that visit with `$anon_distinct_id`; 120,000 more visitors never sign up (three times those signups).
- **Server telemetry** of one app server: `$request_stats` for 30 routes every 10 minutes (24 feature APIs and 6 others, a 5xx row in 3 % of windows) and `$runtime_stats` every 10 minutes, with a process start at each release (every 15 days).

It is deterministic. Its size is derived from these numbers, not chosen: about **68 M events** (about 67 M browser events of known users, 0.5 M of anonymous visitors, 0.4 M of server stats); bench-gen prints the exact count (`events`) and the population (`persona`) in its JSON. `--events N` scales the known users, and the visitors with them, to about `N` events (the server stats stay one server's; under about 8,000 known users the budget measures every session, so no `$sample`); without `--events` the profile is written at full size, and `--events 68000000` is the full size too. `--users` is ignored. `map.yaml` is the realistic profile's 24-feature, 3-role map, which the persona uses as well; with `--map-out` it is stored as the project's map before the rollups.

**Locally:** with the perf build (a debug build is far too slow at this size), `./target/perf/xtask bench-gen /tmp/olp --profile persona --map-out /tmp/olp-map.yaml`, then start the server on `/tmp/olp` as in "How to run" and time the board. The data dir takes several GB: delete it afterwards (`docs/development.md`).

**In CI:** the `scale` job of `perf.yml` (run by hand) with `scale_profile` set to `persona` and `scale_events` to `68000000` (or a smaller count for a scaled-down persona), and the caps to measure in `scale_memory`. The `scale_profile` input comes with its own CI change (rule 14); until it is merged, run it locally.

| Persona, 90 days, budget on, one node | Value |
| --- | --- |
| events (known users, active a day) | not measured yet |
| generation and rollups | not measured yet |
| ingestion (`xtask bench`, events/s on one vCPU) | not measured yet |
| data on disk, total and per day | not measured yet |
| cold board (3 runs), per memory cap | not measured yet |
| server CPU for the board | not measured yet |
| server peak memory | not measured yet |

## How to run

```sh
cargo build --profile perf -p openlens -p xtask
OPENLENS_DATA_DIR=/tmp/olb OPENLENS_ADDR=127.0.0.1:18740 OPENLENS_ADMIN_TOKEN=demo taskset -c 0 ./target/perf/openlens &
KEY=$(curl -s -H "authorization: Bearer demo" localhost:18740/api/projects | jq -r '.projects[0].key')
taskset -c 1-3 ./target/perf/xtask bench http://127.0.0.1:18740 "$KEY" 10 4 50   # secs, connections, batch
```

`xtask bench` prints JSON (events/s and latency percentiles), and exits non-zero if any request failed.

**Steady load and time to visible:** `--rate 200` paces the senders to 200 events/s in all (400 times the average of 4 M events in 90 days) instead of as fast as the server answers, and with `--with-board` each reader loads one page every 5 s, so the board is recomputed on fresh data instead of served from its cache. With `OPENLENS_BENCH_TOKEN` set to the admin token, it also sends one marker event after the warm-up and reports `visible_s` (until `/api/projects/PROJECT/sql` reads it: the WAL is flushed every 10 s) and `board_refreshed_s` (until `/api/projects/PROJECT/board` changes). These are the seconds a PM waits to see what users just did.

**At scale** (#321): `cargo xtask bench-gen <data-dir> [--events N] [--users N]` writes a large synthetic app straight into a data dir: 63 M events over 90 days and 200,000 people by default, in 4 roles, on 20 features, with a release every 15 days. It is deterministic, and it compacts each day at the end. Start the server on the dir and time the board. A debug build writes about 20,000 events/s, so use the perf build for the full size. The `scale` job of `perf.yml` (run by hand, input `scale_events`) does this on a GitHub runner: one job per event count, which generates the app once (or restores it from the Actions cache when the same count and profile were generated the same UTC day, `"cached": true`), then measures each cap of `scale_memory` in turn on a fresh copy, one artifact `scale-EVENTS-CAP` per cap. Its `scale.json` also records how long each wait for the server to go idle took (`idle_seconds`). The perf build is compiled once per run, by the `build` job.

**Realistic profile** (ARCH-10, #443): `cargo xtask bench-gen <data-dir> --events 2000000 --profile realistic --map-out map.yaml` writes what a real product's 90 days look like, for the range D-023 promises (1, 2 and 4 M events): about 160 events per user (12,500 users for 2 M), activity drawn from a Pareto law (α = 1.6) scaled to 20 % of days on average and clamped to 1–95 %, one session per active day (two in 30 % of cases) of Poisson(9) events 25 s apart, 80 % of paths in the session's feature (24 routes `/app/featN/:id`), a `$identify` with `$anon_distinct_id` as each user's first event, six raw roles with case variants, and a release every 15 days. `map.yaml` is the matching 24-feature, 3-role map: push it (`PUT /api/projects/default/feature-map`) before measuring the board. Running it in the `scale` job at 1, 2 and 4 M with the server capped at 1 GiB is a separate CI change.

**Rollups** (D-046): bench-gen then rolls up every closed day with the server's own functions (`days_rolled` and `rollup_seconds` in its output), so the board reads `person_day` and `session_day` for the closed days, as on a server that has run for weeks. With `--map-out` the map is stored as the project's map (version 1) first, so the rollups carry the version the board expects, and pushing the same file again keeps it. `--no-rollups` leaves the rolling to the server's background pass (7 days per 10 minutes), as before.

Measured on 2026-11-24 (perf build, `ubuntu-latest`, 4 vCPU and not pinned, so one vCPU is slower; run 45362175892):

| 63 M events, 200,000 people, 90 days | |
|---|---|
| generation and compaction | 377 s |
| data dir | 2,955 MB (about 47 bytes per event: the synthetic properties compress worse than real traffic) |
| board, first load | 22.0 s |
| board, second load | 12.1 s |
| board with a role filter, first load | 15.2 s |
| server peak memory | 2,590 MB |

At this size the 90-day window was 16 times the snapshot cap of the time (4 M rows), so every board query read the Parquet files: this was the slow path, not the snapshot (D-023). The snapshot is gone since D-051, and these numbers predate the rollups; the `scale` job measures them again.

**Every night** (#314): `perf.yml` runs the perf build on demo data, one minute of ingestion and one minute with `--with-board`, and keeps the JSON as an artifact. `scripts/perf/compare.py` checks it two ways. **Targets** are fixed: at least 20,000 events/s, capture p99 under 50 ms while boards are read, no failed request; a missed target fails the run. **Trend** is the change against the average of the last 7 runs on `main`, failed or not, so the average follows the code instead of freezing on the last green nights (#578); a metric more than 20 % worse comments on one issue without failing. The same night runs the `scale` job at 4 M events (realistic profile, 1 GiB cap); its summary leads with the board's real compute time (the page's cold time is a cache hit since D-072) and the peak memory.

**Ingestion while the dashboard is read** (#344): add `--with-board PROJECT` to compute that project's board in a loop on two more connections. Give it a session in `OPENLENS_BENCH_COOKIE` (for example `ol_session=…`, from the `set-cookie` of `curl -i -XPOST $URL/login -d token=$TOKEN`). The JSON gains a `board` entry (pages computed, p50 and max), and the bench warns when the capture p99 goes above 50 ms. Delete the data dir and the `perf` build afterwards: see the disk rule in `docs/development.md`.
