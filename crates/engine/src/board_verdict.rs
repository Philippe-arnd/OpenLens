//! The verdict of the newest release on the board (#614): the status
//! strip's *Latest release* cell reads it. It is judged as the Releases
//! page judges it ([`crate::verdict`]), by the server next to the board
//! and cached with it.
//!
//! When it is *regressed*, it leads *What to focus on* (#615): [`lead`]
//! says it in plain words from its worst check, with the checks grouped
//! by layer, and its release regression card is not repeated in the
//! ranked list ([`crate::Board::ranked_insights`]).

use serde::Serialize;

use crate::verdict::{Check, Outcome, Report, Verdict};
use crate::{Board, Insight};

/// The newest release of the board's environment, judged.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LatestRelease {
    /// No release with traffic in the environment over the releases'
    /// window.
    NoRelease,
    /// The newest release compared with the one before it.
    Judged(Box<Report>),
    /// It could not be judged now: why.
    Unavailable {
        /// What failed.
        reason: String,
    },
}

impl LatestRelease {
    /// The report, when the release was judged.
    #[must_use]
    pub fn report(&self) -> Option<&Report> {
        match self {
            Self::Judged(r) => Some(r),
            _ => None,
        }
    }
}

/// A layer of the lead's checks (#615): Browser, Features (Backend comes
/// with #623).
#[derive(Debug, Clone, Serialize)]
pub struct Layer {
    /// As shown.
    pub name: &'static str,
    /// Its checks, the worse first.
    pub rows: Vec<Row>,
    /// Checks of the layer left out (see the full verdict).
    pub more: usize,
}

/// One check of a layer, ready to show.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    /// What it measures.
    pub label: String,
    /// On the release before, as shown; none for a new error.
    pub from: Option<String>,
    /// On this release, as shown.
    pub to: String,
    /// How it moved.
    pub outcome: Outcome,
}

/// The lead card of a regressed release (#615): the verdict in plain
/// words, built from its worst check.
#[derive(Debug, Clone, Serialize)]
pub struct Lead {
    /// The release.
    pub release: String,
    /// The release it is compared with.
    pub previous: Option<String>,
    /// Sessions on each (this release, the one before).
    pub sessions: (i64, Option<i64>),
    /// One sentence: what got worse, in plain words.
    pub headline: String,
    /// What it rests on, and whether anything else got worse.
    pub evidence: String,
    /// What to do first.
    pub next_step: String,
    /// The checks by layer, the layers with no check left out.
    pub layers: Vec<Layer>,
    /// The KPI a ticket follows (`error_rate`, `success:<feature>`).
    pub metric: Option<String>,
}

/// Rows a layer shows at most besides its worse checks.
const OTHER_ROWS: usize = 3;

fn pct(r: f64) -> String {
    format!("{:.1}%", r * 100.0)
}

/// A number from two to ten in words, the others in digits.
fn words(n: i64) -> String {
    const W: [&str; 9] = [
        "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    ];
    usize::try_from(n - 2)
        .ok()
        .and_then(|i| W.get(i))
        .map_or_else(|| n.to_string(), |w| (*w).to_owned())
}

/// A share of sessions in plain words, with its verb: "one session in
/// three hits", "more than half of sessions hit", "4.1% of sessions hit".
fn sessions_share(r: f64) -> String {
    if r >= 0.5 {
        return "more than half of sessions hit".to_owned();
    }
    let n = if r > 0.0 { (1.0 / r).round() as i64 } else { 0 };
    if (2..=10).contains(&n) {
        format!("one session in {} hits", words(n))
    } else {
        format!("{} of sessions hit", pct(r))
    }
}

/// A server error group (the backend layer, #623), not a browser one.
fn server_error(r: &Report, c: &Check) -> bool {
    c.kind == "errors"
        && r.current
            .new_errors
            .iter()
            .any(|e| e.fingerprint == c.key && e.server)
}

/// The layer a check shows in; none for the layers the lead leaves to
/// the full verdict (backend, goals and guardrails).
fn layer(r: &Report, c: &Check) -> Option<&'static str> {
    match c.kind {
        "kpi" if c.outcome != Outcome::Info => Some("Browser"),
        "vital" => Some("Browser"),
        "errors" if !server_error(r, c) => Some("Browser"),
        "feature" => Some("Features"),
        _ => None,
    }
}

/// How bad a worse check is, for the headline: errors first, then a
/// feature that stopped finishing, the vitals, the other KPIs, the rest.
fn weight(r: &Report, c: &Check) -> (u8, f64) {
    let moved = c.previous.map_or(c.current, |p| (c.current - p).abs());
    let rank = match (c.kind, c.key.as_str()) {
        ("kpi", "error_rate") => 0,
        ("errors", _) if !server_error(r, c) => 1,
        ("feature", _) => 2,
        ("vital", _) => 3,
        ("kpi", _) => 4,
        _ => 5,
    };
    (rank, moved)
}

