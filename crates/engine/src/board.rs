//! The board (#452): its snapshot, the numbers of every block, and the
//! filter bar's options.

use super::*;

/// Largest 90-day window the board keeps in memory; beyond it, each query
/// reads the files. The snapshot holds only the columns the board reads,
/// and no `properties` (#449), so a row costs less than 160 bytes: 4 M rows
/// stay under ~650 MB (#190). The snapshot also reserves its bytes in the
/// query memory pool (768 MB, shared with every query): when the pool
/// cannot hold it, the board reads the files instead (#329).
const BOARD_SNAPSHOT_ROWS: usize = 4_000_000;

/// The columns of the board snapshot. `uuid`, `url`, `received_at`,
/// `account` and `plan` are not read by any board query, and `properties`
/// (most of an event's bytes) by none since Web Vitals have their own
/// columns and snapshot (#449).
const BOARD_COLUMNS: &str = "event, distinct_id, session_id, timestamp, path, feature, role, \
     environment, release, flags, date";

/// The columns of the board's Web Vitals snapshot: what the 7-day filters
/// read, the page, and the typed facts.
fn vital_snapshot_columns() -> String {
    format!(
        "event, distinct_id, timestamp, path, role, environment, release, date, {VITAL_COLUMNS}"
    )
}

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
    board_with(q, project, f, map, None).await
}

/// [`board`], sharing its 90-day snapshot with the boards computed at the
/// same time for other roles or releases of the same data `generation`
/// (#418): four filtered boards build one snapshot, not four.
pub async fn board_shared(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
    generation: u64,
) -> Result<Board> {
    board_with(q, project, f, map, Some(generation)).await
}

