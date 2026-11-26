//! The board (#452): the numbers of every block, and the filter bar's
//! options. No block reads a projection held in memory (#502, D-051): the
//! people, sessions and releases blocks combine the daily rollups of
//! closed days with the events of the other days (D-040, D-047); success
//! events and Web Vitals are event-filtered scans of 30 and 7 days. The
//! insight cards, retention and resurrected users read the same people and
//! session rows, and journeys `session_feature_day` with the first success
//! from an event-filtered scan (#452, D-059); task times, flag cohorts and
//! funnels read their own window of days from the files.
//!
//! The period blocks (sessions, tasks, errors, the daily active series,
//! features, roles, role × feature and success events) read 7, 14 or 30
//! days, or, on the board pages, the release window and its comparison
//! (#323, D-055, [`board_window`]), or a date range or past release
//! (#513, D-060).

use super::*;

/// What the filter bar of a page needs, without the board (#329): the
/// environments, the roles (with the unmatched ones apart, as on the
/// board) and the latest releases. The Errors, Pages and Events pages call
/// this instead of computing the whole board.
pub async fn filter_options(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<&FeatureMap>,
) -> Result<Board> {
    let q = &q.with_params(&f.params()?);
    let src = source(map);
    let roles_sql = format!(
        "SELECT role, count(DISTINCT distinct_id) AS users FROM {src} WHERE {} \
         GROUP BY role ORDER BY users DESC LIMIT 12",
        Filters {
            role: None,
            ..f.clone()
        }
        .clause(30)?
    );
    let releases_sql = format!(
        "SELECT release, CAST(min(timestamp) AS VARCHAR) AS first_seen, \
         count(DISTINCT distinct_id) AS users FROM {src} WHERE {} AND release IS NOT NULL \
         GROUP BY release ORDER BY min(timestamp) DESC LIMIT 5",
        Filters {
            release: None,
            ..f.clone()
        }
        .clause(90)?
    );
    let (roles, releases, stored) = futures::join!(
        q.sql(project, &roles_sql),
        q.sql(project, &releases_sql),
        q.raw_summary(project),
    );
    let mut b = Board {
        filters: Some(f.clone()),
        ..Board::default()
    };
    for r in roles?.rows.iter() {
        let role = text(r, "role");
        let users = int(r, "users");
        match role.as_deref().and_then(|v| v.strip_prefix("unmatched:")) {
            Some(raw) => b.role_gaps.push((raw.to_owned(), users)),
            None => b.roles.push(RoleUsers { role, users }),
        }
    }
    b.releases = releases?
        .rows
        .iter()
        .map(|r| ReleaseRow {
            release: text(r, "release").unwrap_or_default(),
            first_seen: text(r, "first_seen").unwrap_or_default(),
            users: int(r, "users"),
            ..ReleaseRow::default()
        })
        .collect();
    b.environments = stored?.environments;
    Ok(b)
}

/// Compute the board for a project, with its feature map when it has one.
/// Queries run concurrently.
pub async fn board(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
) -> Result<Board> {
    board_reusing(q, project, f, map, None).await
}

