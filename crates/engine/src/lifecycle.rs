//! The lifecycle (#708, D-111): visitor → account created → activated →
//! paid → retained / churned, with fixed definitions (docs/kpis.md).
//!
//! A stage shows only when its data exists: visitors when the window has
//! anonymous traffic, activated when the map declares success events,
//! paid when a success event carries revenue, retention when the window
//! ends now. An internal tool, where everyone is identified, sees account
//! → activated → retained with no setting (D-014).
//!
//! Every number is aggregated in SQL (the engine returns at most 10 000
//! rows): the people rows of the board (`person_day` for the closed days,
//! D-038, D-040) give each person's last event and success events; the
//! `persons` table their signup (`signed_up`, else the first `$identify`,
//! #707); `raw_events`, cut to the ids of `person_aliases`, the accounts
//! whose anonymous visits were linked; the success events, the payers.

use chrono::{DateTime, Utc};
use openlens_featuremap::FeatureMap;
use openlens_query::QueryEngine;
use serde::Serialize;

use crate::board_people::People;
use crate::{
    Filters, NOT_SERVER, Result, Span, any_person, before, identified, int, lit, lookups, source,
    text,
};

/// What the lifecycle is split by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// One line for everyone.
    None,
    /// Each person's first landing channel in the window.
    Channel,
    /// Each person's first role in the window (the map's canonical role).
    Role,
    /// Each person's plan in the window.
    Plan,
    /// The release of each person's first event in the window.
    Release,
}

impl By {
    /// `channel`, `role`, `plan` or `release`; anything else is no split.
    #[must_use]
    pub fn parse(by: Option<&str>) -> Self {
        match by {
            Some("channel") => Self::Channel,
            Some("role") => Self::Role,
            Some("plan") => Self::Plan,
            Some("release") => Self::Release,
            _ => Self::None,
        }
    }

    /// Its name, none for no split.
    #[must_use]
    pub fn name(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Channel => Some("channel"),
            Self::Role => Some("role"),
            Self::Plan => Some("plan"),
            Self::Release => Some("release"),
        }
    }
}

/// One stage of the lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Stage {
    /// `visitor`, `account`, `activated`, `paid` or `retained`.
    pub key: &'static str,
    /// People at this stage.
    pub people: i64,
}

/// The visitor → account rate (D-111).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VisitorRate {
    /// False in cookieless mode: anonymous ids change every day and are
    /// never linked to an account (rule 11).
    pub available: bool,
    /// Why it is not available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Anonymous visitors of the window never linked to a user.
    pub visitors: i64,
    /// Accounts created in the window with a linked anonymous visit in it.
    pub linked_accounts: i64,
    /// Every account created in the window.
    pub accounts: i64,
    /// Lower bound: `linked_accounts / (visitors + linked_accounts)`.
    pub rate: f64,
    /// Upper bound: `accounts / (visitors + accounts)`.
    pub upper: f64,
}

/// Retention of the accounts created in the window (D-111).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Retention {
    /// Accounts created before the churn window started: old enough to judge.
    pub judged: i64,
    /// Of them, active in the last 7 days.
    pub retained: i64,
    /// Of them, inactive for longer than the churn window.
    pub churned: i64,
}

/// The lifecycle of one group (or everyone).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LifecycleLine {
    /// The group (channel, role, plan or release; empty without a split).
    pub value: String,
    /// The stages with data, in order.
    pub stages: Vec<Stage>,
    /// When the window has anonymous traffic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visitor_rate: Option<VisitorRate>,
    /// When the window ends now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention: Option<Retention>,
}

/// The lifecycle report.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Lifecycle {
    /// Days the window covers.
    pub days: i64,
    /// The churn window (`churn_after_days`).
    pub churn_days: i64,
    /// Everyone.
    pub total: LifecycleLine,
    /// What the groups are split by.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<&'static str>,
    /// Per group, most accounts first (at most 50).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<LifecycleLine>,
    /// Why there is nothing to show, when there is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The raw counts of one group.
