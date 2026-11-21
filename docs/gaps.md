# Known gaps

What is missing or behaves differently from what a user might expect. The UI and the CLI point here when they say "not available yet".

- `openlens-cli` commands (`init`, `features`, `import`, `verify`, `login-link`) are not there yet: use the HTTP API (`docs/agents/wire.md`). ROADMAP M2.
- The Focus board shows KPIs only. Insight cards (stick drivers, churn points, journeys) need the feature map: ROADMAP M3.
- Roles are shown raw. Canonical roles from the feature map's `roles:` arrive in M2/M3.
- `openlens-cli mcp` validates its roots, then exits with "not implemented yet".
