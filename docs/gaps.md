# Known gaps

What is missing or behaves differently from what a user might expect. The UI and the CLI point here when they say "not available yet".

- Insight cards cover stick drivers, the value moment, churn points, friction, role blind spots, release regressions and slow pages. Journeys look at the day before a user's first success only.
- The board runs ~16 queries per load (several seconds on a debug build with 20k events): issue #75.