/// A check's value as shown.
fn shown(c: &Check, v: f64) -> String {
    match c.kind {
        "vital" => crate::vitals::show(&c.key, v),
        "errors" => format!("{v:.0} users"),
        "kpi" | "feature" => pct(v),
        _ => format!("{v:.2}"),
    }
}

/// The worst check's headline, next step and KPI.
fn wording(r: &Report, c: &Check) -> (String, String, Option<String>) {
    let rel = &r.release;
    let since = format!("Since {rel}");
    let from_to = || {
        c.previous.map_or_else(
            || shown(c, c.current),
            |p| format!("{} → {}", shown(c, p), shown(c, c.current)),
        )
    };
    let errors_step = format!(
        "Open the errors on {rel}, the ones hitting the most users first, and watch a session that hit one."
    );
    match (c.kind, c.key.as_str()) {
        ("kpi", "error_rate") => (
            format!("{since}, {} an error", sessions_share(c.current)),
            errors_step,
            Some("error_rate".to_owned()),
        ),
        ("errors", _) if !server_error(r, c) => {
            let sample = r
                .current
                .new_errors
                .iter()
                .find(|e| e.fingerprint == c.key)
                .map(|e| e.sample.chars().take(80).collect::<String>())
                .unwrap_or_default();
            (
                format!(
                    "{since}, a new error reaches {:.0} users: {sample}",
                    c.current
                ),
                errors_step,
                Some("error_rate".to_owned()),
            )
        }
        ("feature", f) => (
            format!("{since}, fewer sessions finish {f}: {}", from_to()),
            format!(
                "Walk through {f} on {rel} as a user would: find the step that no longer finishes."
            ),
            Some(format!("success:{f}")),
        ),
        ("vital", v) => {
            let what = match v {
                "LCP" => "pages load slower",
                "INP" => "pages answer clicks slower",
                _ => "pages shift more while they load",
            };
            (
                format!("{since}, {what}: {} {}", c.label, from_to()),
                format!(
                    "Open the slowest pages on {rel} and see what the release changed on them."
                ),
                None,
            )
        }
        ("kpi", "bounce_rate") => (
            format!("{since}, more sessions end at once: {}", from_to()),
            format!("Look at the entry pages on {rel}: find what makes people leave at once."),
            None,
        ),
        ("kpi", "task_rate") => (
            format!("{since}, fewer sessions complete a task: {}", from_to()),
            format!(
                "Check the features used most on {rel}: find the one people stopped finishing."
            ),
            None,
        ),
        _ => (
            format!("{since}, {}", c.note),
            format!("Open the full verdict of {rel} and check what moved."),
            None,
        ),
    }
}

/// The lead of a regressed release; none for any other verdict.
#[must_use]
pub fn lead(r: &Report) -> Option<Lead> {
    if r.verdict != Some(Verdict::Regressed) {
        return None;
    }
    let worse: Vec<&Check> = r
        .checks
        .iter()
        .filter(|c| c.judged && c.outcome == Outcome::Worse)
        .collect();
    let worst = worse.iter().copied().min_by(|a, b| {
        let (wa, wb) = (weight(r, a), weight(r, b));
        wa.0.cmp(&wb.0).then(wb.1.total_cmp(&wa.1))
    })?;
    let (headline, next_step, metric) = wording(r, worst);
    let sessions = (r.current.sessions, r.previous.as_ref().map(|p| p.sessions));
    let mut evidence = match (&r.previous_release, sessions.1) {
        (Some(p), Some(n)) => format!(
            "{} sessions on {}, {n} on {p}.",
            r.current.sessions, r.release
        ),
        _ => format!("{} sessions on {}.", r.current.sessions, r.release),
    };
    evidence.push_str(&match worse.len().saturating_sub(1) {
        0 => " Nothing else got worse.".to_owned(),
        1 => " One more check got worse.".to_owned(),
        n => format!(" {n} more checks got worse."),
    });
    let layers = ["Browser", "Features"]
        .into_iter()
        .filter_map(|name| {
            let mut checks: Vec<&Check> = r
                .checks
                .iter()
                .filter(|c| layer(r, c) == Some(name))
                .collect();
            // The worse first, then the others in the verdict's order.
            checks.sort_by_key(|c| c.outcome != Outcome::Worse);
            let bad = checks
                .iter()
                .filter(|c| c.outcome == Outcome::Worse)
                .count();
            let shown_n = (bad + OTHER_ROWS).min(checks.len());
            let rows: Vec<Row> = checks
                .iter()
                .take(shown_n)
                .map(|c| Row {
                    label: c.label.clone(),
                    from: c.previous.map(|p| shown(c, p)),
                    to: shown(c, c.current),
                    outcome: c.outcome,
                })
                .collect();
            (!rows.is_empty()).then(|| Layer {
                name,
                rows,
                more: checks.len() - shown_n,
            })
        })
        .collect();
    Some(Lead {
        release: r.release.clone(),
        previous: r.previous_release.clone(),
        sessions,
        headline,
        evidence,
        next_step,
        layers,
        metric,
    })
}

