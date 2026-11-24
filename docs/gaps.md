# Known gaps

What is missing or behaves differently from what a user might expect. The UI and the CLI point here when they say "not available yet". Each gap with a plan has a backlog issue.

- **Freshness:** events show on the board within about 10 seconds. The server flushes its write-ahead log to Parquet every 10 s, or every 50,000 events, whichever comes first.
- **One node:** one process with a local disk (the WAL and SQLite have a single writer). On Kubernetes this means one replica with a persistent volume. S3 holds a copy of closed days, not shared storage; with the opt-in cold tier, it becomes the only copy of days older than 120 days (D-032).
- **npm:** `@openlens/web` is not on the npm registry yet. Use the script tag (`/c.js`) or a git dependency meanwhile. Publishing is ready (`.github/workflows/npm.yml`, #156).
- **Web only:** no native mobile SDK. Server-side events go through the Node, Python, Go, Ruby or PHP client, or `POST /capture`.
- **Users are identified people.** DAU/WAU/MAU, retention and cohorts count users who called `identify`; visitors who never logged in are shown apart ("not logged in"). A project that never identifies anyone counts every visitor.
- **Session cut at UTC midnight:** the SDK starts a new session when the UTC day changes, because the browser does not know the project's time zone. KPI days and weeks follow the project's zone (D-017).
- **Funnels** come from the feature map's declared steps. There is no funnel builder by design (D-014): agents run ad hoc funnels with `sql`.
- **Journeys** look at the day before a user's first success only.
- **Insights need history and size:** cards wait for at least 8 users per compared group, and retention-based cards for users older than the churn window. Backfill (`openlens-cli import`) fills the history gap on day one.
