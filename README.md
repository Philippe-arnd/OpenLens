# OpenLens

**Product analytics you plug in with one dependency.** Self-hosted, open source (AGPL-3.0), one Rust binary.

On first login you land on the **Focus board**: ranked insights on where the value is in your app and what to work on next. Underneath, OpenLens answers the questions product managers actually ask, with almost nothing to configure:

- **What makes users stick, and where they churn:** the features and journeys that predict retention, and the last steps before users leave.
- **Key KPIs by role:** DAU/MAU, churn, session duration and tasks performed; errors and Core Web Vitals; most and least used features and the longest tasks ([definitions](docs/kpis.md)).
- **Who does what:** usage by user role (admin, editor, viewer…), by account and by plan.
- **How each feature is used:** how often, by how many people, whether they come back, whether they like it, and whether it leads to conversion.
- **Where people drop off:** funnels, retention, paths.
- **What they saw:** session replay, heatmaps, and errors linked to the session that hit them.
- **What to ship to whom:** feature flags and experiments.

> Status: **pre-alpha, nothing works yet but `/healthz`.** See [ROADMAP.md](ROADMAP.md).

## Plugging it in (target experience)

```sh
npm install @openlens/web        # or a single <script> tag for any server-rendered stack
```
```js
import openlens from "@openlens/web";
openlens.init({ host: "https://lens.example.com", key: "pk_..." });
openlens.identify(user.id, { role: user.role, account: user.orgId });
```

That is all most apps need. Page views, clicks, forms, errors and features (named from routes and `data-feature` attributes) are captured automatically.

**Or let your coding agent do it.** `openlens-cli wire` (also available as an MCP tool) detects the stack, finds where users log in and what their role is, and proposes one reviewable patch.

## Why another one

- **Lean:** one binary, no Kafka, no ClickHouse, no Redis. Events go to Parquet files on local disk or S3 and are queried with DataFusion. It runs on a 1 vCPU / 1 GB VPS.
- **PM-first defaults:** roles and features are core concepts, not dashboards you have to build yourself.
- **Agent-native:** every UI action is also a CLI command and an MCP tool.
- **Clean room:** written from public documentation and observed behaviour only (see [docs/clean-room.md](docs/clean-room.md)).

## Layout

| Path | What |
| --- | --- |
| `apps/openlens` | The server binary (API, ingestion, UI) |
| `apps/openlens-cli` | Headless CLI: `wire`, `query`, `import`, `mcp` |
| `crates/*` | Engine crates, layered (see [docs/architecture.md](docs/architecture.md)) |
| `sdk/js` | Browser SDK `@openlens/web` |
| `docs/` | Architecture, decisions, clean-room log, parity, gaps |

## Develop

```sh
cargo xtask ci                 # fmt, clippy, tests, layers, licences: what CI runs
cargo run -p openlens          # http://localhost:8686/healthz
```

More in [docs/development.md](docs/development.md). Contributors and agents: read [AGENTS.md](AGENTS.md) first.

## Credits

The architecture is inspired by OpenObserve's public design (single binary, columnar storage, DataFusion), and the working method by the ArtCraft team's shared contribution standards ([storytold/craft-contrib](https://github.com/storytold/craft-contrib)). No code or text was copied from either.