#[derive(Debug, Clone, Default)]
struct Counts {
    visitors: i64,
    cookieless: i64,
    linked: i64,
    accounts: i64,
    activated: i64,
    paid: i64,
    judged: i64,
    retained: i64,
    churned: i64,
}

impl Counts {
    fn add(&mut self, o: &Counts) {
        self.visitors += o.visitors;
        self.cookieless += o.cookieless;
        self.linked += o.linked;
        self.accounts += o.accounts;
        self.activated += o.activated;
        self.paid += o.paid;
        self.judged += o.judged;
        self.retained += o.retained;
        self.churned += o.churned;
    }
}

/// Which stages have data, decided once for every group.
#[derive(Debug, Clone, Copy)]
struct Present {
    visitors: bool,
    cookieless: bool,
    activated: bool,
    paid: bool,
    retention: bool,
}

fn ratio(n: i64, of: i64) -> f64 {
    if of > 0 { n as f64 / of as f64 } else { 0.0 }
}

/// Why the visitor → account rate is not available in cookieless mode.
const COOKIELESS: &str = "not available in cookieless mode: anonymous visitors get a new id every day and are never linked to the account they create";

fn line(value: String, c: &Counts, p: Present) -> LifecycleLine {
    let mut stages = Vec::new();
    if p.visitors {
        stages.push(Stage {
            key: "visitor",
            people: c.visitors + c.linked,
        });
    }
    stages.push(Stage {
        key: "account",
        people: c.accounts,
    });
    if p.activated {
        stages.push(Stage {
            key: "activated",
            people: c.activated,
        });
    }
    if p.paid {
        stages.push(Stage {
            key: "paid",
            people: c.paid,
        });
    }
    if p.retention {
        stages.push(Stage {
            key: "retained",
            people: c.retained,
        });
    }
    let visitor_rate = p.visitors.then(|| VisitorRate {
        available: !p.cookieless,
        why: p.cookieless.then(|| COOKIELESS.to_owned()),
        visitors: c.visitors,
        linked_accounts: c.linked,
        accounts: c.accounts,
        rate: if p.cookieless {
            0.0
        } else {
            ratio(c.linked, c.visitors + c.linked)
        },
        upper: if p.cookieless {
            0.0
        } else {
            ratio(c.accounts, c.visitors + c.accounts)
        },
    });
    let retention = p.retention.then_some(Retention {
        judged: c.judged,
        retained: c.retained,
        churned: c.churned,
    });
    LifecycleLine {
        value,
        stages,
        visitor_rate,
        retention,
    }
}

/// What the lifecycle reads, shared with the board (which has built most
/// of it already).
pub(crate) struct Reads<'a> {
    /// The event source (the map's roles and features).
    pub src: &'a str,
    /// Who is a user (`identified`).
    pub ident: &'a str,
    /// The board's people rows.
    pub people: &'a People<'a>,
    /// The declared success events, as SQL literals joined by commas
    /// (empty without any).
    pub success_list: &'a str,
    /// The churn window, in days.
    pub churn_days: i64,
}