async fn board_with(
    q: &QueryEngine,
    project: &str,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
    share: Option<u64>,
) -> Result<Board> {
    let q = &q.with_params(&f.params()?);
    // Every board query reads the same projection (roles and features
    // resolved from the map) over at most 90 days: compute it once, in
    // memory, unless it is too large, then each query only filters it.
    let window = Filters {
        role: None,
        release: None,
        ..f.clone()
    }
    .clause(90)?;
    let projected = source(map.map(|(m, _)| m));
    let snap_sql = format!("SELECT {BOARD_COLUMNS} FROM {projected} WHERE {window}");
    let snap = match share {
        Some(g) => {
            q.shared_snapshot(project, "board_events", &snap_sql, BOARD_SNAPSHOT_ROWS, g)
                .await?
        }
        None => {
            q.snapshot(project, "board_events", &snap_sql, BOARD_SNAPSHOT_ROWS)
                .await?
        }
    };
    let (q, src) = match &snap {
        Some(s) => (s, "board_events".to_owned()),
        None => (q, projected.clone()),
    };
    // Web Vitals look at 7 days of `$vital` rows only: their own small
    // snapshot of the typed columns, so the 90-day one carries no
    // `properties` (#449).
    let vital_window = Filters {
        role: None,
        release: None,
        ..f.clone()
    }
    .clause(7)?;
    let vital_sql = format!(
        "SELECT {} FROM {projected} WHERE {vital_window} AND event = '$vital'",
        vital_snapshot_columns()
    );
    let vsnap = match share {
        Some(g) => {
            q.shared_snapshot(project, "board_vitals", &vital_sql, BOARD_SNAPSHOT_ROWS, g)
                .await?
        }
        None => {
            q.snapshot(project, "board_vitals", &vital_sql, BOARD_SNAPSHOT_ROWS)
                .await?
        }
    };
    let (q, vsrc) = match &vsnap {
        Some(s) => (s, "board_vitals".to_owned()),
        None => (q, projected),
    };
    let ident = identified(any_person(q, project).await?);
    // The people blocks read `person_day` for the closed days rolled up
    // with the map in force, the events for the rest (#492, D-040).
    // The sessions block and the releases table do the same with
    // `session_day` (#498, D-047).
    let now = chrono::Utc::now();
    let version = map.map(|(_, v)| v);
    let (rolled_people, rolled_sessions) = futures::try_join!(
        board_people::rolled_days(q, project, "person_day", version, now),
        board_people::rolled_days(q, project, "session_day", version, now),
    )?;
    let role = map.map_or_else(|| "role".to_owned(), |(m, _)| lookups(m).0);
    let people = board_people::People {
        src: &src,
        role: role.clone(),
        rolled: rolled_people,
        now,
    };
    let any_role = Filters {
        role: None,
        ..f.clone()
    };
    let (p30, p30_roles, p90) = (
        people.rows(f, 30)?,
        people.rows(&any_role, 30)?,
        people.rows(f, 90)?,
    );
    // Rollup days are UTC days: a block grouped by the project's day reads
    // them only where the two agree (D-017, D-040).
    let utc_days = |days: i64| {
        board_people::utc_days(
            &f.tz,
            (now - Duration::days(days)).date_naive(),
            now.date_naive(),
        )
    };
    let w30 = f.clause(30)?;
    let w14 = f.clause(14)?;
    // Days are the project's days (D-017).
    let day = f.day()?;
    let w7 = f.clause(7)?;

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
    let sessions_sql = sessions.sessions_sql(f)?;
    let dau_sql = if utc_days(14) {
        format!(
            "SELECT day AS date, count(DISTINCT distinct_id) AS n FROM {} WHERE {ident} GROUP BY 1 ORDER BY 1",
            people.rows(f, 14)?
        )
    } else {
        format!(
            "SELECT {day} AS date, count(DISTINCT distinct_id) AS n FROM {src} WHERE {w14} AND {ident} GROUP BY 1 ORDER BY 1"
        )
    };
    let vitals_sql = vitals::p75_sql(&vsrc, &w7);
    let features_sql = if utc_days(30) {
        format!(
            "SELECT feature AS key, sum(events) AS uses, count(DISTINCT distinct_id) AS users, \
             count(DISTINCT concat(distinct_id, '|', day)) AS user_days \
             FROM {p30} WHERE feature IS NOT NULL GROUP BY feature ORDER BY users DESC, uses DESC LIMIT 12"
        )
    } else {
        format!(
            "SELECT feature AS key, count(*) AS uses, count(DISTINCT distinct_id) AS users, \
             count(DISTINCT concat(distinct_id, '|', {day})) AS user_days \
             FROM {src} WHERE {w30} AND feature IS NOT NULL GROUP BY feature ORDER BY users DESC, uses DESC LIMIT 12"
        )
    };
    let roles_sql = format!(
        "SELECT role, count(DISTINCT distinct_id) AS users FROM {p30_roles} \
         GROUP BY role ORDER BY users DESC LIMIT 12"
    );
    let role_features_sql = format!(
        "SELECT role, feature, count(DISTINCT distinct_id) AS users FROM {p30_roles} \
         WHERE role IS NOT NULL AND feature IS NOT NULL GROUP BY role, feature"
    );
    let releases_sql = sessions.releases_sql(f)?;
    // Neither depends on who did what: they come from the segments, which
    // are read once (#447), not from a query.

    let success_sql = if successes.is_empty() {
        "SELECT CAST(NULL AS VARCHAR) AS event, 0 AS n, 0 AS users WHERE false".to_owned()
    } else {
        format!(
            "SELECT event, count(*) AS n, count(DISTINCT distinct_id) AS users FROM {src} WHERE {w30} AND event IN ({success_list}) GROUP BY event"
        )
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
    ) = futures::join!(
        q.sql(project, &users_sql),
        q.sql(project, &sessions_sql),
        q.sql(project, &dau_sql),
        q.sql(project, &vitals_sql),
        q.sql(project, &features_sql),
        q.sql(project, &roles_sql),
        q.sql(project, &releases_sql),
        q.raw_summary(project),
        q.sql(project, &success_sql),
        q.sql(project, &role_features_sql),
        q.sql(project, &anonymous_sql),
        q.sql(project, &known_sql),
    );

    let mut b = Board {
        filters: Some(f.clone()),
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
        let user_days = int(r, "user_days").max(1) as f64;
        b.sessions = Sessions {
            count: int(r, "sessions"),
            avg_secs: float(r, "avg_secs"),
            bounce_rate: float(r, "bounce_rate"),
            error_rate: float(r, "error_rate"),
            tasks_per_user_day: int(r, "tasks") as f64 / user_days,
        };
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
            insights::compute(q, project, &src, f, ident, churn, &successes),
            insights::journeys(q, project, &src, f, &successes),
            insights::flag_cohorts(q, project, &src, f),
            insights::funnels(q, project, &src, f, &funnel_defs),
            tasks::task_times(q, project, &src, f, &successes),
            tasks::resurrected(q, project, &src, f, churn),
            tasks::retention(q, project, &src, f, ident),
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
            vitals::pages(q, project, &vsrc, f),
            vital_parts::attribution(q, project, &vsrc, f)
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
