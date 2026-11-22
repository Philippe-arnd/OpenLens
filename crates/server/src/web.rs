//! Dashboard routes (server-rendered, D-011).

use axum::Router;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use maud::Markup;
use serde::Deserialize;

use crate::api::FilterQuery;
use crate::{AppState, auth};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        .route("/login", get(login_page).post(login))
        .route("/logout", post(logout))
        .route("/l/{token}", get(login_link))
        .route("/p/{project}", get(board))
        .route("/p/{project}/events", get(events))
        .route("/p/{project}/errors", get(errors_page))
        .route("/p/{project}/pages", get(pages_page))
        .route("/p/{project}/replays", get(replays_page))
        .route("/p/{project}/replays/{session}", get(player_page))
        .route(
            "/p/{project}/replays/{session}/events.json",
            get(replay_events),
        )
        .route("/p/{project}/watch", get(watch_churned))
        .route("/assets/player.js", get(player_js))
        .route("/assets/player.css", get(player_css))
        .route("/assets/app.css", get(css))
        .route("/theme/{mode}", get(set_theme))
}

/// Strict headers for every HTML page: no scripts at all, no framing.
pub(crate) fn html_page(status: StatusCode, m: Markup) -> Response {
    html(status, m)
}

fn html(status: StatusCode, m: Markup) -> Response {
    let mut r = (status, m).into_response();
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'self'; img-src 'self' data:; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

fn to_login() -> Response {
    Redirect::to("/login").into_response()
}

fn with_cookie(mut r: Response, cookie: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(cookie) {
        r.headers_mut().append(header::SET_COOKIE, v);
    }
    r
}

async fn css() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        openlens_ui::CSS,
    )
        .into_response()
}

async fn home(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !auth::has_session(&state, &headers) {
        return to_login();
    }
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || meta.projects()).await {
        Ok(Ok(ps)) => match ps.first() {
            Some(p) => Redirect::to(&format!("/p/{}", p.id)).into_response(),
            None => html(
                StatusCode::OK,
                openlens_ui::message("No project", "Ask your agent to create one."),
            ),
        },
        _ => html(
            StatusCode::INTERNAL_SERVER_ERROR,
            openlens_ui::message("Oops", "The metadata store is unavailable."),
        ),
    }
}

async fn login_page() -> Response {
    html(StatusCode::OK, openlens_ui::login(false))
}

#[derive(Deserialize)]
struct LoginForm {
    token: String,
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<LoginForm>,
) -> Response {
    if crate::token_matches(f.token.trim(), &state.admin_token) {
        let c = auth::set_cookie(&auth::new_session(&state), &headers, auth::SESSION_SECS);
        return with_cookie(Redirect::to("/").into_response(), &c);
    }
    // Slow down guessing.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    html(StatusCode::UNAUTHORIZED, openlens_ui::login(true))
}

async fn logout(headers: HeaderMap) -> Response {
    with_cookie(
        Redirect::to("/login").into_response(),
        &auth::set_cookie("", &headers, 0),
    )
}

async fn login_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    let meta = state.meta.clone();
    let ok = matches!(
        tokio::task::spawn_blocking(move || meta.consume_login_link(&token)).await,
        Ok(Ok(true))
    );
    if !ok {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        return html(
            StatusCode::UNAUTHORIZED,
            openlens_ui::message(
                "Link expired",
                "This login link was already used or has expired. Ask your agent for a new one.",
            ),
        );
    }
    let c = auth::set_cookie(&auth::new_session(&state), &headers, auth::SESSION_SECS);
    with_cookie(Redirect::to("/").into_response(), &c)
}

async fn project(state: &AppState, id: String) -> Option<openlens_meta::Project> {
    let meta = state.meta.clone();
    tokio::task::spawn_blocking(move || meta.project(&id))
        .await
        .ok()?
        .ok()?
}

async fn board(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<FilterQuery>,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return to_login();
    }
    let Some(p) = project(&state, id).await else {
        return html(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let f = q.filters();
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let map = crate::load_map(&state, &p.id).await;
    match openlens_engine::board(&state.query, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))).await
    {
        Ok(b) => html(
            StatusCode::OK,
            openlens_ui::board(&info, &b, &f, &crate::base_url(&headers)),
        ),
        Err(e) => html(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e.to_string()),
        ),
    }
}

async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<FilterQuery>,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return to_login();
    }
    let Some(p) = project(&state, id).await else {
        return html(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let f = q.filters();
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let map = crate::load_map(&state, &p.id).await;
    let (b, rows) = futures_join(
        &state,
        &p.id,
        &f,
        map.as_ref().map(|(m, v)| (m, *v)),
        q.event.as_deref(),
    )
    .await;
    match (b, rows) {
        (Ok(b), Ok(rows)) => html(StatusCode::OK, openlens_ui::events(&info, &b, &rows, &f)),
        (Err(e), _) | (_, Err(e)) => html(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e.to_string()),
        ),
    }
}

async fn futures_join(
    state: &AppState,
    project: &str,
    f: &openlens_engine::Filters,
    map: Option<(&openlens_featuremap::FeatureMap, i64)>,
    event: Option<&str>,
) -> (
    openlens_engine::Result<openlens_engine::Board>,
    openlens_engine::Result<Vec<openlens_engine::EventRow>>,
) {
    tokio::join!(
        openlens_engine::board(&state.query, project, f, map),
        openlens_engine::recent_events(&state.query, project, f, map.map(|(m, _)| m), event, 100)
    )
}

