//! The portfolio (#717, D-118): one row per project the caller may see,
//! for a PM who runs several apps on one OpenLens. Each row reads what is
//! already there: the project's default board as the host keeps it (never
//! computed for this command: a project whose board is not kept yet says
//! *computing*, with the last compute time, like D-073), the last hourly
//! goals snapshot and the open incidents. Rows are ordered by what needs
//! attention (a regressed release, goals off track, open incidents), then
//! by activity.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Access, Caller, Command, Error, Host};
use crate::Board;
use crate::board_verdict::LatestRelease;
use crate::verdict::Verdict;

/// The default board of a project as the host keeps it (#717): kept or
/// not, and how long it took to compute last time.
#[derive(Debug, Clone, Default)]
pub struct CachedBoard {
    /// The board and how old it is, when the host keeps one.
    pub board: Option<(Arc<Board>, Duration)>,
    /// How long it took to compute last time, when it was computed before.
    pub took: Option<Duration>,
}

/// The input of `portfolio.get`: none, the rows are the caller's projects.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetInput {}

/// The board's Outcomes in compact form (#616): the same numbers as each
/// board's tiles, none when there is nothing to count yet.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Outcomes {
    /// Reach: the window's users, else the last 30 days' (MAU).
    pub reach: i64,
    /// Value: completed tasks per session.
    pub value: Option<f64>,
    /// Retention: the churn rate (share of users who left).
    pub retention: Option<f64>,
    /// Quality: the share of sessions with an error.
    pub quality: Option<f64>,
    /// Speed: LCP at p75, in milliseconds.
    pub speed_ms: Option<f64>,
}

impl Outcomes {
    /// The tiles' numbers from a board, as `ui::board::tiles` reads them.
    #[must_use]
    pub fn of(b: &Board) -> Self {
        let s = &b.sessions;
        let counted = s.count > 0;
        Self {
            reach: b.window.as_ref().map_or(b.users.mau, |w| w.current.users),
            value: counted.then_some(s.tasks_per_session),
            retention: b.churn.as_ref().and_then(crate::churn::ChurnLine::rate),
            quality: counted.then_some(s.error_rate),
            speed_ms: b
                .vitals
                .iter()
                .find(|v| v.name == "LCP" && v.samples > 0)
                .map(|v| v.p75),
        }
    }
}

/// The newest production release and its verdict.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReleaseLine {
    /// Release name; none when there is no release with traffic lately.
    pub release: Option<String>,
    /// `healthy`, `regressed`, `inconclusive`, `measuring` (no verdict
    /// yet), `none` (no release) or `not judged` (it could not be judged).
    pub verdict: String,
    /// The first reason, when there is one.
    pub reason: Option<String>,
}

impl ReleaseLine {
    fn of(l: &LatestRelease) -> Self {
        match l {
            LatestRelease::NoRelease => Self {
                release: None,
                verdict: "none".into(),
                reason: None,
            },
            LatestRelease::Unavailable { .. } => Self {
                release: None,
                verdict: "not judged".into(),
                reason: None,
            },
            LatestRelease::Judged(r) => Self {
                release: Some(r.release.clone()),
                verdict: r.verdict.map_or("measuring", Verdict::label).into(),
                reason: r.reasons.first().cloned(),
            },
        }
    }

    /// Whether the release regressed.
    #[must_use]
    pub fn regressed(&self) -> bool {
        self.verdict == Verdict::Regressed.label()
    }
}

/// One project of the portfolio.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    /// Project id.
    pub project: String,
    /// Display name.
    pub name: String,
    /// Its board: `/p/{id}`.
    pub board_url: String,
    /// The board is not kept yet: it is being computed, nothing was
    /// computed for this row.
    pub computing: bool,
    /// How old the board read is, in seconds.
    pub board_age_secs: Option<u64>,
    /// How long the board took to compute last time, in seconds (rounded
    /// up), when it was computed before.
    pub last_compute_secs: Option<u64>,
    /// The Outcomes, when the board is kept.
    pub outcomes: Option<Outcomes>,
    /// The newest production release, when the board is kept.
    pub latest_release: Option<ReleaseLine>,
    /// Goals at risk (last hourly snapshot).
    pub goals_at_risk: usize,
    /// Goals off track.
    pub goals_off_track: usize,
    /// The worst goal (off track first, else at risk): its line and status.
    pub worst_goal: Option<(String, String)>,
    /// Open incidents.
    pub open_incidents: usize,
    /// The title of the board's top insight card, when the board is kept.
    pub top_insight: Option<String>,
}

