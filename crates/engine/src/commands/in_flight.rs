//! What is in flight (#617): the goals and how far along they are, the
//! tickets filed from findings and where their measurement stands, and
//! the flags running with their cohort result. The board shows it next
//! to the usage chart, to close the act → compare loop.
//!
//! It reads only what is already stored or computed: the goals and the
//! hourly goals snapshot (the Goals page's numbers and its suggested
//! target), the tickets, the flags, and the board's own flag cohorts. No
//! query runs for it.

use openlens_meta::Meta;
use serde::Serialize;

use super::{flags, goals, tickets};
use crate::{Board, FlagCohort};

/// Rows shown per kind; the rest are counted.
const SHOWN: usize = 3;

/// The days after the release before a ticket's effect is measured
/// (the tickets' hourly pass).
use super::tickets::CHECK_DAYS;

/// The "In flight" card's data.
#[derive(Debug, Clone, Default, Serialize)]
pub struct InFlight {
    /// Goals, as the last hourly pass measured them (at most 3).
    pub goals: Vec<GoalProgress>,
    /// Goals beyond those listed.
    pub more_goals: usize,
    /// With no goal: the Goals page's first suggested target.
    pub suggestion: Option<Suggestion>,
    /// Filed tickets, unmeasured first (at most 3).
    pub tickets: Vec<Measured>,
    /// Filed tickets beyond those listed.
    pub more_tickets: usize,
    /// Flags switched on, those with a cohort first (at most 3).
    pub flags: Vec<Running>,
    /// Running flags beyond those listed.
    pub more_flags: usize,
}

/// A goal and where it stands.
#[derive(Debug, Clone, Serialize)]
pub struct GoalProgress {
    /// KPI label.
    pub label: String,
    /// A share (shown as %).
    pub share: bool,
    /// Value when it was set.
    pub baseline: f64,
    /// Target.
    pub target: f64,
    /// Deadline (YYYY-MM-DD).
    pub deadline: String,
    /// Value at the last hourly pass; none before the first one.
    pub current: Option<f64>,
    /// Status at the last hourly pass (`on track`…); none before it.
    pub status: Option<String>,
}

impl GoalProgress {
    /// How far from the baseline to the target the value has come, 0–1.
    #[must_use]
    pub fn progress(&self) -> Option<f64> {
        let c = self.current?;
        let span = self.target - self.baseline;
        if span.abs() < f64::EPSILON {
            return Some(1.0);
        }
        Some(((c - self.baseline) / span).clamp(0.0, 1.0))
    }
}

/// A suggested target, as the Goals page offers it.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct Suggestion {
    /// KPI key.
    pub kpi: String,
    /// KPI label.
    pub label: String,
    /// A share.
    pub share: bool,
    /// Value now.
    pub current: f64,
    /// Suggested target.
    pub target: f64,
}

/// Where a filed ticket's measurement stands.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "stage")]
pub enum Stage {
    /// The issue is open.
    Open,
    /// Closed on `day`, no release seen since.
    Closed {
        /// Day closed.
        day: String,
    },
    /// Shipped in `release`; measured from `measure_on`.
    Shipped {
        /// Release name.
        release: String,
        /// Day it was measured or will be (release day + 14).
        measure_on: String,
    },
    /// Measured: `improved`, `no effect`, `worse` or `no metric`.
    Measured {
        /// Release name.
        release: String,
        /// The verdict.
        word: String,
        /// Before → after, with the test.
        evidence: String,
    },
}

/// A filed ticket.
#[derive(Debug, Clone, Serialize)]
pub struct Measured {
    /// Title.
    pub title: String,
    /// GitHub issue: (number, URL).
    pub issue: (u64, String),
    /// Where it stands.
    pub stage: Stage,
}

/// A flag switched on, with its sessions against everyone else's.
#[derive(Debug, Clone, Serialize)]
pub struct Running {
    /// Flag key.
    pub key: String,
    /// Rollout percentage.
    pub rollout: u8,
    /// Its cohort, when sessions carried it in the last 30 days.
    pub cohort: Option<FlagCohort>,
}

