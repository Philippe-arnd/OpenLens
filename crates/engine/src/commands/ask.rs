//! What Ask OpenLens looks up, as commands (#508, part of #308): the
//! board behind the `kpi` and `insights` tools and the brief, one feature
//! in depth, the top errors, read-only SQL, Web Vitals and the goals'
//! status. The tool loop, the model call and the text the model reads
//! stay in the server (`crates/server/src/ask.rs`); these commands return
//! the data, and the server writes it as compact TSV.
//!
//! The SQL tool's guard lives here: a readable first check
//! ([`sql_allowed`]), then, unless an admin allowed person-level detail
//! for the project, the query runs on an aggregate-only view keyed by a
//! per-project secret, which is what actually keeps people out (D-020).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Access, Caller, Command, Error, Host};
use crate::{Board as BoardData, ErrorGroup, FeatureContext, Filters, VitalReport};

/// Longest role name the board is filtered by; a longer one is ignored.
const MAX_ROLE: usize = 64;
/// Longest SQL query run.
const MAX_SQL: usize = 4000;

/// The filters every Ask lookup uses: the last 30 days in production, in
/// the caller's time zone or else the project's.
fn filters(caller: &Caller, tz: &str) -> Filters {
    Filters {
        tz: caller.tz.clone().unwrap_or_else(|| tz.to_owned()),
        ..Filters::default()
    }
}

fn unavailable(e: impl std::fmt::Display) -> Error {
    Error::Unavailable(e.to_string())
}

/// The input of the lookups that only need the project.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectInput {
    /// Project id.
    pub project: String,
}

/// The input of `ask.board`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoardInput {
    /// Project id.
    pub project: String,
    /// One canonical role only (at most 64 characters, else ignored).
    #[serde(default)]
    pub role: Option<String>,
}

/// `ask.board`: the Focus board of the last 30 days (KPIs, releases,
/// features, insight cards), optionally for one role.
pub struct Board;

impl Command for Board {
    const NAME: &'static str = "ask.board";
    const ACCESS: Access = Access::Read;
    type Input = BoardInput;
    type Output = BoardData;

    async fn run<H: Host>(
        host: &H,
        caller: &Caller,
        input: BoardInput,
    ) -> Result<BoardData, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let mut f = filters(caller, &p.timezone);
        f.role = input.role.filter(|r| r.len() <= MAX_ROLE);
        let map = host.feature_map(&p.id).await;
        host.board(&p.id, &f, map.as_ref().map(|(m, v)| (m, *v)))
            .await
            .map_err(unavailable)
    }
}

/// The input of `ask.feature`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeatureInput {
    /// Project id.
    pub project: String,
    /// The feature's key.
    pub key: String,
}

/// `ask.feature`: one feature in depth (definition, health findings, users
/// and success by role, top pages, adoption ladder); `None` when the map
/// has no such feature.
pub struct Feature;

impl Command for Feature {
    const NAME: &'static str = "ask.feature";
    const ACCESS: Access = Access::Read;
    type Input = FeatureInput;
    type Output = Option<FeatureContext>;

    async fn run<H: Host>(
        host: &H,
        caller: &Caller,
        input: FeatureInput,
    ) -> Result<Option<FeatureContext>, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let Some((m, _)) = host.feature_map(&p.id).await else {
            return Err(Error::Invalid("no feature map, or no key".into()));
        };
        let history = crate::inforce::history(host.meta(), &p.id);
        let f = filters(caller, &p.timezone);
        crate::feature_context(host.query(), &p.id, &m, &history, &f, &input.key)
            .await
            .map_err(unavailable)
    }
}

/// `ask.errors`: the error groups of the last 30 days, most frequent first,
/// with users, sessions and releases.
pub struct Errors;

impl Command for Errors {
    const NAME: &'static str = "ask.errors";
    const ACCESS: Access = Access::Read;
    type Input = ProjectInput;
    type Output = Vec<ErrorGroup>;

    async fn run<H: Host>(
        host: &H,
        caller: &Caller,
        input: ProjectInput,
    ) -> Result<Vec<ErrorGroup>, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let map = host.feature_map(&p.id).await;
        let src = crate::source(map.as_ref().map(|(m, _)| m));
        let f = filters(caller, &p.timezone);
        crate::error_groups(host.query(), &p.id, &src, &f)
            .await
            .map_err(unavailable)
    }
}

/// `ask.vitals`: Web Vitals of the last 7 days, explained.
pub struct Vitals;

impl Command for Vitals {
    const NAME: &'static str = "ask.vitals";
    const ACCESS: Access = Access::Read;
    type Input = ProjectInput;
    type Output = Vec<VitalReport>;

    async fn run<H: Host>(
        host: &H,
        caller: &Caller,
        input: ProjectInput,
    ) -> Result<Vec<VitalReport>, Error> {
        let p = host.project(&input.project).await.ok_or(Error::NotFound)?;
        let map = host.feature_map(&p.id).await;
        let f = filters(caller, &p.timezone);
        crate::vitals(host.query(), &p.id, &f, map.as_ref().map(|(m, _)| m))
            .await
            .map_err(unavailable)
    }
}

