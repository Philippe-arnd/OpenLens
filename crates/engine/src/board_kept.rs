//! The period blocks of a board over closed days, kept by the caller and
//! handed back so the board does not read them again (#516, D-064).
//!
//! A date range that ends before today reads its period blocks (sessions,
//! the daily active series, features with their success events, roles,
//! role × feature, the window and its comparison) from days that are
//! over: they change only when something is written to one of those days.
//! The other blocks (DAU, WAU, MAU, known users, Web Vitals, the insight
//! cards, journeys, retention, the releases table…) keep their windows
//! before now and are computed on every board.

use super::*;

/// What a board read from its window's days, and only that.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PeriodBlocks {
    sessions: Sessions,
    dau_series: Vec<(String, i64)>,
    features: Vec<FeatureUse>,
    roles: Vec<RoleUsers>,
    role_gaps: Vec<(String, i64)>,
    role_features: Vec<(String, String, i64)>,
    window: Option<BoardWindow>,
}

impl PeriodBlocks {
    /// The period blocks of a computed board.
    #[must_use]
    pub fn of(b: &Board) -> Self {
        Self {
            sessions: b.sessions.clone(),
            dau_series: b.dau_series.clone(),
            features: b.features.clone(),
            roles: b.roles.clone(),
            role_gaps: b.role_gaps.clone(),
            role_features: b.role_features.clone(),
            window: b.window.clone(),
        }
    }

    /// Put these blocks in `b`, in place of what it computed for them.
    pub(crate) fn put_into(&self, b: &mut Board) {
        b.sessions = self.sessions.clone();
        b.dau_series = self.dau_series.clone();
        b.features = self.features.clone();
        b.roles = self.roles.clone();
        b.role_gaps = self.role_gaps.clone();
        b.role_features = self.role_features.clone();
        b.window = self.window.clone();
    }
}

/// `sql`, or nothing read when its block is `kept` from an earlier board.
pub(crate) async fn unless_kept(
    q: &QueryEngine,
    project: &str,
    sql: &str,
    kept: bool,
) -> std::result::Result<openlens_query::QueryResult, QueryError> {
    if kept {
        return Ok(openlens_query::QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
            truncated: false,
        });
    }
    q.sql(project, sql).await
}
