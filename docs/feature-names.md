# Feature names

Plain words first. Generic category terms (funnel, retention, heatmap, feature flag, session replay) are fine. Only the ideas that are specific to the OpenLens method get a name, and those names are our own.

| Name | What it is |
| --- | --- |
| Focus board | Home screen: KPI strip plus ranked insight cards on where the value is and what to work on |
| Feature map | `openlens.features.yaml`, the declared features and roles of an app |
| Value moment | The early behaviour that best predicts a user staying |
| Stick drivers | Features ranked by how much they lift retention |
| Churn points | Last features and steps seen before users go inactive |
| Role gap | A raw role value that the feature map does not match yet |
| Used with | On a feature page: the features used in the same sessions, which comes first, and whether the first leads to value in the second |
| Automatic release | A release named `YYYY-MM-DD.N` by the server when a new build fingerprint arrives with no release |
| Time in staging | On the Releases page: from a build's first event in staging to its first in production |
| Release verdict | On the Releases page: healthy, regressed or inconclusive, a release against the one before it |
| Server routes | Quality › Routes (#622) and `routes`: requests, 5xx share and response times per server route template, from the server SDK, with a ten-day p95 line and the move since the release before; the Errors page keeps one line and a link |
| Runtime health | In `runtime` and `release_report`: process restarts per day, memory (RSS) and event-loop delay per release, from the Node SDK |
| Key action | A server route a feature of the map declares under `server:`; its regression is named in the verdict and its alert is urgent |
| Deploy hook | The project's secret URL that platforms post their deploy notifications to |
| Delivery | On the Releases page: deploy frequency, lead time, change failure rate and time to restore per environment |
| Map drift | Routes new in the latest release that no feature of the map claims |
| Counted with | On a feature's releases: the map version in force when the release came out |
| Production right now | The status strip at the top of the board: the app, the latest release's verdict, open incidents and the setup left, each in a word |
| Outcomes | On the board: one tile per question (Reach, Value, Retention, Quality, Speed, and Income when there is revenue), each with what it compares with |
| Period picker | On the board: the form that picks a past release or a date range for the period blocks |
| Board job | A long period of the board computed in the background after *Run it*: its own page, progress and Cancel, kept an hour for whoever started it |
| Feature pulse | Per feature: adoption, uses per user, active days, completion and task time |
| Who uses what | The roles × features grid: the share of each role's users who used each feature |
| Do they come back? | Weekly retention cohorts |
| Came back | Resurrected users: active again after a gap longer than the churn window |
| Why and what to do | Under a Web Vital that is not good: its worst pages, the likely cause and what to fix first |
| Planned maintenance | A window declared ahead when components are expected down: shown as *under maintenance*, its incidents not announced, its time not counted in availability |
| Announcement | On Uptime (and the status page): what the team says about the app's state, with a level, components and a start and end |
| Incident update | A dated update of an announcement that follows an incident: investigating, identified, monitoring or resolved |
| Pinned notice | The one information announcement shown at the top of the page while it is active |
| Go to… | The sidebar field that opens a section, a feature or a person by typing its name |
| Availability | On Uptime: time up over time measured per window (24 hours to 365 days, this and last month), time with no check shown as not measured and planned maintenance left out |
| Availability target | One per environment over a rolling 30 days; with it, the error budget (downtime it allows), what is spent and left, and the burn rate |
| Status page | The public, read-only page of a project at `/status/<slug>`: its state, components, 90 daily bars, announcements and planned maintenance |
| Component | A line of the status page: production or a monitored environment, under a public name |
| Job check-ins | On Uptime: cron and background jobs that report each run; OpenLens learns how often they run and alerts when one is missed, fails or gets slower |
