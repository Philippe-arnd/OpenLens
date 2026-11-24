//! The weekly digest: this week against last week, in a few lines, for the
//! PM's inbox and for agents (`digest` MCP tool).

use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::{Filters, Result, int, lit, source, text};

/// One number, this week and last week.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Delta {
    /// Last 7 days.
    pub now: i64,
    /// The 7 days before.
    pub before: i64,
}

impl Delta {
    /// Relative change, or `None` when last week was 0.
    pub fn change(&self) -> Option<f64> {
        (self.before > 0).then(|| (self.now - self.before) as f64 / self.before as f64)
    }
}

/// The digest of one project.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Digest {
    /// Active users.
    pub users: Delta,
    /// Sessions.
    pub sessions: Delta,
    /// Error events.
    pub errors: Delta,
    /// Completed tasks (success events of the feature map), per feature key.
    pub tasks: Vec<(String, Delta)>,
    /// The top insight cards of the board right now.
    pub insights: Vec<crate::Insight>,
    /// Goals, one line each with its status (#272; set by the server from
    /// the hourly goals pass).
    pub goals: Vec<(String, String)>,
    /// Objectives at risk or off track (#272).
    pub objectives_at_risk: Vec<String>,
}

/// Compute the digest (production environment, all roles).
pub async fn digest(
    q: &QueryEngine,
    project: &str,
    map: Option<(&FeatureMap, i64)>,
) -> Result<Digest> {
    let f = Filters::default();
    let q = &q.with_params(&f.params()?);
    let src = source(map.map(|(m, _)| m));
    let w = f.clause(14)?;
    let now = "timestamp >= now() - INTERVAL '7 days'";
    let sql = format!(
        "SELECT count(DISTINCT CASE WHEN {now} THEN distinct_id END) AS u1, \
                count(DISTINCT CASE WHEN NOT ({now}) THEN distinct_id END) AS u0, \
                count(DISTINCT CASE WHEN {now} THEN session_id END) AS s1, \
                count(DISTINCT CASE WHEN NOT ({now}) THEN session_id END) AS s0, \
                sum(CASE WHEN event = '$error' AND {now} THEN 1 ELSE 0 END) AS e1, \
                sum(CASE WHEN event = '$error' AND NOT ({now}) THEN 1 ELSE 0 END) AS e0 \
         FROM {src} WHERE {w}"
    );
    let successes: Vec<(String, String)> = map
        .map(|(m, _)| {
            m.features
                .iter()
                .filter_map(|x| Some((x.key.clone(), x.success.clone()?)))
                .collect()
        })
        .unwrap_or_default();
    let tasks_sql = if successes.is_empty() {
        None
    } else {
        let list = successes
            .iter()
            .map(|(_, e)| lit(e))
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        Some(format!(
            "SELECT event, sum(CASE WHEN {now} THEN 1 ELSE 0 END) AS n1, \
                    sum(CASE WHEN NOT ({now}) THEN 1 ELSE 0 END) AS n0 \
             FROM {src} WHERE {w} AND event IN ({list}) GROUP BY event"
        ))
    };
    let totals = q.sql(project, &sql).await?;
    let mut d = Digest::default();
    if let Some(r) = totals.rows.first() {
        d.users = Delta {
            now: int(r, "u1"),
            before: int(r, "u0"),
        };
        d.sessions = Delta {
            now: int(r, "s1"),
            before: int(r, "s0"),
        };
        d.errors = Delta {
            now: int(r, "e1"),
            before: int(r, "e0"),
        };
    }
    if let Some(sql) = tasks_sql {
        let rows = q.sql(project, &sql).await?.rows;
        for (key, ev) in &successes {
            let r = rows
                .iter()
                .find(|r| text(r, "event").as_deref() == Some(ev.as_str()));
            d.tasks.push((
                key.clone(),
                Delta {
                    now: r.map_or(0, |r| int(r, "n1")),
                    before: r.map_or(0, |r| int(r, "n0")),
                },
            ));
        }
        d.tasks.sort_by_key(|t| std::cmp::Reverse(t.1.now));
    }
    let board = crate::board(q, project, &f, map).await?;
    d.insights = board.insights.into_iter().take(3).collect();
    Ok(d)
}

fn trend(d: &Delta) -> String {
    match d.change() {
        Some(c) if c.abs() < 0.005 => "→ same as last week".into(),
        Some(c) => format!(
            "{} {:+.0}% vs last week ({})",
            if c > 0.0 { "↗" } else { "↘" },
            c * 100.0,
            d.before
        ),
        None if d.now > 0 => "new this week".into(),
        None => "—".into(),
    }
}

/// The digest as Markdown (email body, agents).
pub fn digest_markdown(project_name: &str, board_url: &str, d: &Digest) -> String {
    let mut s = format!("# {project_name}: your week\n\n");
    s.push_str(&format!(
        "- **Active users:** {} {}\n",
        d.users.now,
        trend(&d.users)
    ));
    s.push_str(&format!(
        "- **Sessions:** {} {}\n",
        d.sessions.now,
        trend(&d.sessions)
    ));
    s.push_str(&format!(
        "- **Errors:** {} {}\n",
        d.errors.now,
        trend(&d.errors)
    ));
    if !d.tasks.is_empty() {
        s.push_str("\n## Completed tasks\n\n");
        for (k, t) in &d.tasks {
            s.push_str(&format!("- **{k}:** {} {}\n", t.now, trend(t)));
        }
    }
    if !d.insights.is_empty() {
        s.push_str("\n## What to focus on\n\n");
        for i in &d.insights {
            s.push_str(&format!(
                "- **{}** ({}). Next step: {}\n",
                i.title, i.evidence, i.next_step
            ));
        }
    }
    if !d.goals.is_empty() {
        s.push_str("\n## Goals\n\n");
        for o in &d.objectives_at_risk {
            s.push_str(&format!("- **Objective at risk:** {o}\n"));
        }
        for (line, st) in &d.goals {
            s.push_str(&format!("- {line} — **{st}**\n"));
        }
    }
    s.push_str(&format!("\nOpen the board: {board_url}\n"));
    s
}