/// The lifecycle over `span` at `now` (the board's own window).
pub(crate) async fn compute(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    span: Span,
    now: DateTime<Utc>,
    r: &Reads<'_>,
    by: By,
) -> Result<Lifecycle> {
    let rows = r.people.rows_in(f, span)?;
    let w = f.clause_span(span, now)?;
    let (src, ident) = (r.src, r.ident);
    let start = before(now, span.start);
    let until = if span.end > 0 {
        format!(" AND s.signup < {}", before(now, span.end))
    } else {
        String::new()
    };
    let churn_cut = before(now, r.churn_days * 86_400);
    let week = before(now, 7 * 86_400);
    // Each person's group: from the people rows, or from the events for
    // what they do not hold (the plan, the landing channel).
    let (in_rows, keyed, key) = match by {
        By::None => ("", String::new(), "''"),
        By::Role => (
            ", first_value(role ORDER BY first_ts) AS g",
            String::new(),
            "coalesce(x.g, '(none)')",
        ),
        By::Release => (
            ", first_value(release ORDER BY first_ts) AS g",
            String::new(),
            "coalesce(x.g, '(none)')",
        ),
        By::Plan => (
            "",
            format!(
                ", k AS (SELECT distinct_id, max(plan) AS g FROM {src} WHERE {w} AND plan IS NOT NULL GROUP BY distinct_id)"
            ),
            "coalesce(k.g, '(none)')",
        ),
        By::Channel => (
            "",
            format!(
                ", k AS (SELECT distinct_id, first_value(acq_channel ORDER BY timestamp) AS g FROM {src} \
                 WHERE {w} AND event = '$pageview' AND acq_channel IS NOT NULL GROUP BY distinct_id)"
            ),
            "coalesce(k.g, '(none)')",
        ),
    };
    let join_k = if keyed.is_empty() {
        ""
    } else {
        " LEFT JOIN k ON k.distinct_id = x.distinct_id"
    };
    let payers = if r.success_list.is_empty() {
        "SELECT CAST(NULL AS VARCHAR) AS distinct_id WHERE false".to_owned()
    } else {
        format!(
            "SELECT DISTINCT distinct_id FROM {src} WHERE {w} AND event IN ({list}) AND {amount} IS NOT NULL",
            list = r.success_list,
            amount = crate::revenue::AMOUNT,
        )
    };
    let accounts_sql = format!(
        "WITH x AS (SELECT distinct_id, max(last_ts) AS last_ts, sum(successes) AS successes{in_rows} \
         FROM {rows} WHERE {ident} GROUP BY distinct_id){keyed}, \
         s AS (SELECT person, COALESCE(signed_up, first_seen) AS signup FROM persons), \
         l AS (SELECT DISTINCT a.person FROM (SELECT DISTINCT distinct_id FROM raw_events \
               WHERE {w} AND distinct_id IN (SELECT anon FROM person_aliases)) r \
               JOIN person_aliases a ON a.anon = r.distinct_id), \
         pay AS ({payers}) \
         SELECT {key} AS g, count(*) AS accounts, \
         sum(CASE WHEN x.successes > 0 THEN 1 ELSE 0 END) AS activated, \
         sum(CASE WHEN pay.distinct_id IS NOT NULL THEN 1 ELSE 0 END) AS paid, \
         sum(CASE WHEN l.person IS NOT NULL THEN 1 ELSE 0 END) AS linked, \
         sum(CASE WHEN s.signup < {churn_cut} THEN 1 ELSE 0 END) AS judged, \
         sum(CASE WHEN s.signup < {churn_cut} AND x.last_ts >= {week} THEN 1 ELSE 0 END) AS retained, \
         sum(CASE WHEN s.signup < {churn_cut} AND x.last_ts < {churn_cut} THEN 1 ELSE 0 END) AS churned \
         FROM x JOIN s ON s.person = x.distinct_id{join_k} \
         LEFT JOIN pay ON pay.distinct_id = x.distinct_id LEFT JOIN l ON l.person = x.distinct_id \
         WHERE s.signup >= {start}{until} GROUP BY 1"
    );
    // Visitors never linked to a user (the `events` view gives a linked
    // anonymous id its user); the server's daily cookieless ids start
    // with `ck_` (#273).
    let visitors_sql = format!(
        "WITH x AS (SELECT distinct_id{in_rows} FROM {rows} WHERE NOT ({ident}) AND {NOT_SERVER} \
         GROUP BY distinct_id){keyed} \
         SELECT {key} AS g, count(*) AS visitors, \
         sum(CASE WHEN starts_with(x.distinct_id, 'ck_') THEN 1 ELSE 0 END) AS cookieless \
         FROM x{join_k} GROUP BY 1"
    );
    let paid_sql = if r.success_list.is_empty() {
        None
    } else {
        Some(format!("SELECT count(*) AS n FROM ({payers}) AS p"))
    };
    let paid_any = async {
        match &paid_sql {
            Some(sql) => Ok::<_, crate::EngineError>(
                q.sql(project, sql)
                    .await?
                    .rows
                    .first()
                    .map_or(0, |r| int(r, "n")),
            ),
            None => Ok(0),
        }
    };
    let (accounts, visitors, paid_any) = futures::join!(
        q.sql(project, &accounts_sql),
        q.sql(project, &visitors_sql),
        paid_any,
    );
    let mut groups: std::collections::BTreeMap<String, Counts> = Default::default();
    for row in &accounts?.rows {
        let c = groups
            .entry(text(row, "g").unwrap_or_default())
            .or_default();
        c.accounts += int(row, "accounts");
        c.activated += int(row, "activated");
        c.paid += int(row, "paid");
        c.linked += int(row, "linked");
        c.judged += int(row, "judged");
        c.retained += int(row, "retained");
        c.churned += int(row, "churned");
    }
    for row in &visitors?.rows {
        let c = groups
            .entry(text(row, "g").unwrap_or_default())
            .or_default();
        c.visitors += int(row, "visitors");
        c.cookieless += int(row, "cookieless");
    }
    let mut total = Counts::default();
    for c in groups.values() {
        total.add(c);
    }
    let present = Present {
        visitors: total.visitors + total.linked > 0,
        // Most never-linked visitors carry the server's daily ids.
        cookieless: total.cookieless * 2 > total.visitors,
        activated: !r.success_list.is_empty(),
        paid: paid_any? > 0,
        retention: span.end == 0,
    };
    let note = (total.accounts == 0 && !present.visitors)
        .then(|| "no account created and no anonymous visitor in this window".to_owned());
    let mut lines: Vec<LifecycleLine> = if by == By::None {
        Vec::new()
    } else {
        groups
            .iter()
            .map(|(v, c)| line(v.clone(), c, present))
            .collect()
    };
    let size = |l: &LifecycleLine| l.stages.iter().map(|s| s.people).max().unwrap_or(0);
    lines.sort_by(|a, b| size(b).cmp(&size(a)).then(a.value.cmp(&b.value)));
    lines.truncate(50);
    Ok(Lifecycle {
        days: (span.start - span.end + 86_399) / 86_400,
        churn_days: r.churn_days,
        total: line(String::new(), &total, present),
        by: by.name(),
        groups: lines,
        note,
    })
}

