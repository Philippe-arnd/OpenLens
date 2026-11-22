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

## Board

| Data | Board (API, one vCPU) |
| --- | --- |
| 21 k events (demo app, planted patterns) | about 0.4 s (slowest query 41 ms) |
| 368 k events, with identities (2026-11-22) | about **0.8 s** (1.7 s before #159) |
| 2.1 M events, with identities (above the in-memory cap) | about 6–7 s |

**With identities:** `xtask bench` sends realistic visits since #159. A third of the users browse anonymously before logging in, `$identify` links that visit, and one event in seven comes from a visitor who never logs in. So the board pays for the `persons` and `person_aliases` joins like a real app does.

The board takes one in-memory snapshot of the 90-day projection, then filters it for every query (#75). Above 2 M rows in the window it reads the files instead, so memory stays bounded, at the cost of reading them once per query.

**Watch out for subqueries in `OR`.** DataFusion turns `x IN (SELECT …)` into a semi-join, but not when it sits in an `OR` with `NOT EXISTS (…)`. The "identified users" filter written that way cost 20× more: 0.7 s instead of 35 ms per query. A scalar-subquery variant was fast, but it sometimes returned rows out of `ORDER BY` order. The engine now decides in Rust, from the cached `persons` count, and writes either the plain `IN` or `TRUE`. To see what each query costs, run with `RUST_LOG=openlens_query=debug`.

## How to run

```sh
cargo build --profile perf -p openlens -p xtask
OPENLENS_DATA_DIR=/tmp/olb OPENLENS_ADDR=127.0.0.1:18740 OPENLENS_ADMIN_TOKEN=demo taskset -c 0 ./target/perf/openlens &
KEY=$(curl -s -H "authorization: Bearer demo" localhost:18740/api/projects | jq -r '.projects[0].key')
taskset -c 1-3 ./target/perf/xtask bench http://127.0.0.1:18740 "$KEY" 10 4 50   # secs, connections, batch
```

`xtask bench` prints JSON (events/s and latency percentiles), and exits non-zero if any request failed. Delete the data dir and the `perf` build afterwards: see the disk rule in `docs/development.md`.
