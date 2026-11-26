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
| Deploy hook | The project's secret URL that platforms post their deploy notifications to |
| Delivery | On the Releases page: deploy frequency, lead time, change failure rate and time to restore per environment |
| Map drift | Routes new in the latest release that no feature of the map claims |
| Counted with | On a feature's releases: the map version in force when the release came out |
| Period picker | On the board: the form that picks a past release or a date range for the period blocks |
| Board job | A long period of the board computed in the background after *Run it*: its own page, progress and Cancel, kept an hour for whoever started it |
| Feature pulse | Per feature: adoption, uses per user, active days, completion and task time |
| Who uses what | The roles × features grid: the share of each role's users who used each feature |
| Do they come back? | Weekly retention cohorts |
| Came back | Resurrected users: active again after a gap longer than the churn window |
| Why and what to do | Under a Web Vital that is not good: its worst pages, the likely cause and what to fix first |
| Announcement | On Uptime (and the status page): what the team says about the app's state, with a level, components and a start and end |
| Incident update | A dated update of an announcement that follows an incident: investigating, identified, monitoring or resolved |
| Pinned notice | The one information announcement shown at the top of the page while it is active |