impl Row {
    /// The order key: what needs attention first, then activity.
    fn rank(&self) -> (bool, usize, usize, i64) {
        (
            self.latest_release
                .as_ref()
                .is_some_and(ReleaseLine::regressed),
            self.goals_off_track,
            self.open_incidents,
            self.outcomes.as_ref().map_or(0, |o| o.reach),
        )
    }
}

/// The portfolio: the caller's projects, the ones needing attention first.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Portfolio {
    /// One row per project.
    pub projects: Vec<Row>,
}

/// Order rows: a regressed release first, then goals off track, then open
/// incidents, then activity (reach), then by name.
pub fn order(rows: &mut [Row]) {
    rows.sort_by(|a, b| b.rank().cmp(&a.rank()).then_with(|| a.name.cmp(&b.name)));
}

fn secs(d: Duration) -> u64 {
    d.as_millis().div_ceil(1000).try_into().unwrap_or(u64::MAX)
}

/// One project's row from what the host already has.
pub fn row(meta: &openlens_meta::Meta, p: &openlens_meta::Project, cached: &CachedBoard) -> Row {
    let (_, goals) = super::goals::snapshot(meta, &p.id);
    let off: Vec<&(String, String)> = goals.iter().filter(|g| g.1 == "off track").collect();
    let risk: Vec<&(String, String)> = goals.iter().filter(|g| g.1 == "at risk").collect();
    let worst = off.first().or_else(|| risk.first()).map(|g| (*g).clone());
    let incidents = super::alerts::status(meta, &p.id).open.len();
    let board = cached.board.as_ref();
    Row {
        project: p.id.clone(),
        name: p.name.clone(),
        board_url: format!("/p/{}", p.id),
        computing: board.is_none(),
        board_age_secs: board.map(|(_, age)| age.as_secs()),
        last_compute_secs: cached.took.map(secs),
        outcomes: board.map(|(b, _)| Outcomes::of(b)),
        latest_release: board
            .and_then(|(b, _)| b.latest_release.as_ref())
            .map(ReleaseLine::of),
        goals_at_risk: risk.len(),
        goals_off_track: off.len(),
        worst_goal: worst,
        open_incidents: incidents,
        top_insight: board.and_then(|(b, _)| b.insights.first().map(|i| i.title.clone())),
    }
}

/// `portfolio.get`: the caller's projects, ordered.
pub struct Get;

impl Command for Get {
    const NAME: &'static str = "portfolio.get";
    // Anyone signed in: the rows are only the projects they may see.
    const ACCESS: Access = Access::Signed;
    type Input = GetInput;
    type Output = Portfolio;

    async fn run<H: Host>(host: &H, caller: &Caller, _: GetInput) -> Result<Portfolio, Error> {
        let projects = host
            .meta()
            .projects()
            .map_err(|e| Error::Storage(e.to_string()))?;
        let mut rows = Vec::new();
        for p in projects.iter().filter(|p| caller.may_see(&p.id)) {
            let cached = host.cached_board(p, caller.tz.as_deref()).await;
            rows.push(row(host.meta(), p, &cached));
        }
        order(&mut rows);
        Ok(Portfolio { projects: rows })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(name: &str, verdict: &str, off: usize, incidents: usize, reach: i64) -> Row {
        Row {
            project: name.into(),
            name: name.into(),
            board_url: format!("/p/{name}"),
            computing: false,
            board_age_secs: Some(1),
            last_compute_secs: Some(1),
            outcomes: Some(Outcomes {
                reach,
                ..Outcomes::default()
            }),
            latest_release: Some(ReleaseLine {
                release: Some("1.0".into()),
                verdict: verdict.into(),
                reason: None,
            }),
            goals_at_risk: 0,
            goals_off_track: off,
            worst_goal: None,
            open_incidents: incidents,
            top_insight: None,
        }
    }

    #[test]
    fn a_regressed_project_comes_first_then_goals_then_incidents_then_activity() {
        let mut rows = vec![
            r("busy", "healthy", 0, 0, 900),
            r("incident", "healthy", 0, 2, 10),
            r("goals", "healthy", 1, 0, 10),
            r("regressed", "regressed", 0, 0, 1),
            r("quiet", "healthy", 0, 0, 5),
        ];
        order(&mut rows);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["regressed", "goals", "incident", "busy", "quiet"]);
    }
}