/// [`board`], with the period blocks `kept` from an earlier board of the
/// same filters and map over the same closed days (#516, D-064): they are
/// not read again, every other block is.
pub async fn board_reusing(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
    kept: Option<&board_kept::PeriodBlocks>,
) -> Result<Board> {
    let q = &q.with_params(&f.params()?);
    let reuse = kept.is_some();
    // Every query reads the events with roles and features resolved from
    // the map, cut to its own window of days (partition pruning on
    // `date`), and only the columns it names: no 90-day copy is held in
    // memory (#502, D-051). Web Vitals are a 7-day scan of `$vital` rows
    // and their typed columns (#449), success events a 30-day scan of the
    // declared success events.
    let src = source(map.map(|(m, _)| m));
    let ident = identified(any_person(q, project).await?);
    // The people blocks read `person_day` for the closed days rolled up
    // with the map in force, the events for the rest (#492, D-040).
    // The sessions block and the releases table do the same with
    // `session_day` (#498, D-047).
    let now = chrono::Utc::now();
    // The board pages read their period blocks since the current release
    // (D-055), or a date range or past release they ask for (D-060); a
    // board filtered on one release already reads that release, and keeps
    // the fixed windows, as every other caller.
    let win = board_window::resolve(q, project, f, now).await?;
    // The comparison reaches twice the window back (more for a range in
    // the past).
    let back = win
        .as_ref()
        .map_or(90, |w| w.previous.start / 86_400 + 2)
        .max(90);
    let version = map.map(|(_, v)| v);
    // Journeys read `session_feature_day` the same way (#452, D-059), and
    // success events (the block, the first success of each journey)
    // `success_day` (D-062).
    let (rolled_people, rolled_sessions, rolled_features, rolled_successes) = futures::try_join!(
        board_people::rolled_days_back(q, project, "person_day", version, now, back),
        board_people::rolled_days_back(q, project, "session_day", version, now, back),
        board_people::rolled_days(q, project, "session_feature_day", version, now),
        board_people::rolled_days_back(q, project, "success_day", version, now, back),
    )?;
    // A day with neither its events nor rollups made with the map in
    // force counts nobody: the board names it (#489, D-070).
    let unavailable = crate::unavailable::days(
        q,
        project,
        (now - Duration::days(back - 1)).date_naive(),
        &[&rolled_people, &rolled_sessions, &rolled_successes],
        now,
    )
    .await?;
    // Most blocks read the same closed days of the same rollups: they are
    // read once, into memory, for this board's queries (#452, D-072).
    let held = hold_rollups(
        q,
        project,
        &[
            ("person_day", &rolled_people),
            ("session_day", &rolled_sessions),
            ("session_feature_day", &rolled_features),
            ("success_day", &rolled_successes),
        ],
    )
    .await;
    let q = &held;
    // A date range or a past release says what it read (D-061).
    let costed = match &win {
        Some(w) if !reuse && matches!(f.window, Window::Range { .. } | Window::Since { .. }) => {
            let sessions: std::collections::BTreeSet<_> = rolled_sessions.iter().collect();
            let rolled: std::collections::BTreeSet<chrono::NaiveDate> = rolled_people
                .iter()
                .filter(|d| sessions.contains(d))
                .copied()
                .collect();
            let first = (now - Duration::seconds(w.previous.start)).date_naive();
            Some((rolled, first))
        }
        _ => None,
    };
    // A period block's span: its fixed days, or the release window.
    let span = |days: i64| win.as_ref().map_or(Span::days(days), |w| w.current);
    let clause = |days: i64| match &win {
        Some(w) => f.clause_span(w.current, now),
        None => f.clause(days),
    };
    let role = map.map_or_else(|| "role".to_owned(), |(m, _)| lookups(m).0);
    // Completed tasks are success events when the map declares them;
    // without a map, every feature use counts.
    let successes: Vec<&str> = map
        .map(|(m, _)| {
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
    let people = board_people::People {
        src: &src,
        role: role.clone(),
        task: &task,
        rolled: rolled_people,
        now,
    };
    let journey_rows = board_sessions::feature_rows(&src, &role, &rolled_features, now, f, 30)?;
    let success_reads = board_success::SuccessRows {
        src: &src,
        role: role.clone(),
        rolled: rolled_successes,
        now,
    };
    let any_role = Filters {
        role: None,
        ..f.clone()
    };
    // MAU, known and anonymous users keep their KPI windows (docs/kpis.md);
    // features, roles and role × feature are period blocks.
    let (p30, p_features, p_roles, p90) = (
        people.rows(f, 30)?,
        people.rows_in(f, span(30))?,
        people.rows_in(&any_role, span(30))?,
        people.rows(f, 90)?,
    );
    // Rollup days are UTC days: a block grouped by the project's day reads
    // them only where the two agree (D-017, D-040).
    let utc_days = |days: i64| {
        board_people::utc_days(
            &f.tz,
            (now - Duration::seconds(span(days).start)).date_naive(),
            now.date_naive(),
        )
    };
    let w30 = clause(30)?;
    let w14 = clause(14)?;
    // Days are the project's days (D-017).
    let day = f.day()?;
    let w7 = f.clause(7)?;

    let users_sql = format!(
        "SELECT count(DISTINCT CASE WHEN last_ts >= now() - INTERVAL '1 day' THEN distinct_id END) AS dau, \
         count(DISTINCT CASE WHEN last_ts >= now() - INTERVAL '7 days' THEN distinct_id END) AS wau, \
         count(DISTINCT distinct_id) AS mau FROM {p30} WHERE {ident}"
    );
    let known_sql = format!("SELECT count(DISTINCT distinct_id) AS n FROM {p90} WHERE {ident}");
    let anonymous_sql =
        format!("SELECT count(DISTINCT distinct_id) AS n FROM {p30} WHERE NOT {ident}");
    let sessions = board_sessions::SessionRows {
        src: &src,
        role,
        task: &task,
        rolled: rolled_sessions,
        now,
    };
    let sessions_sql = sessions.sessions_sql(f, span(7))?;
    // The insight cards read the same rows as the blocks, over their own
    // fixed windows (#452, D-059).
    let card_rows = insights::CardRows {
        people90: p90.clone(),
        people30: p30.clone(),
        roles30: people.rows(&any_role, 30)?,
        sessions90: sessions.rows(
            &Filters {
                release: None,
                ..f.clone()
            },
            Span::days(90),
        )?,
    };
    let dau_sql = if utc_days(14) {
        format!(
            "SELECT day AS date, count(DISTINCT distinct_id) AS n FROM {} WHERE {ident} GROUP BY 1 ORDER BY 1",
            people.rows_in(f, span(14))?
        )
    } else {
        format!(
            "SELECT {day} AS date, count(DISTINCT distinct_id) AS n FROM {src} WHERE {w14} AND {ident} GROUP BY 1 ORDER BY 1"
        )
    };
    let vitals_sql = vitals::p75_sql(&src, &w7);
    let features_sql = if utc_days(30) {
        format!(
            "SELECT feature AS key, sum(events) AS uses, count(DISTINCT distinct_id) AS users, \
             count(DISTINCT concat(distinct_id, '|', day)) AS user_days \
             FROM {p_features} WHERE feature IS NOT NULL GROUP BY feature ORDER BY users DESC, uses DESC LIMIT 12"
        )
    } else {
        format!(
            "SELECT feature AS key, count(*) AS uses, count(DISTINCT distinct_id) AS users, \
             count(DISTINCT concat(distinct_id, '|', {day})) AS user_days \
             FROM {src} WHERE {w30} AND feature IS NOT NULL GROUP BY feature ORDER BY users DESC, uses DESC LIMIT 12"
        )
    };
    let roles_sql = format!(
        "SELECT role, count(DISTINCT distinct_id) AS users FROM {p_roles} \
         GROUP BY role ORDER BY users DESC LIMIT 12"
    );
    let role_features_sql = format!(
        "SELECT role, feature, count(DISTINCT distinct_id) AS users FROM {p_roles} \
         WHERE role IS NOT NULL AND feature IS NOT NULL GROUP BY role, feature"
    );
    let releases_sql = sessions.releases_sql(f)?;
    // The environments and the stored row count do not depend on who did
    // what: they come from the segments, which are read once (#447), not
    // from a query.

    // Success events: 30 days (or the release window) of the declared
    // events only: `success_day` for the closed days, the events for the
    // rest (#502, D-062).
    let success_sql = if successes.is_empty() {
        "SELECT CAST(NULL AS VARCHAR) AS event, 0 AS n, 0 AS users WHERE false".to_owned()
    } else {
        format!(
            "SELECT event, sum(events) AS n, count(DISTINCT distinct_id) AS users FROM {} GROUP BY event",
            success_reads.rows_in(f, span(30), &success_list)?
        )
    };
    // The release window's comparison (D-055): users of the window and of
    // the same length before it, and the sessions block before it.
    let compare = match &win {
        Some(w) if !reuse => {
            let users = |s: Span| -> Result<String> {
                Ok(format!(
                    "SELECT count(DISTINCT distinct_id) AS n FROM {} WHERE {ident}",
                    people.rows_in(f, s)?
                ))
            };
            Some((
                users(w.current)?,
                users(w.previous)?,
                sessions.sessions_sql(f, w.previous)?,
            ))
        }
        _ => None,
    };
    let compared = async {
        let Some((now_users, before_users, before_sessions)) = &compare else {
            return Ok::<_, EngineError>(None);
        };
        let (a, b, c) = futures::join!(
            q.sql(project, now_users),
            q.sql(project, before_users),
            q.sql(project, before_sessions),
        );
        Ok(Some((a?, b?, c?)))
    };
    let (
        users,
        sessions,
        dau,
        vitals,
        features,
        roles,
        releases,
        stored,
        success,
        role_features,
        anonymous,
        known,
        compared,
    ) = futures::join!(
        q.sql(project, &users_sql),
        board_kept::unless_kept(q, project, &sessions_sql, reuse),
        board_kept::unless_kept(q, project, &dau_sql, reuse),
        q.sql(project, &vitals_sql),
        board_kept::unless_kept(q, project, &features_sql, reuse),
        board_kept::unless_kept(q, project, &roles_sql, reuse),
        q.sql(project, &releases_sql),
        q.raw_summary(project),
        board_kept::unless_kept(q, project, &success_sql, reuse),
        board_kept::unless_kept(q, project, &role_features_sql, reuse),
        q.sql(project, &anonymous_sql),
        q.sql(project, &known_sql),
        compared,
    );

    let mut b = Board {
        filters: Some(f.clone()),
        unavailable_days: unavailable,
        ..Board::default()
    };
    if let Some(r) = users?.rows.first() {
        b.users = Users {
            dau: int(r, "dau"),
            wau: int(r, "wau"),
            mau: int(r, "mau"),
            stickiness: if int(r, "mau") > 0 {
                int(r, "dau") as f64 / int(r, "mau") as f64
            } else {
                0.0
            },
            resurrected: 0,
            anonymous: anonymous?.rows.first().map_or(0, |r| int(r, "n")),
            known: known?.rows.first().map_or(0, |r| int(r, "n")),
        };
    }
    if let Some(r) = sessions?.rows.first() {
        b.sessions = sessions_of(r);
    }
    if let Some(w) = win {
        let estimate = match &costed {
            Some((rolled, first)) => {
                let sizes = q.day_sizes(project, *first, now.date_naive()).await?;
                let mut e = crate::board_cost::cost(&w, f, rolled, &sizes, now);
                // It ran: confirmed, or under the threshold.
                e.confirm = false;
                Some(e)
            }
            None => None,
        };
        let mut window = w.window;
        window.estimate = estimate;
        if let Some((now_users, before_users, before_sessions)) = compared? {
            let n = |r: Option<&Map<String, Value>>| r.map_or(0, |r| int(r, "n"));
            window.current = Period {
                users: n(now_users.rows.first()),
                sessions: b.sessions.clone(),
            };
            window.previous = Period {
                users: n(before_users.rows.first()),
                sessions: before_sessions
                    .rows
                    .first()
                    .map(sessions_of)
                    .unwrap_or_default(),
            };
        }
        b.window = Some(window);
    }
    b.dau_series = dau?
        .rows
        .iter()
        .map(|r| (text(r, "date").unwrap_or_default(), int(r, "n")))
        .collect();
    b.vitals = vitals?
        .rows
        .iter()
        .filter_map(|r| {
            let name = text(r, "name")?;
            let p75 = float(r, "p75");
            Some(Vital {
                rating: rating(&name, p75).into(),
                name,
                p75,
                samples: int(r, "samples"),
            })
        })
        .collect();
    b.features = features?
        .rows
        .iter()
        .map(|r| FeatureUse {
            key: text(r, "key").unwrap_or_default(),
            name: None,
            uses: int(r, "uses"),
            users: int(r, "users"),
            completed: 0,
            completed_users: 0,
            user_days: int(r, "user_days"),
            declared: false,
        })
        .collect();
    b.role_features = role_features?
        .rows
        .iter()
        .filter_map(|r| Some((text(r, "role")?, text(r, "feature")?, int(r, "users"))))
        .collect();
    let success_rows = success?.rows;
    if let Some((m, version)) = map {
        b.map_version = Some(version);
        for decl in &m.features {
            let of = |col: &str| {
                decl.success.as_deref().map_or(0, |ev| {
                    success_rows
                        .iter()
                        .filter(|r| text(r, "event").as_deref() == Some(ev))
                        .map(|r| int(r, col))
                        .sum()
                })
            };
            let (completed, completed_users) = (of("n"), of("users"));
            match b.features.iter_mut().find(|x| x.key == decl.key) {
                Some(x) => {
                    x.name = Some(decl.name.clone());
                    x.completed = completed;
                    x.completed_users = completed_users;
                    x.declared = true;
                }
                None => b.features.push(FeatureUse {
                    key: decl.key.clone(),
                    name: Some(decl.name.clone()),
                    uses: 0,
                    users: 0,
                    completed,
                    completed_users,
                    user_days: 0,
                    declared: true,
                }),
            }
        }
    }
    for r in roles?.rows.iter() {
        let role = text(r, "role");
        let users = int(r, "users");
        match role.as_deref().and_then(|v| v.strip_prefix("unmatched:")) {
            Some(raw) => b.role_gaps.push((raw.to_owned(), users)),
            None => b.roles.push(RoleUsers { role, users }),
        }
    }
    if let Some(k) = kept {
        k.put_into(&mut b);
    }
    b.releases = releases?
        .rows
        .iter()
        .map(|r| ReleaseRow {
            release: text(r, "release").unwrap_or_default(),
            first_seen: text(r, "first_seen").unwrap_or_default(),
            users: int(r, "users"),
            sessions: int(r, "sessions"),
            error_rate: float(r, "error_rate"),
            avg_secs: float(r, "avg_secs"),
            bounce_rate: float(r, "bounce_rate"),
            tasks_per_session: float(r, "tasks_per_session"),
        })
        .collect();
    let stored = stored?;
    b.environments = stored.environments;
    let churn = map.map_or(14, |(m, _)| i64::from(m.churn_days()));
    let successes: Vec<(String, String)> = map
        .map(|(m, _)| {
            m.features
                .iter()
                .filter_map(|x| Some((x.key.clone(), x.success.clone()?)))
                .collect()
        })
        .unwrap_or_default();
    let funnel_defs: Vec<(String, Vec<String>)> = map
        .map(|(m, _)| {
            m.features
                .iter()
                .filter(|x| !x.steps.is_empty())
                .map(|x| {
                    let mut steps = x.steps.clone();
                    if let Some(s) = &x.success
                        && steps.last() != Some(s)
                    {
                        steps.push(s.clone());
                    }
                    (x.key.clone(), steps)
                })
                .collect()
        })
        .unwrap_or_default();
    let ((mut cards, readiness), journeys, cohorts, funnels, task_times, resurrected, retention) =
        futures::try_join!(
            insights::compute(q, project, &card_rows, f, ident, churn, &successes),
            insights::journeys(q, project, &success_reads, &journey_rows, f, &successes),
            insights::flag_cohorts(q, project, &src, f),
            insights::funnels(q, project, &src, f, &funnel_defs),
            tasks::task_times(q, project, map.map(|(m, _)| m), f, &successes),
            tasks::resurrected(q, project, &p90, f, churn),
            tasks::retention(q, project, &p90, f, ident),
        )?;
    b.retention = retention;
    b.task_times = task_times;
    b.users.resurrected = resurrected;
    cards.extend(insights::funnel_cards(&funnels));
    b.funnels = funnels;
    b.journeys = journeys;
    cards.extend(insights::flag_cards(&cohorts));
    b.flag_cohorts = cohorts;
    if b.vitals
        .iter()
        .any(|v| v.rating != "good" && vitals_guide(&v.name).is_some_and(|g| g.pages))
    {
        let (pages, attr) = futures::join!(
            vitals::pages(q, project, &src, f),
            vital_parts::attribution(q, project, &src, f)
        );
        b.vitals_why = vitals::explain(&b.vitals, &pages?, &attr?, true);
    } else {
        b.vitals_why = vitals::explain(&b.vitals, &[], &vital_parts::Attribution::default(), true);
    }
    if let Some(c) = insights::slow_pages(&b.vitals, &b.vitals_why) {
        cards.push(c);
        cards.sort_by(|a, b| b.score.total_cmp(&a.score));
    }
    cards.truncate(7);
    b.insights = cards;
    b.readiness = readiness;
    b.total_events = i64::try_from(stored.rows).unwrap_or(i64::MAX);
    Ok(b)
}

/// The share of the query pool (D-033) the rollup rows one board holds
/// in memory may take together (D-072).
const HELD_SHARE: usize = 4;

/// `q`, with the rows of the closed `days` of each rollup table held in
/// memory (#452, D-072): the board's environment, every column. Each
/// block reads its days of a rollup (`date IN (…)`, a subset of its
/// table's `days`, D-040) and filters the environment (`environment =
/// $env`, [`board_people::split_span`]), so it finds the same rows in
/// memory as in the files. A table that does not fit is read from the
/// files by every query, as before.
async fn hold_rollups(
    q: &QueryEngine,
    project: &str,
    tables: &[(&'static str, &Vec<chrono::NaiveDate>)],
) -> QueryEngine {
    let sql: Vec<(&'static str, String)> = tables
        .iter()
        .filter(|(_, days)| !days.is_empty())
        .map(|(name, days)| {
            let list = days
                .iter()
                .map(|d| format!("'{}'", d.format("%Y-%m-%d")))
                .collect::<Vec<_>>()
                .join(", ");
            (
                *name,
                format!("SELECT * FROM {name} WHERE date IN ({list}) AND environment = $env"),
            )
        })
        .collect();
    if sql.is_empty() {
        return q.clone();
    }
    q.holding(project, sql, q.memory() / HELD_SHARE).await.0
}

/// The sessions block from its row.
fn sessions_of(r: &Map<String, Value>) -> Sessions {
    let user_days = int(r, "user_days").max(1) as f64;
    Sessions {
        count: int(r, "sessions"),
        avg_secs: float(r, "avg_secs"),
        bounce_rate: float(r, "bounce_rate"),
        error_rate: float(r, "error_rate"),
        tasks_per_user_day: int(r, "tasks") as f64 / user_days,
    }
}
