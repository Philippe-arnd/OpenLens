# Known gaps

What is missing or behaves differently from what a user might expect. The UI and the CLI point here when they say "not available yet". Each gap with a plan has a backlog issue.

- **Freshness:** events show on the board within about 10 seconds. The server flushes its write-ahead log to Parquet every 10 s, or every 50,000 events, whichever comes first.
- **One node:** one process with a local disk (the WAL and SQLite have a single writer). On Kubernetes this means one replica with a persistent volume. The S3 option is a backup copy, not shared storage.
- **npm:** `@openlens/web` is not on the npm registry while the repository is private. Use the script tag (`/c.js`) or a `file:`/git dependency. Publishing is ready (`.github/workflows/npm.yml`).
- **Web only:** no native mobile SDK. Server-side events go through the Node or Python client, or `POST /capture`.
- **Users are identified people.** DAU/WAU/MAU, retention and cohorts count users who called `identify`; visitors who never logged in are shown apart ("not logged in"). A project that never identifies anyone counts every visitor.
- **UTC days:** every daily KPI, session cut and retention week uses UTC.
- **Funnels** come from the feature map's declared steps. There is no funnel builder by design (D-014): agents run ad hoc funnels with `sql`.
- **Journeys** look at the day before a user's first success only.
- **Insights need history and size:** cards wait for at least 8 users per compared group, and retention-based cards for users older than the churn window. Backfill (`openlens-cli import`) fills the history gap on day one.
- **Rare duplicates:** a send that reached the server but whose answer was lost is sent again, with the same event ids. Events are not deduplicated yet.
- **Shadow DOM:** clicks inside open shadow roots are captured; form submits inside them are not yet.
- **Sign-in by email** needs OIDC or a login link from the CLI: there is no "email me a link" on the login page yet, even with SMTP set.