impl Board {
    /// The lead of a regressed latest release (#615); none otherwise, and
    /// then the top insight leads.
    #[must_use]
    pub fn release_lead(&self) -> Option<Lead> {
        self.latest_release
            .as_ref()
            .and_then(LatestRelease::report)
            .and_then(lead)
    }

    /// The insights of the ranked list, with their index in
    /// [`Board::insights`]: every one, but the release regression card of
    /// a release that leads as its verdict (#615).
    #[must_use]
    pub fn ranked_insights(&self) -> Vec<(usize, &Insight)> {
        let leading = self.release_lead().map(|l| l.release);
        self.insights
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                !(c.kind == "release_regression" && leading.as_deref() == Some(c.subject.as_str()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::{NewError, Side, compare};

    fn side(release: &str, users: i64, sessions: i64, bad: i64) -> Side {
        Side {
            release: release.into(),
            users,
            sessions,
            bounced: sessions / 4,
            error_sessions: bad,
            task_sessions: sessions / 2,
            tasks: sessions,
            avg_secs: 60.0,
            ..Side::default()
        }
    }

    fn card(kind: &'static str, subject: &str) -> Insight {
        Insight {
            kind,
            title: kind.into(),
            evidence: String::new(),
            next_step: String::new(),
            score: 1.0,
            subject: subject.into(),
            figure: None,
        }
    }

    /// Release N+1 (v2) judged against N (v1): `new` are its numbers.
    fn board(new: &Side) -> Board {
        let r = compare("production", new, Some(&side("v1", 150, 500, 10)), 30, &[]);
        Board {
            latest_release: Some(LatestRelease::Judged(Box::new(r))),
            insights: vec![
                card("release_regression", "v2"),
                card("churn_point", "billing"),
            ],
            ..Board::default()
        }
    }

    #[test]
    fn a_regression_planted_in_the_newest_release_leads_once_in_plain_words() {
        // One session in five hits an error on v2, one in fifty on v1.
        let b = board(&side("v2", 120, 400, 80));
        let l = b.release_lead().unwrap();
        assert_eq!(l.release, "v2");
        assert!(
            l.headline
                .starts_with("Since v2, one session in five hits an error"),
            "{}",
            l.headline
        );
        assert!(l.next_step.contains("errors on v2"), "{}", l.next_step);
        assert_eq!(l.metric.as_deref(), Some("error_rate"));
        // The Browser layer, the worse check first.
        let browser = l.layers.iter().find(|x| x.name == "Browser").unwrap();
        assert_eq!(browser.rows[0].outcome, Outcome::Worse);
        assert_eq!(browser.rows[0].label, "Sessions with errors");
        // Its regression card is the lead: not ranked again; the others are.
        let ranked: Vec<&str> = b.ranked_insights().iter().map(|(_, c)| c.kind).collect();
        assert_eq!(ranked, ["churn_point"]);
        assert_eq!(b.ranked_insights()[0].0, 1, "its index on the board");
    }

    #[test]
    fn a_healthy_release_leaves_the_lead_to_the_top_insight() {
        let b = board(&side("v2", 120, 400, 8));
        assert!(b.release_lead().is_none());
        assert_eq!(b.ranked_insights().len(), b.insights.len());
        // Nothing judged: nothing leads either.
        let none = Board {
            latest_release: Some(LatestRelease::NoRelease),
            ..b
        };
        assert!(none.release_lead().is_none());
    }

    #[test]
    fn a_new_error_alone_makes_the_headline_and_a_server_one_stays_out_of_the_browser() {
        let mut v2 = side("v2", 120, 400, 8);
        v2.new_errors = vec![
            NewError {
                fingerprint: "browser".into(),
                sample: "TypeError: x is undefined".into(),
                users: 4,
                count: 9,
                server: false,
            },
            NewError {
                fingerprint: "server".into(),
                sample: "Error: pool exhausted".into(),
                users: 3,
                count: 5,
                server: true,
            },
        ];
        let l = board(&v2).release_lead().unwrap();
        assert!(
            l.headline
                .contains("a new error reaches 4 users: TypeError"),
            "{}",
            l.headline
        );
        let browser = l.layers.iter().find(|x| x.name == "Browser").unwrap();
        assert_eq!(
            browser
                .rows
                .iter()
                .filter(|r| r.label == "New error")
                .count(),
            1,
            "{browser:?}"
        );
        assert!(
            l.evidence.contains("One more check got worse"),
            "{}",
            l.evidence
        );
    }
}
