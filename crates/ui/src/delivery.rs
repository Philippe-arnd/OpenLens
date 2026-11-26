//! The delivery card of the Releases page (#523, D-068): deploy frequency,
//! lead time, change failure rate and time to restore per environment,
//! from the engine's `releases.delivery`.

use super::*;
use crate::components::tile;
use openlens_engine::delivery::{Delivery, Metrics, Source};

/// A duration in seconds, short: `3 d 4 h`, `5 h 10 min`, `12 min`.
fn span(secs: i64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else {
        format!("{} min", m.max(1))
    }
}

fn from(m: &Metrics, days: i64) -> String {
    match m.source {
        Source::Deploys => format!("{} succeeded deploys in {days} days", m.deploys),
        Source::Releases => format!("{} releases reached live in {days} days", m.deploys),
        Source::None => format!("nothing shipped in {days} days"),
    }
}

fn tiles(m: &Metrics, days: i64) -> Markup {
    let lead = m.lead_time_secs.map_or_else(|| "unknown".to_owned(), span);
    let lead_hint = match (&m.lead_time_note, m.lead_time_samples) {
        (Some(n), _) => n.clone(),
        (None, n) => format!("median of {n} releases, first commit to live"),
    };
    let cfr = m.change_failure_rate.map_or_else(|| "—".to_owned(), pct);
    let restore = m.restore_secs.map_or_else(|| "—".to_owned(), span);
    let restore_hint = if m.unrestored > 0 {
        format!(
            "median of {}; {} not restored yet",
            m.restored, m.unrestored
        )
    } else {
        format!("median of {} failures", m.restored)
    };
    html! {
        section.tiles aria-label={ "Delivery, " (m.environment) } {
            (tile("violet", "release", "Deploy frequency", &format!("{:.1} / week", m.per_week), &from(m, days)))
            (tile("sky", "clock", "Lead time", &lead, &lead_hint))
            (tile("coral", "alert", "Change failure rate", &cfr, &format!("{} of {} changes", m.failures, m.changes)))
            (tile("mint", "pulse", "Time to restore", &restore, &restore_hint))
        }
    }
}

/// The delivery card: the metrics of each environment that shipped, or
/// a line saying nothing did.
pub(crate) fn delivery_card(d: &Delivery) -> Markup {
    let shipped: Vec<&Metrics> = d
        .environments
        .iter()
        .filter(|m| m.source != Source::None)
        .collect();
    html! {
        section.card.wide {
            h2 { "Delivery, last " (d.days) " days" }
            p.muted.small { "From the deploy hook's deploys when it reports them, else from releases reaching live. A change fails when its deploy fails, it is rolled back, or its verdict is regressed; it is restored by the rollback or the next good change. Each ⓘ gives the definition." }
            @if shipped.is_empty() {
                p.small.muted { "Nothing shipped in this period yet." }
            }
            @for m in shipped {
                h3 { (m.environment) }
                (tiles(m, d.days))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(env: &str, source: Source) -> Metrics {
        Metrics {
            environment: env.into(),
            source,
            deploys: 6,
            per_week: 3.0,
            lead_time_secs: None,
            lead_time_samples: 0,
            lead_time_note: Some("unknown: no commit range recorded".into()),
            changes: 7,
            failures: 2,
            change_failure_rate: Some(2.0 / 7.0),
            restore_secs: Some(5_400),
            restored: 1,
            unrestored: 1,
        }
    }

    #[test]
    fn the_card_shows_each_environment_that_shipped_and_says_what_is_unknown() {
        let d = Delivery {
            days: 14,
            timezone: "UTC".into(),
            environments: vec![
                metrics("production", Source::Deploys),
                metrics("preview <x>", Source::None),
            ],
        };
        let html = delivery_card(&d).into_string();
        assert!(html.contains("production"));
        assert!(
            !html.contains("preview"),
            "an environment with nothing shipped is left out"
        );
        assert!(html.contains("unknown: no commit range recorded"));
        assert!(html.contains("1 not restored yet"));
        let none = Delivery {
            environments: Vec::new(),
            ..d
        };
        assert!(
            delivery_card(&none)
                .into_string()
                .contains("Nothing shipped")
        );
    }
}
