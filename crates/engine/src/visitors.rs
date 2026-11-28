//! The board's Visitors block (#709, D-101): the anonymous traffic of the
//! window, per entry page and per channel, with the signup rate of each.
//!
//! - An **anonymous session** is one whose stored id is not a user's id,
//!   so a visit made before login still counts after the id was linked.
//! - Its **entry page** is the path of its first page view, its
//!   **channel** that page view's landing channel.
//! - A **bounce** is a session with at most one interaction (docs/kpis.md).
//! - **Signed up**: visitors (anonymous ids) linked to a user whose signup
//!   (#707) falls in the window. The rate, signed up ÷ visitors, is a
//!   lower bound: a signup on another device leaves no link.
//!
//! Two bounded scans of the window, aggregated in SQL (top 10 each).

use chrono::{DateTime, Utc};
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::{Filters, INTERACTION, NOT_SERVER, Result, Span, before, int, text};

/// One entry page or channel.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct VisitorRow {
    /// The page's path, or the channel.
    pub value: String,
    /// Anonymous sessions that started there.
    pub sessions: i64,
    /// Distinct anonymous visitors among them.
    pub visitors: i64,
    /// Sessions with at most one interaction.
    pub bounces: i64,
    /// Visitors linked to an account created in the window.
    pub signed_up: i64,
}

impl VisitorRow {
    /// Share of the sessions that bounced.
    #[must_use]
    pub fn bounce_rate(&self) -> f64 {
        self.bounces as f64 / self.sessions.max(1) as f64
    }

    /// Signed up ÷ visitors (a lower bound).
    #[must_use]
    pub fn signup_rate(&self) -> f64 {
        self.signed_up as f64 / self.visitors.max(1) as f64
    }
}

/// The Visitors block.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Visitors {
    /// Anonymous sessions with a page view in the window.
    pub sessions: i64,
    /// Of them, bounces.
    pub bounces: i64,
    /// The top 10 entry pages, most visitors first.
    pub pages: Vec<VisitorRow>,
    /// The top 10 channels, most visitors first.
    pub channels: Vec<VisitorRow>,
}

/// The block over `span` at `now`, or none when the window has no
/// anonymous session with a page view. `src` is the board's event source.
pub(crate) async fn visitors(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    span: Span,
    now: DateTime<Utc>,
    src: &str,
) -> Result<Option<Visitors>> {
    let w = f.clause_span(span, now)?;
    let start = before(now, span.start);
    let until = if span.end > 0 {
        format!(" AND COALESCE(p.signed_up, p.first_seen) < {}", before(now, span.end))
    } else {
        String::new()
    };
    // Sessions read on the stored ids (`raw_events`): a linked anonymous
    // id is still anonymous there. Landings read on the board's source,
    // for the typed landing facts; a session id is the same on both.
    let with = format!(
        "WITH ses AS (SELECT session_id, min(distinct_id) AS distinct_id, \
               sum(CASE WHEN {INTERACTION} THEN 1 ELSE 0 END) AS n FROM raw_events \
               WHERE {w} AND session_id IS NOT NULL AND {NOT_SERVER} GROUP BY session_id), \
         anon AS (SELECT * FROM ses WHERE NOT (distinct_id IN (SELECT person FROM persons))), \
         land AS (SELECT session_id, first_value(path ORDER BY timestamp) AS entry, \
               first_value(acq_channel ORDER BY timestamp) AS ch FROM {src} \
               WHERE {w} AND event = '$pageview' AND session_id IS NOT NULL GROUP BY session_id), \
         acc AS (SELECT a.anon FROM person_aliases a JOIN persons p ON p.person = a.person \
               WHERE COALESCE(p.signed_up, p.first_seen) >= {start}{until})"
    );
    let grouped = |col: &str, limit: &str| {
        format!(
            "{with} SELECT coalesce(land.{col}, '(none)') AS v, count(*) AS sessions, \
             count(DISTINCT anon.distinct_id) AS visitors, \
             sum(CASE WHEN anon.n <= 1 THEN 1 ELSE 0 END) AS bounces, \
             count(DISTINCT acc.anon) AS signed_up \
             FROM anon JOIN land ON land.session_id = anon.session_id \
             LEFT JOIN acc ON acc.anon = anon.distinct_id \
             GROUP BY 1 ORDER BY visitors DESC, sessions DESC, v{limit}"
        )
    };
    // Channels are a short fixed list: all of them, so the sessions and
    // bounces of the block are their sums.
    let (pages_sql, channels_sql) = (grouped("entry", " LIMIT 10"), grouped("ch", ""));
    let (pages, channels) = futures::join!(
        q.sql(project, &pages_sql),
        q.sql(project, &channels_sql),
    );
    let read = |rows: &[serde_json::Map<String, serde_json::Value>]| -> Vec<VisitorRow> {
        rows.iter()
            .map(|r| VisitorRow {
                value: text(r, "v").unwrap_or_default(),
                sessions: int(r, "sessions"),
                visitors: int(r, "visitors"),
                bounces: int(r, "bounces"),
                signed_up: int(r, "signed_up"),
            })
            .collect()
    };
    let pages = read(&pages?.rows);
    let mut channels = read(&channels?.rows);
    let sessions = channels.iter().map(|c| c.sessions).sum();
    let bounces = channels.iter().map(|c| c.bounces).sum();
    if sessions == 0 {
        return Ok(None);
    }
    channels.truncate(10);
    Ok(Some(Visitors {
        sessions,
        bounces,
        pages,
        channels,
    }))
}
