# Known gaps

What is missing or behaves differently from what a user might expect. The UI and the CLI point here when they say "not available yet".

- **Freshness:** events show on the board within about 10 seconds. The server flushes its write-ahead log to Parquet every 10 s, or every 50,000 events, whichever comes first.
- **npm:** `@openlens/web` is not on the npm registry while the repository is private. Use the script tag (`/c.js`) or a `file:`/git dependency.
- **One admin:** there is one admin token plus one-time login links, and no per-user accounts or permissions. OIDC is in Later (D-015).
- **Heatmaps** are lists (most-clicked elements, rage clicks, scroll depth per page), not an overlay drawn on the page.
- **Funnels** come from the feature map's declared steps. There is no funnel builder by design (D-014): agents run ad hoc funnels with `sql`.
- **Journeys** look at the day before a user's first success only.
- **Insights need history:** retention-based cards wait until enough users are older than the churn window. Backfill (`openlens-cli import`) fills that gap on day one.
- **Anonymous visitors** count in events and sessions, but role-based KPIs only cover identified users.
