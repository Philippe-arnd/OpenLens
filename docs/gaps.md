# Known gaps

What is missing or behaves differently from what a user might expect. The UI and the CLI point here when they say "not available yet".

- Insight cards cover stick drivers, the value moment, churn points, friction, role blind spots, release regressions and slow pages. Value journeys (paths into success events) and release-vs-release comparison on every KPI are not there yet: ROADMAP M3.
- The board runs ~16 queries per load (several seconds on a debug build with 20k events): issue #75.
