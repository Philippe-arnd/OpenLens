//! The replay list, the player and its assets. Moved out of web.rs as is (#307).

use super::*;

/// The replay player (rrweb Replayer, MIT) and its stylesheet.
pub(super) const PLAYER_JS: &str = include_str!("../../../../sdk/js/dist/openlens-player.min.js");
pub(super) const PLAYER_CSS: &str = include_str!("../../../../sdk/js/dist/openlens-player.min.css");

pub(super) async fn player_js() -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=3600"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        PLAYER_JS,
    )
        .into_response()
}

pub(super) async fn player_css() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        PLAYER_CSS,
    )
        .into_response()
}

#[derive(Deserialize)]
pub(super) struct ReplayQuery {
    u: Option<String>,
    at: Option<String>,
    /// Playback position to open at, milliseconds (comment links).
    t: Option<u64>,
    /// List order: `active` for most active first (#404).
    sort: Option<String>,
    /// List filter: more than this many minutes of active time (#404).
    active: Option<String>,
}

pub(super) async fn replays_page(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ReplayQuery>,
) -> Response {
    let Some(p) = project(&state, id).await else {
        return html(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let root = state.data_dir.clone();
    let pid = p.id.clone();
    let who = q.u.clone();
    let view = openlens_replay::View::from_query(q.sort.as_deref(), q.active.as_deref());
    let (list, rate) = tokio::join!(
        tokio::task::spawn_blocking(move || {
            openlens_replay::list_view(&root, &pid, who.as_deref(), view)
        }),
        crate::replay::rate(&state, &p.id, &p.timezone)
    );
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    match list {
        Ok(Ok(l)) => html(
            StatusCode::OK,
            openlens_ui::replays(
                &info,
                &l,
                q.u.as_deref(),
                view,
                &auth::tz_for(&state, &headers, &p.timezone),
                &state.meta.replay_comment_counts(&p.id).unwrap_or_default(),
                &rate,
            ),
        ),
        _ => html(
            StatusCode::INTERNAL_SERVER_ERROR,
            openlens_ui::message("Can't show that", "Recordings cannot be read right now."),
        ),
    }
}

/// The player page is the only dashboard page that runs a script: its own
/// (`script-src 'self'`). Recorded pages are rebuilt in rrweb's sandboxed
/// iframe (no scripts) and may reference their own images, styles and fonts.
pub(super) fn player_csp(mut r: Response) -> Response {
    r.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'self' 'unsafe-inline' https:; img-src * data: blob:; font-src https: data:; media-src https: blob:; frame-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    r
}

pub(super) async fn player_page(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, session)): Path<(String, String)>,
    Query(q): Query<ReplayQuery>,
) -> Response {
    let Some(p) = project(&state, id).await else {
        return html(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let not_found = || {
        html(
            StatusCode::NOT_FOUND,
            openlens_ui::message(
                "No recording",
                "This session has no recording (replay was off, consent was not given, or it was not kept).",
            ),
        )
    };
    if !openlens_replay::valid_id(&session) {
        return not_found();
    }
    let root = state.data_dir.clone();
    let (pid, sid) = (p.id.clone(), session.clone());
    let owner = tokio::task::spawn_blocking(move || {
        openlens_replay::list(&root, &pid, None).map(|l| {
            l.into_iter()
                .find(|r| r.session_id == sid)
                .map(|r| r.distinct_id)
        })
    })
    .await;
    let Ok(Ok(Some(who))) = owner else {
        return not_found();
    };
    let at = q.at.filter(|a| {
        a.len() <= 40
            && a.chars()
                .all(|c| c.is_ascii_digit() || " -:.TZ".contains(c))
    });
    let viewer = auth::viewer(&state, &headers);
    let meta = state.meta.clone();
    let (pid, sid) = (p.id.clone(), session.clone());
    let comments = tokio::task::spawn_blocking(move || meta.replay_comments(&pid, &sid))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let me = viewer.as_ref().map(|v| (v.id.as_str(), v.admin));
    player_csp(html(
        StatusCode::OK,
        openlens_ui::player(
            &info,
            &session,
            Some(&who),
            at.as_deref(),
            q.t,
            &comments,
            me,
        ),
    ))
}

pub(super) async fn replay_events(
    _: crate::access::SessionData,
    State(state): State<AppState>,
    Path((id, session)): Path<(String, String)>,
) -> Response {
    match crate::replay::load(&state, id, session).await {
        Ok(Some((_, events))) => (
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            format!("{{\"events\":{events}}}"),
        )
            .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The session's OpenLens events (page views, clicks, submits, errors,
/// success and custom events), for the markers on the player's timeline.
pub(super) async fn replay_timeline(
    _: crate::access::SessionData,
    State(state): State<AppState>,
    Path((id, session)): Path<(String, String)>,
) -> Response {
    if !openlens_replay::valid_id(&session) || project(&state, id.clone()).await.is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let sql = "SELECT event, CAST(date_part('epoch', timestamp) * 1000 AS BIGINT) AS ms, feature, path, \
         regexp_match(properties, '\"\\$message\":\"([^\"]{0,120})')[1] AS message, \
         properties LIKE '%\"$rage\":true%' AS rage \
         FROM events WHERE session_id = $session \
         AND event NOT IN ('$scroll', '$vital', '$identify') ORDER BY timestamp LIMIT 1000";
    let mut params = openlens_sqlgen::Params::new();
    if params.set("session", &session).is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    match state.query.with_params(&params).sql(&id, sql).await {
        Ok(r) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({ "events": r.rows })),
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct WatchQuery {
    churned_at: String,
}

/// From a churn-point card: open the replay of the latest recorded session
/// of a user who churned at that place, at the moment they left.
pub(super) async fn watch_churned(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<WatchQuery>,
) -> Response {
    let Some(p) = project(&state, id).await else {
        return html(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let map = crate::load_map(&state, &p.id).await;
    let f = openlens_engine::Filters {
        tz: auth::tz_for(&state, &headers, &p.timezone),
        ..openlens_engine::Filters::default()
    };
    let (sessions, recorded) = tokio::join!(
        openlens_engine::churned_sessions(
            &state.query,
            &p.id,
            map.as_ref().map(|(m, _)| m),
            &f,
            &q.churned_at
        ),
        {
            let root = state.data_dir.clone();
            let pid = p.id.clone();
            tokio::task::spawn_blocking(move || openlens_replay::list(&root, &pid, None))
        }
    );
    let recorded: std::collections::HashSet<String> = match recorded {
        Ok(Ok(l)) => l.into_iter().map(|r| r.session_id).collect(),
        _ => Default::default(),
    };
    let hit = sessions
        .unwrap_or_default()
        .into_iter()
        .find(|(s, _)| recorded.contains(s));
    match hit {
        Some((s, ts)) => Redirect::to(&format!(
            "/p/{}/replays/{s}?at={}",
            p.id,
            openlens_ui::url_enc(&ts)
        ))
        .into_response(),
        None => html(
            StatusCode::OK,
            openlens_ui::message(
                "No recording yet",
                "None of the sessions that churned there was recorded. Replays fill in as new sessions come in.",
            ),
        ),
    }
}
