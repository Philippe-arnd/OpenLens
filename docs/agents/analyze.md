# Analysing an app with OpenLens (agent guide)

Draft, tools arrive in M3. Start from summaries and drill down only where something stands out.

| PM question | Tool | Drill down with |
| --- | --- | --- |
| What makes users stick? | `stickiness_drivers` | `cohort_compare`, `journeys` |
| Where do users churn? | `churn_points` | `user_timeline` on sample ids |
| Which journeys lead to value? | `journeys --to <success event>` | `cohort_compare` retained vs churned |
| How is feature X used, by whom? | `feature_report X` | `sql` |
| Anything else | `sql` (read-only DataFusion) or `openlens-cli export` | schema in `docs/schema/` |

Every result has a summary of 2 kB or less, plus ids for drilling down. Report findings with the numbers and the cohort sizes behind them, and say when a sample is too small to conclude.
