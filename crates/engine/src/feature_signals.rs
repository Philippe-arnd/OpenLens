//! The board's Features table (#618): each feature with the signals the
//! board's insights carry about it, as short tags, and the declared
//! features nobody used. The UI prints them; it does not read the cards'
//! text (rule 10).

use crate::{Board, FeatureUse, Insight};
use serde::Serialize;

/// A short tag on a feature's row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Signal {
    /// The tag's text: "Stick driver 5.2×", "Churn exit 72%", "Admins 0%".
    pub label: String,
    /// Its tone: `good`, `info` or `warn`.
    pub tone: &'static str,
    /// How much it asks for attention (higher first): the table's order.
    pub severity: u8,
}

/// A feature and its signals, most severe first.
#[derive(Debug, Clone, Serialize)]
pub struct FeatureRow<'a> {
    /// The feature's numbers.
    pub feature: &'a FeatureUse,
    /// Its signals; empty when nothing stands out.
    pub signals: Vec<Signal>,
}

fn pct(r: f64) -> String {
    format!("{:.0}%", r * 100.0)
}

/// "admin" → "Admins": the role as the tag names its users.
fn plural(role: &str) -> String {
    let mut c = role.chars();
    c.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(c).chain(['s']).collect()
    })
}

/// The tag an insight card puts on `feature`'s row, if the card is about it.
fn from_insight(card: &Insight, feature: &str) -> Option<Signal> {
    let tag = |label: String, tone, severity| {
        Some(Signal {
            label,
            tone,
            severity,
        })
    };
    match card.kind {
        "stick_driver" if card.subject == feature => tag(
            card.figure.map_or_else(
                || "Stick driver".to_owned(),
                |x| format!("Stick driver {x:.1}×"),
            ),
            "good",
            1,
        ),
        "churn_point" if card.subject == feature => tag(
            format!("Churn exit {}", pct(card.figure.unwrap_or(0.0))),
            "warn",
            3,
        ),
        "friction" if card.subject == feature => tag(
            format!("Finished by {}", pct(card.figure.unwrap_or(0.0))),
            "warn",
            3,
        ),
        "role_blind_spot" => {
            let (role, feat) = card.subject.split_once(':')?;
            if feat != feature {
                return None;
            }
            tag(
                format!("{} {}", plural(role), pct(card.figure.unwrap_or(0.0))),
                "info",
                2,
            )
        }
        _ => None,
    }
}

impl Board {
    /// The signals on `feature`'s row: the insight cards about it, and
    /// "not used" for a declared feature nobody used in the period.
    #[must_use]
    pub fn feature_signals(&self, feature: &FeatureUse) -> Vec<Signal> {
        let mut out: Vec<Signal> = self
            .insights
            .iter()
            .filter_map(|c| from_insight(c, &feature.key))
            .collect();
        if feature.declared && feature.users == 0 {
            out.push(Signal {
                label: if self.window.is_some() {
                    "Not used in the window"
                } else {
                    "Not used in 30 days"
                }
                .to_owned(),
                tone: "warn",
                severity: 2,
            });
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.severity));
        out.dedup();
        out
    }

    /// Every feature of the board with its signals: those with signals
    /// first, the most severe first, then by adoption (#618).
    #[must_use]
    pub fn feature_rows(&self) -> Vec<FeatureRow<'_>> {
        let mut rows: Vec<FeatureRow<'_>> = self
            .features
            .iter()
            .map(|f| FeatureRow {
                feature: f,
                signals: self.feature_signals(f),
            })
            .collect();
        let top = |r: &FeatureRow<'_>| r.signals.first().map_or(0, |s| s.severity + 1);
        rows.sort_by(|a, b| {
            top(b)
                .cmp(&top(a))
                .then(b.feature.users.cmp(&a.feature.users))
        });
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(kind: &'static str, subject: &str, figure: Option<f64>) -> Insight {
        Insight {
            kind,
            title: String::new(),
            evidence: String::new(),
            next_step: String::new(),
            score: 1.0,
            subject: subject.into(),
            figure,
        }
    }

    fn feature(key: &str, users: i64, declared: bool) -> FeatureUse {
        FeatureUse {
            key: key.into(),
            name: None,
            uses: users,
            users,
            completed: 0,
            completed_users: 0,
            user_days: users,
            declared,
        }
    }

    #[test]
    fn cards_tag_the_feature_they_are_about_and_flagged_rows_come_first() {
        let b = Board {
            features: vec![
                feature("home", 140, true),
                feature("reports", 110, true),
                feature("export", 40, true),
                feature("billing", 0, true),
            ],
            insights: vec![
                card("stick_driver", "export", Some(5.2)),
                card("role_blind_spot", "admin:reports", Some(0.0)),
                card("churn_point", "billing", Some(0.72)),
                card("release_regression", "v2", None),
            ],
            ..Board::default()
        };
        let rows = b.feature_rows();
        let of = |k: &str| {
            rows.iter()
                .find(|r| r.feature.key == k)
                .map(|r| r.signals.clone())
                .unwrap_or_default()
        };
        assert!(of("export")[0].label.contains("5.2"), "{:?}", of("export"));
        assert!(of("reports")[0].label.starts_with("Admins"));
        // The unused feature is listed, tagged, with its card's tag.
        let billing = of("billing");
        assert_eq!(billing.len(), 2, "{billing:?}");
        assert!(billing.iter().any(|s| s.label.starts_with("Not used")));
        assert!(
            of("home").is_empty(),
            "a card about a release tags no feature"
        );
        // Every row with a signal before every row without one, the most
        // severe first, although home has the most users.
        let keys: Vec<&str> = rows.iter().map(|r| r.feature.key.as_str()).collect();
        assert_eq!(keys.last(), Some(&"home"), "{keys:?}");
        let pos = |k: &str| keys.iter().position(|x| *x == k);
        assert!(pos("billing") < pos("reports") && pos("reports") < pos("export"));
    }
}