/// `GET /theme/{auto|light|dark}`: remember the theme (cookie, 1 year) and
/// go back to the page the user came from (same-origin paths only).
async fn set_theme(headers: HeaderMap, Path(mode): Path<String>) -> Response {
    let mode = match mode.as_str() {
        "light" | "dark" => mode,
        _ => "auto".to_owned(),
    };
    let base = crate::base_url(&headers);
    let back = headers
        .get(header::REFERER)
        .and_then(|v| v.to_str().ok())
        .and_then(|r| r.strip_prefix(base.as_str()))
        .filter(|p| p.starts_with('/') && !p.starts_with("//"))
        .unwrap_or("/")
        .to_owned();
    let cookie = format!("ol_theme={mode}; Path=/; SameSite=Lax; Max-Age=31536000");
    with_cookie(Redirect::to(&back).into_response(), &cookie)
}

/// Apply the theme cookie to HTML pages (`<html data-theme="…">`), so the
/// page renders in the right theme with no script and no flash.
pub(crate) async fn theme_layer(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let theme = req
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == "ol_theme")
        .map(|(_, v)| v)
        .filter(|v| *v == "light" || *v == "dark")
        .map(str::to_owned);
    let resp = next.run(req).await;
    let Some(theme) = theme else { return resp };
    let is_html = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    if !is_html {
        return resp;
    }
    let (mut parts, body) = resp.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 8 * 1024 * 1024).await else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "page too large").into_response();
    };
    let html = String::from_utf8_lossy(&bytes).replacen(
        "<html lang=\"en\">",
        &format!("<html lang=\"en\" data-theme=\"{theme}\">"),
        1,
    );
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, axum::body::Body::from(html))
}

async fn errors_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<FilterQuery>,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return to_login();
    }
    let Some(p) = project(&state, id).await else {
        return html(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let f = q.filters();
    let map = crate::load_map(&state, &p.id).await;
    let src = openlens_engine::source(map.as_ref().map(|(m, _)| m));
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let (b, groups) = tokio::join!(
        openlens_engine::board(&state.query, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))),
        openlens_engine::error_groups(&state.query, &p.id, &src, &f)
    );
    match (b, groups) {
        (Ok(b), Ok(g)) => html(StatusCode::OK, openlens_ui::errors(&info, &b, &g, &f)),
        (Err(e), _) | (_, Err(e)) => html(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e.to_string()),
        ),
    }
}

async fn pages_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<FilterQuery>,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return to_login();
    }
    let Some(p) = project(&state, id).await else {
        return html(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let f = q.filters();
    let map = crate::load_map(&state, &p.id).await;
    let src = openlens_engine::source(map.as_ref().map(|(m, _)| m));
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let (b, list) = tokio::join!(
        openlens_engine::board(&state.query, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))),
        openlens_engine::pages(&state.query, &p.id, &src, &f)
    );
    match (b, list) {
        (Ok(b), Ok(l)) => html(StatusCode::OK, openlens_ui::pages(&info, &b, &l, &f)),
        (Err(e), _) | (_, Err(e)) => html(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e.to_string()),
        ),
    }
}

/// The replay player (rrweb Replayer, MIT) and its stylesheet.
const PLAYER_JS: &str = include_str!("../../../sdk/js/dist/openlens-player.min.js");
const PLAYER_CSS: &str = include_str!("../../../sdk/js/dist/openlens-player.min.css");

async fn player_js() -> Response {
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

async fn player_css() -> Response {
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
struct ReplayQuery {
    u: Option<String>,
    at: Option<String>,
}

async fn replays_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ReplayQuery>,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return to_login();
    }
    let Some(p) = project(&state, id).await else {
        return html(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let root = state.data_dir.clone();
    let pid = p.id.clone();
    let who = q.u.clone();
    let list =
        tokio::task::spawn_blocking(move || openlens_replay::list(&root, &pid, who.as_deref()))
            .await;
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    match list {
        Ok(Ok(l)) => html(
            StatusCode::OK,
            openlens_ui::replays(&info, &l, q.u.as_deref()),
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
fn player_csp(mut r: Response) -> Response {
    r.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'self' 'unsafe-inline' https:; img-src * data: blob:; font-src https: data:; media-src https: blob:; frame-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    r
}

async fn player_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, session)): Path<(String, String)>,
    Query(q): Query<ReplayQuery>,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return to_login();
    }
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
    player_csp(html(
        StatusCode::OK,
        openlens_ui::player(&info, &session, Some(&who), at.as_deref()),
    ))
}

async fn replay_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, session)): Path<(String, String)>,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
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

#[derive(Deserialize)]
struct WatchQuery {
    churned_at: String,
}

/// From a churn-point card: open the replay of the latest recorded session
/// of a user who churned at that place, at the moment they left.
async fn watch_churned(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<WatchQuery>,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return to_login();
    }
    let Some(p) = project(&state, id).await else {
        return html(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let map = crate::load_map(&state, &p.id).await;
    let f = openlens_engine::Filters::default();
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