impl Running {
    /// The smaller p-value of the cohort's two tests (errors, tasks).
    #[must_use]
    pub fn p(&self) -> Option<f64> {
        let c = self.cohort.as_ref()?;
        match (c.p_errors, c.p_tasks) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// What is in flight for `project`, next to the board `b` (whose flag
/// cohorts it reuses).
#[must_use]
pub fn in_flight(meta: &Meta, project: &str, b: &Board) -> InFlight {
    let (measured, suggestion) = goals::measured(meta, project);
    let all_goals: Vec<GoalProgress> = goals::load(meta, project)
        .into_iter()
        .map(|g| {
            let (label, _, share) = crate::goals::kpi_info(&g.kpi);
            let m = measured.iter().find(|m| m.0 == g.id);
            GoalProgress {
                label,
                share,
                baseline: g.baseline,
                target: g.target,
                deadline: g.deadline,
                current: m.and_then(|m| m.1),
                status: m.map(|m| m.2.clone()),
            }
        })
        .collect();
    let mut filed: Vec<(bool, String, Measured)> = tickets::load(meta, project)
        .into_iter()
        .filter_map(|t| {
            let issue = t.issue.clone()?;
            let done = t.outcome.is_some();
            Some((
                done,
                t.created.clone(),
                Measured {
                    title: t.title.clone(),
                    issue,
                    stage: stage(&t),
                },
            ))
        })
        .collect();
    // Unmeasured first, then the newest.
    filed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
    let mut running: Vec<Running> = flags::load(meta, project)
        .into_iter()
        .filter(|f| f.enabled)
        .map(|f| Running {
            cohort: b.flag_cohorts.iter().find(|c| c.flag == f.key).cloned(),
            key: f.key,
            rollout: f.rollout,
        })
        .collect();
    running.sort_by(|a, b| {
        b.cohort
            .is_some()
            .cmp(&a.cohort.is_some())
            .then_with(|| a.key.cmp(&b.key))
    });
    InFlight {
        more_goals: all_goals.len().saturating_sub(SHOWN),
        suggestion: if all_goals.is_empty() {
            suggestion
        } else {
            None
        },
        goals: all_goals.into_iter().take(SHOWN).collect(),
        more_tickets: filed.len().saturating_sub(SHOWN),
        tickets: filed.into_iter().take(SHOWN).map(|t| t.2).collect(),
        more_flags: running.len().saturating_sub(SHOWN),
        flags: running.into_iter().take(SHOWN).collect(),
    }
}

/// Where a ticket's measurement stands, from what the hourly pass stored.
fn stage(t: &tickets::Ticket) -> Stage {
    let shipped = t.shipped_in.as_deref().map(|s| {
        let (rel, day) = s.split_once('|').unwrap_or((s, ""));
        (rel.to_owned(), day.to_owned())
    });
    match (&t.outcome, shipped, &t.closed) {
        (Some((word, evidence)), s, _) => Stage::Measured {
            release: s.map(|s| s.0).unwrap_or_default(),
            word: word.clone(),
            evidence: evidence.clone(),
        },
        (None, Some((release, day)), _) => Stage::Shipped {
            release,
            measure_on: chrono::NaiveDate::parse_from_str(&day, "%Y-%m-%d").map_or_else(
                |_| String::new(),
                |d| (d + chrono::Duration::days(CHECK_DAYS)).to_string(),
            ),
        },
        (None, None, Some(day)) => Stage::Closed { day: day.clone() },
        (None, None, None) => Stage::Open,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket() -> tickets::Ticket {
        tickets::Ticket {
            fp: "f".into(),
            kind: "insight".into(),
            subject: "s".into(),
            title: "t".into(),
            metric: Some("error_rate".into()),
            baseline: Some(0.1),
            baseline_n: 100,
            target: Some(0.05),
            up: false,
            created: "2026-11-01".into(),
            issue: Some((7, "https://github.com/o/r/issues/7".into())),
            closed: None,
            shipped_in: None,
            outcome: None,
        }
    }

    #[test]
    fn a_ticket_moves_from_open_to_measured_14_days_after_its_release() {
        let mut t = ticket();
        assert_eq!(stage(&t), Stage::Open);
        t.closed = Some("2026-11-03".into());
        assert!(matches!(stage(&t), Stage::Closed { ref day } if day == "2026-11-03"));
        t.shipped_in = Some("v2|2026-11-04".into());
        let Stage::Shipped {
            release,
            measure_on,
        } = stage(&t)
        else {
            panic!("{:?}", stage(&t));
        };
        assert_eq!(release, "v2");
        let shipped = chrono::NaiveDate::from_ymd_opt(2026, 11, 4).unwrap_or_default();
        let on = chrono::NaiveDate::parse_from_str(&measure_on, "%Y-%m-%d").unwrap_or_default();
        assert_eq!((on - shipped).num_days(), CHECK_DAYS);
        t.outcome = Some(("improved".into(), "10.0% before → 5.0% after".into()));
        assert!(
            matches!(stage(&t), Stage::Measured { ref release, ref word, .. } if release == "v2" && word == "improved")
        );
    }

    #[test]
    fn it_joins_the_stored_goals_tickets_and_flags_with_the_board() {
        let meta = Meta::in_memory().unwrap();
        let p = meta.create_project("Shop", None).unwrap().id;
        let set = |k: &str, v: serde_json::Value| {
            meta.set_project_setting(&p, k, &v.to_string()).unwrap();
        };
        // No goal: the snapshot's suggestion is offered.
        let tip = Suggestion {
            kpi: "error_rate".into(),
            label: "Sessions with errors".into(),
            share: true,
            current: 0.04,
            target: 0.03,
        };
        set(
            "goals_snapshot",
            serde_json::json!({ "rows": [["g2", 0.08, "on track"]], "suggestion": tip }),
        );
        let empty = in_flight(&meta, &p, &Board::default());
        assert_eq!(empty.suggestion.as_ref(), Some(&tip));
        assert!(empty.goals.is_empty() && empty.tickets.is_empty() && empty.flags.is_empty());
        // Two goals; the snapshot measured only the second.
        let goal = |id: &str| {
            serde_json::json!({ "id": id, "kpi": "error_rate", "target": 0.05, "up": false,
                "deadline": "2027-01-31", "baseline": 0.1, "created": "2026-11-01" })
        };
        set("goals", serde_json::json!([goal("g1"), goal("g2")]));
        // Tickets: one never filed, one measured, one waiting.
        let mut measured = ticket();
        measured.title = "measured".into();
        measured.created = "2026-11-20".into();
        measured.outcome = Some(("worse".into(), "e".into()));
        let mut waiting = ticket();
        waiting.title = "waiting".into();
        let mut draft = ticket();
        draft.title = "draft".into();
        draft.issue = None;
        set("tickets", serde_json::json!([draft, measured, waiting]));
        for (key, on) in [("off", false), ("quiet", true), ("seen", true)] {
            let f: openlens_flags::Flag = serde_json::from_value(
                serde_json::json!({ "key": key, "enabled": on, "rollout": 50 }),
            )
            .unwrap();
            flags::save(&meta, &p, &f).unwrap();
        }
        let cohort = |flag: &str| FlagCohort {
            flag: flag.into(),
            with: crate::CohortStats::default(),
            without: crate::CohortStats::default(),
            p_errors: Some(0.01),
            p_tasks: Some(0.2),
        };
        let b = Board {
            flag_cohorts: vec![cohort("seen"), cohort("off")],
            ..Board::default()
        };
        let d = in_flight(&meta, &p, &b);
        // A goal is set: no suggestion; each goal has its own numbers.
        assert!(d.suggestion.is_none());
        assert_eq!(d.goals.len(), 2);
        assert!(d.goals[0].current.is_none() && d.goals[0].status.is_none());
        assert_eq!(d.goals[1].status.as_deref(), Some("on track"));
        assert!(d.goals[1].progress() > Some(0.0));
        // Filed tickets only, the unmeasured one first.
        let titles: Vec<&str> = d.tickets.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["waiting", "measured"]);
        // Flags switched on only, the one with sessions first, with its cohort.
        let keys: Vec<&str> = d.flags.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["seen", "quiet"]);
        assert!(d.flags[0].cohort.is_some() && d.flags[1].cohort.is_none());
        assert!(d.flags[0].p() < Some(crate::stats::ALPHA));
    }

    #[test]
    fn progress_runs_from_the_baseline_to_the_target_either_way() {
        let g = |baseline: f64, target: f64, current: f64| GoalProgress {
            label: String::new(),
            share: true,
            baseline,
            target,
            deadline: String::new(),
            current: Some(current),
            status: None,
        };
        // Lower is better: halfway down is halfway along.
        let down = g(0.10, 0.05, 0.075).progress().unwrap_or_default();
        let up = g(0.10, 0.20, 0.15).progress().unwrap_or_default();
        assert!((down - 0.5).abs() < 1e-9 && (up - 0.5).abs() < 1e-9);
        // Going the wrong way is no progress; past the target is all of it.
        assert!(g(0.10, 0.05, 0.12).progress() < g(0.10, 0.05, 0.09).progress());
        assert!(g(0.10, 0.05, 0.01).progress() >= g(0.10, 0.05, 0.05).progress());
        // Not measured yet: no bar.
        let mut none = g(0.10, 0.05, 0.0);
        none.current = None;
        assert!(none.progress().is_none());
    }
}
