# Clean room

OpenLens is a clean-room implementation of product analytics. The rules are in `AGENTS.md` ("Clean room and licences"). This page adds the details and keeps the source log.

## Allowed sources
- Public product documentation, pricing pages, published screenshots, conference talks and blog posts.
- Open specifications: HTTP, W3C DOM/Beacon/Performance APIs, the rrweb event format, the MCP specification, Parquet and Arrow.
- Papers and textbooks (for example on funnel computation, HyperLogLog, consistent hashing).
- Behaviour observed with **our own** test inputs on a product we are entitled to use. The person observing writes a behaviour spec only, and does not also write the code for it in the same session.

## Forbidden
- Reading the source of PostHog, OpenObserve, or any GPL/LGPL/AGPL project, including "just to check".
- Copying UI text, help text, icons, colours sampled from screenshots, or naming of features (see `feature-names.md`).
- Relying on remembered internals (class names, table layouts, wire field names) that no public source shows.

## Captures
Screenshots and observation notes stay in the gitignored `plan/` folder. They are never committed or attached to issues.

## Source log

| Date | Source | What we learned | Used in |
| --- | --- | --- | --- |
| 2026-11-21 | OpenObserve public site and README-level docs | single binary, Parquet on disk/S3, DataFusion query engine, AGPL licence | D-001, architecture.md |
| 2026-11-21 | storytold/craft-contrib public standards (read as rules, not code) | agent rules structure, clean-room roles, licence allowlist, layered crates | AGENTS.md, deny.toml |
