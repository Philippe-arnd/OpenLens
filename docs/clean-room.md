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
| 2026-11-21 | Public feature lists and pricing pages of hosted product-analytics suites | which capabilities the category offers, described in our own words | target-app-parity.md |
| 2026-11-21 | rrweb README and event format (MIT; used as a dependency, not reimplemented) | recorder and replayer options, masking options | sdk/js replay, player |
| 2026-11-21 | web.dev public pages on Core Web Vitals | LCP, INP, CLS, FCP, TTFB thresholds (good / needs improvement / poor) | sdk/js vitals, kpis.md |
| 2026-11-21 | W3C/WHATWG specs: Beacon, Fetch keepalive, History API, Page Visibility, Performance Timeline | delivery on page hide, SPA route detection | sdk/js |
| 2026-11-21 | Apache Parquet and Arrow specs, DataFusion user guide | storage layout, SQL surface, listing tables | storage, query |
| 2026-11-21 | MCP specification (public) | stdio transport, tools list and call shapes | openlens-cli mcp |
| 2026-11-22 | Source Map Revision 3 proposal (public) | VLQ mappings, sources, names | crates/errors |
| 2026-11-22 | Statistics textbooks: two-proportion z-test, Welch's t-test | significance on flag cohorts | engine stats.rs |
| 2026-11-22 | OpenID Connect Core 1.0, RFC 6749 (OAuth 2.0), RFC 7636 (PKCE) | code flow, ID token claims and checks | server oidc.rs |
| 2026-11-22 | AWS Signature Version 4 public documentation and its published example | canonical request, string to sign, signing key; checked against the example | server s3.rs |
| 2026-11-22 | RFC 5321 / RFC 5322 (SMTP, message format) | digest email | server digest.rs |
| 2026-11-22 | Anthropic Messages API public documentation | request shape, structured output | server reword.rs |
| 2026-11-22 | WHATWG HTML (bfcache `pageshow`, prerendering), DOM spec (`composedPath`, shadow roots), CSP Level 3 | SDK visit and click capture, CSP guide | sdk/js, wire.md |
| 2026-11-22 | Public framework docs: Next.js, Nuxt, SvelteKit, Astro, Remix, Vite, Angular; Nginx, Caddy, Traefik, Vercel, Netlify | client-only init, build-time env, rewrites and proxies | wire.md |
| 2026-11-22 | npm docs (trusted publishing, provenance), Keep a Changelog 1.1.0, SemVer 2.0.0 | publishing and release process | npm.yml, CHANGELOG.md, development.md |
| 2026-11-22 | OpenObserve RUM, used as a product on Phil's own apps (observed behaviour only) | kept in parallel during dogfooding to compare counts | ROADMAP M6 |

## Review (2026-11-22, v0.1.0)

A pass over every mention of another product in the repository:
- **PostHog** is named only in the rules that forbid reading its source (`AGENTS.md`, this page). Feature names are our own (`feature-names.md`), and the parity page describes the category's capabilities in our own words, without naming a vendor.
- **OpenObserve** is credited for its public architecture (README, release notes), and named in M6 as the RUM product running in parallel on Phil's apps. No source was read.
- **ArtCraft / storytold/craft-contrib** is credited for the structure of the agent rules. Its text is not copied (it has no licence); `AGENTS.md` is written in our own words.
- **Third-party code shipped:** rrweb (MIT) in the recorder (`/r.js`) and the player (`/assets/player.js`, `/assets/player.css`). Both are recorded in `ATTRIBUTION.md`.

| 2026-11-22 | Public feature pages of Matomo, Plausible, Mixpanel, Amplitude and Umami (PostHog's pages returned no readable content) | capabilities common to the category: path flows, world map and devices, acquisition and UTM, alerts and annotations, live view, share links, privacy tools, revenue, form analytics, importers | M11 issues #224–#233, target-app-parity.md |
