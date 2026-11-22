# Architecture

One process, few moving parts. The server takes events over HTTP, appends them to a write-ahead log, flushes them into compressed columnar Parquet files, and answers queries with DataFusion. Metadata (projects, keys, users, flags, feature catalog, saved insights) lives in SQLite. The UI is server-rendered HTML (maud) with SVG charts, served by the same binary; no client-side JavaScript.

```
browser SDK / server SDKs
        │  POST /capture (batched, gzip)        GET /flags        GET /c.js
        ▼
┌──────────────────────────── openlens (one binary) ────────────────────────────┐
│ server (axum) ──► engine (commands) ──► query (DataFusion) ──► storage        │
│    │                 │                    funnel/retention/paths   WAL        │
│    │                 ├─ flags  ├─ replay  ├─ errors ├─ heatmaps    Parquet    │
│    │                 └─ featuremap (map, roles, lint)             object_store │
│    └─ ui (server-rendered)     mcp ◄── openlens-cli mcp         (fs / S3)   │
│                                   meta (SQLite)                               │
└───────────────────────────────────────────────────────────────────────────────┘
```

## Layers

A crate depends only on crates in strictly lower layers. `cargo xtask layers` enforces this, using `xtask/src/layers.rs`.

| Layer | Crates | Role |
| --- | --- | --- |
| L0 | `event` | Event schema, ids, validation of hostile input |
| L1 | `storage`, `meta` | WAL, Parquet segments, compaction, object store; SQLite metadata |
| L2 | `query`, `flags`, `replay`, `errors`, `heatmaps`, `featuremap` | Feature logic, each independent of the others |
| L3 | `engine` | Command registry; the only API the upper layer uses |
| L4 | `ui`, `mcp` | Rendering (HTML) and the agent protocol |
| L5 | `server` | HTTP: capture, API, dashboard |
| L6 | `apps/openlens`, `apps/openlens-cli` | Binaries |

## Data model (first cut)

- **Event**: `uuid`, `event` (name), `timestamp`, `received_at`, `distinct_id`, `session_id`. Promoted columns: `url`, `path`, `feature`, `role` (raw), `account`, `plan`, `environment`, `release`. Everything else stays in `properties` (JSON). The `project` and `date` columns come from the partition directories.
- **Person**: `distinct_id` → properties, including `role`, `account` and `plan`, stored in SQLite and joined at query time. Person properties are also copied onto each event at capture time, so role breakdowns need no join.
- **Feature map**: pushed from the app repo (`openlens.features.yaml`) and stored, versioned, in SQLite. Features and canonical roles are resolved at query time, so editing the map re-labels history without rewriting any files.

## Storage

- Partitioning: `<root>/events/project=<p>/date=<yyyy-mm-dd>/<segment>.parquet` (daily: hourly partitions made 24× more small files and slowed every query). Closed days compact to one file; the current day compacts once it holds more than 24 segments.
- The write-ahead log is fsynced per batch. On restart, unflushed segments are replayed.
- Replay chunks are stored separately, one zstd file per chunk: `<root>/replay/project=<p>/<session>/<seq>.zst`, plus `info.json` (owner, start). Sessions are capped at 32 MB compressed.
- Retention is fixed (D-015): event days older than 400 days and recordings idle for 30 days are deleted by the background job.
- Backends go through the `object_store` crate: local filesystem first, S3-compatible in M9.

## Why not ClickHouse / Kafka / Postgres

The target is a single 1 vCPU / 1 GB VPS run by one person. Parquet + DataFusion gives columnar scans, and one process means one thing to back up and one thing to upgrade. If scale demands it later, the storage layer is the seam for swapping backends.