/// What `ask.goals` returns: the goals' status as the hourly pass last
/// wrote it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GoalsStatus {
    /// Objectives at risk.
    pub at_risk: Vec<String>,
    /// Each goal as a line, with its status.
    pub goals: Vec<(String, String)>,
}

/// `ask.goals`: goals and objectives with their status.
pub struct Goals;

impl Command for Goals {
    const NAME: &'static str = "ask.goals";
    const ACCESS: Access = Access::Read;
    type Input = ProjectInput;
    type Output = GoalsStatus;

    async fn run<H: Host>(host: &H, _: &Caller, input: ProjectInput) -> Result<GoalsStatus, Error> {
        let (at_risk, goals) = super::goals::snapshot(host.meta(), &input.project);
        Ok(GoalsStatus { at_risk, goals })
    }
}

/// The input of `ask.sql`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SqlInput {
    /// Project id.
    pub project: String,
    /// One read-only query (DataFusion) on the table `events`.
    pub query: String,
}

/// `ask.sql`: a read-only query on the project's events, aggregated unless
/// an admin allowed person-level detail for the project. A refused query
/// is [`Error::Invalid`], its text starting with `refused:`.
pub struct Sql;

impl Command for Sql {
    const NAME: &'static str = "ask.sql";
    const ACCESS: Access = Access::Read;
    type Input = SqlInput;
    type Output = Vec<Map<String, Value>>;

    async fn run<H: Host>(
        host: &H,
        _: &Caller,
        input: SqlInput,
    ) -> Result<Vec<Map<String, Value>>, Error> {
        let q = input.query;
        if q.len() > MAX_SQL {
            return Err(Error::Invalid("a query is required".into()));
        }
        let person = person_level(host, &input.project);
        sql_allowed(&q, person).map_err(|e| Error::Invalid(format!("refused: {e}")))?;
        let map = host.feature_map(&input.project).await;
        let src = crate::source(map.as_ref().map(|(m, _)| m));
        let cold = host.query().cold();
        let run = if person {
            let full = format!(
                "(SELECT *, distinct_id AS user_key, session_id AS session_key FROM {src}) AS events"
            );
            Box::pin(cold.sql_over(&input.project, &full, &q)).await
        } else {
            let key = pseudonym_key(host, &input.project);
            Box::pin(cold.sql_aggregate(&input.project, &src, key.as_bytes(), &q)).await
        };
        run.map(|r| r.rows).map_err(unavailable)
    }
}

/// A readable first check. Without person-level detail, the query then
/// runs on an aggregate-only view
/// ([`openlens_query::QueryEngine::sql_aggregate`]), which is what
/// actually keeps people out.
pub fn sql_allowed(query: &str, person_level: bool) -> Result<(), String> {
    let q = query
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if !q.starts_with("select") && !q.starts_with("with") {
        return Err("only SELECT queries".into());
    }
    if person_level {
        return Ok(());
    }
    for banned in ["distinct_id", "properties", "url", "account", "session_id"] {
        if q.contains(banned) {
            return Err(format!(
                "`{banned}` is person-level: count people with count(DISTINCT user_key) instead, or ask an admin to allow person-level detail for this project"
            ));
        }
    }
    if !(q.contains("group by")
        || ["count(", "sum(", "avg(", "min(", "max("]
            .iter()
            .any(|a| q.contains(a)))
    {
        return Err("aggregate the rows (GROUP BY, count, sum, avg)".into());
    }
    Ok(())
}

/// Did an admin allow person-level detail for the project?
fn person_level<H: Host>(host: &H, project: &str) -> bool {
    host.meta()
        .project_setting(project, "ai_person_level")
        .ok()
        .flatten()
        .as_deref()
        == Some("on")
}

/// The project's secret for `user_key` (never shown, never sent).
fn pseudonym_key<H: Host>(host: &H, project: &str) -> String {
    host.meta()
        .setting_or_init(&format!("project:{project}:ask_key"), || {
            host.random_hex(32)
        })
        .unwrap_or_else(|_| host.random_hex(32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_without_person_level_counts_but_does_not_list() {
        assert!(
            sql_allowed(
                "SELECT feature, count(DISTINCT user_key) FROM events GROUP BY 1",
                false
            )
            .is_ok()
        );
        assert!(sql_allowed("SELECT distinct_id FROM events", false).is_err());
        assert!(sql_allowed("SELECT properties FROM events GROUP BY 1", false).is_err());
        assert!(
            sql_allowed("SELECT event FROM events", false).is_err(),
            "not aggregated"
        );
        assert!(sql_allowed("DELETE FROM events", true).is_err());
        assert!(sql_allowed("select distinct_id from events", true).is_ok());
    }
}