/// The lifecycle of `project` for the filters `f` (its window: the
/// release window or a range when asked, else the last 30 days), split
/// `by` a channel, role, plan or release.
///
/// # Errors
/// A failed query or an invalid filter.
pub async fn lifecycle(
    q: &QueryEngine,
    project: &str,
    map: Option<(&FeatureMap, i64)>,
    f: &Filters,
    by: By,
) -> Result<Lifecycle> {
    let q = &q.with_params(&f.params()?);
    let now = Utc::now();
    let win = crate::board_window::resolve(q, project, f, now).await?;
    let span = win.as_ref().map_or(Span::days(30), |w| w.current);
    let back = (span.start / 86_400 + 2).max(90);
    let m = map.map(|(m, _)| m);
    let src = source(m);
    let ident = identified(any_person(q, project).await?);
    let rolled = crate::board_people::rolled_days_back(
        q,
        project,
        "person_day",
        map.map(|(_, v)| v),
        now,
        back,
    )
    .await?;
    let successes: Vec<&str> = m
        .map(|m| {
            m.features
                .iter()
                .filter_map(|x| x.success.as_deref())
                .collect()
        })
        .unwrap_or_default();
    let success_list = successes
        .iter()
        .map(|e| lit(e))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let task = if successes.is_empty() {
        "feature IS NOT NULL".to_owned()
    } else {
        format!("event IN ({success_list})")
    };
    let people = People {
        src: &src,
        role: m.map_or_else(|| "role".to_owned(), |m| lookups(m).0),
        task: &task,
        rolled,
        now,
    };
    let reads = Reads {
        src: &src,
        ident,
        people: &people,
        success_list: &success_list,
        churn_days: m.map_or(14, |m| i64::from(m.churn_days())),
    };
    compute(q, project, f, span, now, &reads, by).await
}
