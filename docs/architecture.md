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
│    │                 └─ wire (install plans)                      object_store │
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
| L2 | `query`, `flags`, `replay`, `errors`, `heatmaps`, `wire` | Feature logic, each independent of the others |
| L3 | `engine` | Command registry; the only API the upper layer uses |
| L4 | `server`, `mcp`, `ui` | Front ends of the engine: HTTP, agents, browser |
| L5 | `apps/openlens`, `apps/openlens-cli` | Binaries |

## Data model (first cut)

- **Event**: `project_id`, `event` (name), `timestamp`, `distinct_id`, `session_id`, `properties` (JSON, with promoted columns for `$url`, `$feature`, `$role`, `$account`), and `received_at`.
- **Person**: `distinct_id` → properties, including `role`, `account` and `plan`, stored in SQLite and joined at query time. Person properties are also copied onto each event at capture time, so role breakdowns need no join.
- **Feature**: catalog row (`key`, `display_name`, matchers on route patterns or `data-feature`), stored in SQLite. Events carry `$feature` once it is resolved.

## Storage

- Partitioning: `<root>/<project>/<yyyy-mm-dd>/<hh>/<segment>.parquet`. Small segments are compacted hourly.
- The write-ahead log is fsynced per batch. On restart, unflushed segments are replayed.
- Replay blobs are stored separately: `<root>/<project>/replay/<session>/<chunk>.zst`.
- Backends go through the `object_store` crate: local filesystem first, S3-compatible in M9.

## Why not ClickHouse / Kafka / Postgres

The target is a single 1 vCPU / 1 GB VPS run by one person. Parquet + DataFusion gives columnar scans, and one process means one thing to back up and one thing to upgrade. If scale demands it later, the storage layer is the seam for swapping backends.
