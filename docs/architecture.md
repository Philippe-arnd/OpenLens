# Architecture

One process, few moving parts. The server takes events over HTTP, appends them to a write-ahead log, flushes them into compressed columnar Parquet files, and answers queries with DataFusion. Metadata (projects, keys, persons and role history, flags, feature maps, login links) lives in SQLite. The UI is server-rendered HTML (maud) with SVG charts, served by the same binary. The only script on the dashboard is the replay player, on the replay page.

```
browser SDK (@openlens/web, /c.js)          agents (openlens-cli, MCP)
   │ POST /capture  GET /flags  POST /r (replay)        │ HTTP API + admin token
   ▼                                                    ▼
┌──────────────────────────── openlens (one binary) ─────────────────────────────┐
│ server (axum) ──► engine (board, insights, KPIs) ──► query (DataFusion)        │
│   │ capture · api · dashboard      │                    │                       │
│   │                                ├─ featuremap (roles, features, lint)        │
│   │                                ├─ flags · replay · errors (source maps)     │
│   ▼                                ▼                    ▼                       │
│ ui (maud, no JS)              meta (SQLite)      storage: WAL → Parquet (zstd)  │
└────────────────────────────────────────────────────────────────────────────────┘
                     one data dir: events/ wal/ replay/ sourcemaps/ meta.db
```

## Layers

A crate depends only on crates in strictly lower layers. `cargo xtask layers` enforces this, using `xtask/src/layers.rs`.

| Layer | Crates | Role |
| --- | --- | --- |
| L0 | `event`, `sqlgen` | Event schema, ids, validation of hostile input; the one place SQL values are bound or quoted (D-027) |
| L1 | `storage`, `meta` | WAL, Parquet segments, compaction, retention; SQLite metadata |
| L2 | `query`, `flags`, `replay`, `errors`, `featuremap` | Feature logic, each independent of the others |
| L3 | `engine` | Board, insights, KPIs, pages (scroll, clicks), error groups: the only API the upper layers use |
| L4 | `ui`, `mcp` | Rendering (HTML) and the agent protocol |
| L5 | `server` | HTTP: capture, API, dashboard |
| L6 | `apps/openlens`, `apps/openlens-cli` | Binaries |

## Data model

- **Event**: `uuid`, `event` (name), `timestamp`, `received_at`, `distinct_id`, `session_id`. Promoted columns: `url`, `path`, `feature`, `role` (raw), `account`, `plan`, `environment`, `release`. Everything else stays in `properties` (JSON); `$vital` facts are also written as typed `vital_*` columns (D-037). The `project` and `date` columns come from the partition directories.
- **Person**: `distinct_id` → properties, including `role`, `account` and `plan`, stored in SQLite and joined at query time. Person properties are also copied onto each event at capture time, so role breakdowns need no join.
- **Feature map**: pushed from the app repo (`openlens.features.yaml`) and stored, versioned, in SQLite. Features and canonical roles are resolved at query time, so editing the map re-labels history without rewriting any files.

## Storage

- Partitioning: `<root>/events/project=<p>/date=<yyyy-mm-dd>/<segment>.parquet` (daily: hourly partitions made 24× more small files and slowed every query). Closed days compact to one file; the current day compacts once it holds more than 24 segments.
- Rollups of closed days (#322, D-025): `<root>/rollups/v1/project=<p>/date=<d>/{person_q,metric_q,click_q,label_q,scroll_q}.parquet`: one row per person or per metric and 15-minute bucket, and per page for clicks and scroll (D-028), written by the background job once a day has been over for a full day (seven days per pass while an older history is caught up). They are kept after their events expire (D-026), so they are part of backups. Queries see them as the `person_q` and `metric_q` tables. The board does not read them yet (#323).
- The write-ahead log is fsynced per batch. On restart, unflushed segments are replayed.
- Replay chunks are stored separately, one zstd file per chunk: `<root>/replay/project=<p>/<session>/<seq>.zst`, plus `info.json` (owner, start). Sessions are capped at 32 MB compressed.
- Source maps are stored per release, `<root>/sourcemaps/project=<p>/release=<r>/<bundle>.map`, uploaded by `openlens-cli sourcemaps`, and used to show each error group's original frame.
- Retention is fixed (D-015): event days older than 400 days and recordings idle for 30 days are deleted by the background job.
- One local data directory (one volume to back up with `openlens backup`), the source of truth. Optionally, closed days and a daily metadata snapshot are copied to S3-compatible storage (D-016); with the opt-in cold tier, days older than 120 days then live on S3 only (D-031, D-032).

## Why not ClickHouse / Kafka / Postgres

The target is a single 1 vCPU / 1 GB VPS run by one person. Parquet + DataFusion gives columnar scans, and one process means one thing to back up and one thing to upgrade. If scale demands it later, the storage layer is the seam for swapping backends.
