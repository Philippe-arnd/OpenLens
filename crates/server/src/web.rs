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
        .route("/assets/app.css", get(css))
}

/// Strict headers for every HTML page: no scripts at all, no framing.
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
