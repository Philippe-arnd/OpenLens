//! Dashboard routes (server-rendered, D-011).

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use maud::Markup;
use serde::Deserialize;
use serde_json::json;

use crate::api::FilterQuery;
use crate::{AppState, auth};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        .route("/login", get(login_page).post(login))
        .route("/login/email", post(login_email))
        .route(
            "/projects",
            get(projects_page).post(crate::settings::create),
        )
        .route("/welcome", get(welcome_page).post(welcome_create))
        .route("/users", get(crate::users::page).post(crate::users::invite))
        .route(
            "/users/{id}",
            get(crate::users::edit_page).post(crate::users::update),
        )
        .route("/users/{id}/{action}", post(crate::users::action))
        .route(
            "/me",
            get(crate::users::me_page).post(crate::users::me_save),
        )
        .route("/logout", post(logout))
        .route("/l/{token}", get(login_link))
        .route("/p/{project}", get(board))
        .route("/p/{project}/events", get(events))
        .route("/p/{project}/errors", get(errors_page))
        .route("/p/{project}/pages", get(pages_page))
        .route("/p/{project}/replays", get(replays_page))
        .route("/p/{project}/replays/{session}", get(player_page))
        .route(
            "/p/{project}/replays/{session}/comments",
            post(crate::comments::add),
        )
        .route(
            "/p/{project}/replays/{session}/comments/{id}/delete",
            post(crate::comments::delete),
        )
        .route(
            "/p/{project}/replays/{session}/timeline.json",
            get(replay_timeline),
        )
        .route(
            "/p/{project}/replays/{session}/events.json",
            get(replay_events),
        )
        .route("/p/{project}/watch", get(watch_churned))
        .route("/p/{project}/heatmap", get(heatmap_page))
        .route("/p/{project}/icon", get(crate::icon::icon))
        .route("/settings/storage", get(crate::storage::page))
        .route("/settings/storage/s3", post(crate::storage::s3))
        .route("/settings/storage/backups", post(crate::storage::plan))
        .route(
            "/settings/storage/backups/run",
            post(crate::storage::run_now),
        )
        .route(
            "/settings/storage/backups/latest.tar.gz",
            get(crate::storage::latest),
        )
        .route(
            "/p/{project}/features",
            get(crate::features::page).post(crate::features::edit),
        )
        .route(
            "/p/{project}/features/{file}",
            get(crate::features::download),
        )
        .route("/p/{project}/people/{who}", get(crate::explore::person))
        .route("/p/{project}/breakdown", get(crate::explore::breakdown))
        .route("/p/{project}/audience", get(crate::explore::audience))
        .route("/p/{project}/flows", get(crate::explore::flows))
        .route(
            "/p/{project}/sql",
            get(crate::explore::sql_page).post(crate::explore::sql_run),
        )
        .route(
            "/p/{project}/flags-edit",
            get(crate::manage::flag_page).post(crate::manage::flag_save),
        )
        .route(
            "/p/{project}/flags/{key}/delete",
            post(crate::manage::flag_delete),
        )
        .route(
            "/p/{project}/surveys-edit",
            get(crate::manage::survey_page).post(crate::manage::survey_save),
        )
        .route(
            "/p/{project}/surveys/{key}/delete",
            post(crate::manage::survey_delete),
        )
        .route(
            "/p/{project}/settings",
            get(crate::settings::page).post(crate::settings::save),
        )
        .route(
            "/p/{project}/settings/rotate-key",
            post(crate::settings::rotate),
        )
        .route(
            "/p/{project}/settings/delete",
            post(crate::settings::delete),
        )
        .route("/p/{project}/export", get(crate::settings::export))
        .route("/p/{project}/digest", get(digest_page))
        .route("/p/{project}/heatmap.json", get(heatmap_json))
        .route("/p/{project}/heatmap/overlay", get(heatmap_overlay))
        .route("/assets/player.js", get(player_js))
        .route("/assets/player.css", get(player_css))
        .route("/assets/app.css", get(css))
        .route("/assets/favicon.svg", get(favicon))
        .route("/favicon.ico", get(favicon))
        .route("/theme/{mode}", get(set_theme))
        .route("/nav/{mode}", get(set_nav))
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

async fn favicon() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        openlens_ui::FAVICON,
    )
        .into_response()
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
    let viewer = auth::viewer(&state, &headers);
    let visible = tokio::task::spawn_blocking(move || meta.projects())
        .await
        .map(|r| {
            r.map(|ps| {
                ps.into_iter()
                    .filter(|p| viewer.as_ref().is_some_and(|v| v.can_see(&p.id)))
                    .collect::<Vec<_>>()
            })
        });
    match visible {
        Ok(Ok(ps)) if ps.len() > 1 => Redirect::to("/projects").into_response(),
        Ok(Ok(ps)) => match ps.first() {
            Some(p) => Redirect::to(&format!("/p/{}", p.id)).into_response(),
            None => html(
                StatusCode::OK,
                openlens_ui::message(
                    "No project yet",
                    "Your coding agent creates one for each app and wires it in: give it this server's guide, /agents.md (it starts with openlens-cli init --name <your app>).",
                ),
            ),
        },
        _ => html(
            StatusCode::INTERNAL_SERVER_ERROR,
            openlens_ui::message("Oops", "The metadata store is unavailable."),
        ),
    }
}

async fn login_page() -> Response {
    html(
        StatusCode::OK,
        openlens_ui::login(false, crate::oidc::enabled(), email_login().is_some()),
    )
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
        let c = auth::set_cookie(
            &auth::new_session(&state, auth::OWNER),
            &headers,
            auth::SESSION_SECS,
        );
        let to = after_owner_login(&state).await;
        return with_cookie(Redirect::to(to).into_response(), &c);
    }
    // Slow down guessing.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    html(
        StatusCode::UNAUTHORIZED,
        openlens_ui::login(true, crate::oidc::enabled(), email_login().is_some()),
    )
}

/// Sign-in links by email: needs SMTP and the public URL, which the link
/// is built from (never from the request's Host header, which a caller
/// controls). Returns (SMTP URL, sender, public URL).
fn email_login() -> Option<(String, String, String)> {
    let (smtp, from) = crate::digest::login_mail()?;
    let base = std::env::var("OPENLENS_PUBLIC_URL")
        .or_else(|_| std::env::var("SERVICE_URL_OPENLENS"))
        .ok()
        .filter(|u| u.starts_with("https://") || u.starts_with("http://localhost"))?;
    Some((smtp, from, base.trim_end_matches('/').to_owned()))
}

/// At most one email per address every 10 minutes, and 30 an hour in all.
fn email_allowed(email: &str) -> bool {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    type Sent = (Vec<Instant>, std::collections::HashMap<String, Instant>);
    static SENT: OnceLock<Mutex<Sent>> = OnceLock::new();
    let Ok(mut s) = SENT.get_or_init(Mutex::default).lock() else {
        return false;
    };
    let now = Instant::now();
    s.0.retain(|t| now.duration_since(*t) < Duration::from_secs(3600));
    s.1.retain(|_, t| now.duration_since(*t) < Duration::from_secs(600));
    if s.0.len() >= 30 || s.1.contains_key(email) {
        return false;
    }
    s.0.push(now);
    s.1.insert(email.to_owned(), now);
    true
}

#[derive(Deserialize)]
struct EmailForm {
    email: String,
}

/// "Email me a login link": the same answer whether the address has an
/// account or not, and the email goes out in the background.
async fn login_email(State(state): State<AppState>, Form(f): Form<EmailForm>) -> Response {
    let done = html(
        StatusCode::OK,
        openlens_ui::message(
            "Check your inbox",
            "If this address was invited, a login link is on its way. It works once, within 15 minutes.",
        ),
    );
    let Some((smtp, from, base)) = email_login() else {
        return done;
    };
    let email = f.email.trim().to_lowercase();
    if email.len() > 200 || !email.contains('@') || !email_allowed(&email) {
        return done;
    }
    tokio::spawn(async move {
        let meta = state.meta.clone();
        let e = email.clone();
        let link = tokio::task::spawn_blocking(move || {
            let u = meta.user_by_email(&e).ok()??;
            if u.disabled {
                return None;
            }
            meta.create_login_link_for(Some(&u.id), 15 * 60).ok()
        })
        .await
        .ok()
        .flatten();
        if let Some(token) = link {
            let body = format!(
                "Here is your OpenLens login link. It works once, within 15 minutes:\n\n{base}/l/{token}\n\nIf you did not ask for it, ignore this email.\n"
            );
            if let Err(err) =
                crate::digest::send_mail(&smtp, &from, &[email], "Your OpenLens login link", &body)
                    .await
            {
                tracing::warn!(%err, "login email failed");
            }
        }
    });
    done
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
    let who = match tokio::task::spawn_blocking(move || meta.consume_login_link(&token)).await {
        Ok(Ok(Some(user))) => Some(user.unwrap_or_else(|| auth::OWNER.to_owned())),
        _ => None,
    };
    let Some(who) = who else {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        return html(
            StatusCode::UNAUTHORIZED,
            openlens_ui::message(
                "Link expired",
                "This login link was already used or has expired. Ask your agent for a new one.",
            ),
        );
    };
    if who != auth::OWNER {
        let _ = state.meta.touch_sign_in(&who);
    }
    let c = auth::set_cookie(
        &auth::new_session(&state, &who),
        &headers,
        auth::SESSION_SECS,
    );
    let to = if who == auth::OWNER {
        after_owner_login(&state).await
    } else {
        "/"
    };
    with_cookie(Redirect::to(to).into_response(), &c)
}

/// Where the owner goes after logging in: the welcome page while nobody
/// has an account yet (the first setup), else the board.
async fn after_owner_login(state: &AppState) -> &'static str {
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || meta.users()).await {
        Ok(Ok(users)) if users.is_empty() => "/welcome",
        _ => "/",
    }
}

/// The welcome page is for the owner session only (admin token or owner
/// link): a user already has an account.
fn owner_only(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    match auth::session_id(state, headers) {
        Some(id) if id == auth::OWNER => None,
        Some(_) => Some(Redirect::to("/").into_response()),
        None => Some(to_login()),
    }
}

async fn welcome_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = owner_only(&state, &headers) {
        return r;
    }
    html(
        StatusCode::OK,
        openlens_ui::welcome(crate::oidc::enabled(), None),
    )
}

#[derive(Deserialize)]
struct WelcomeForm {
    email: String,
}

/// Create the owner's admin account and continue the session as that user.
async fn welcome_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<WelcomeForm>,
) -> Response {
    if let Some(r) = owner_only(&state, &headers) {
        return r;
    }
    if !crate::flags::same_origin(&headers) {
        return html(
            StatusCode::FORBIDDEN,
            openlens_ui::message("Refused", "Cross-site form."),
        );
    }
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || meta.upsert_user(&f.email, "admin", None)).await {
        Ok(Ok(u)) => {
            let c = auth::set_cookie(
                &auth::new_session(&state, &u.id),
                &headers,
                auth::SESSION_SECS,
            );
            with_cookie(Redirect::to("/").into_response(), &c)
        }
        Ok(Err(e)) => html(
            StatusCode::BAD_REQUEST,
            openlens_ui::welcome(crate::oidc::enabled(), Some(&e.to_string())),
        ),
        Err(_) => html(
            StatusCode::INTERNAL_SERVER_ERROR,
            openlens_ui::message("Oops", "Try again."),
        ),
    }
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
    let f = q.filters(&auth::tz_for(&state, &headers, &p.timezone));
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
            openlens_ui::board(
                &info,
                &b,
                &f,
                &crate::base_url(&headers),
                &crate::reword::plain(&b.insights)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| (p.title, p.next_step))
                    .collect::<Vec<_>>(),
            ),
        ),
        Err(e) => html(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e.to_string()),
        ),
    }
}

#[derive(Deserialize)]
struct EventsQuery {
    #[serde(flatten)]
    f: FilterQuery,
    family: Option<String>,
    person: Option<String>,
    path: Option<String>,
    days: Option<String>,
    sort: Option<String>,
    before: Option<String>,
    skip: Option<String>,
}

async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
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
    let raw = raw.unwrap_or_default();
    let Ok(q) = serde_urlencoded::from_str::<EventsQuery>(&raw) else {
        return html(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", "Unreadable filters."),
        );
    };
    let some = |v: &Option<String>| {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let f = q.f.filters(&auth::tz_for(&state, &headers, &p.timezone));
    let eq = openlens_engine::EventQuery {
        event: some(&q.f.event),
        family: some(&q.family),
        person: some(&q.person),
        path: some(&q.path),
        days: some(&q.days).and_then(|d| d.parse().ok()),
        sort: some(&q.sort).filter(|s| s == "event" || s == "person"),
        before: some(&q.before),
        skip: some(&q.skip).and_then(|d| d.parse().ok()).unwrap_or(0),
        limit: 100,
    };
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let map = crate::load_map(&state, &p.id).await;
    let (b, rows) = tokio::join!(
        openlens_engine::board(&state.query, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))),
        openlens_engine::event_list(&state.query, &p.id, &f, map.as_ref().map(|(m, _)| m), &eq)
    );
    match (b, rows) {
        (Ok(b), Ok(rows)) => {
            // The next page keeps every filter: a cursor when newest first,
            // an offset for the other sorts.
            let next = (rows.len() == eq.limit).then(|| {
                let kept: Vec<(String, String)> =
                    serde_urlencoded::from_str::<Vec<(String, String)>>(&raw)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|(k, _)| k != "before" && k != "skip")
                        .collect();
                let mut kept = kept;
                match eq.sort {
                    Some(_) => kept.push(("skip".into(), (eq.skip + eq.limit).to_string())),
                    None => kept.push((
                        "before".into(),
                        rows.last().map(|r| r.timestamp.clone()).unwrap_or_default(),
                    )),
                }
                format!(
                    "/p/{}/events?{}",
                    p.id,
                    serde_urlencoded::to_string(&kept).unwrap_or_default()
                )
            });
            html(
                StatusCode::OK,
                openlens_ui::events(&info, &b, &rows, &f, &eq, next.as_deref()),
            )
        }
        (Err(e), _) | (_, Err(e)) => html(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e.to_string()),
        ),
    }
}

/// The page the user came from, as a same-origin path, else `/`. A path
/// that a browser could read as another host (`//x`, `/\x`) is refused.
fn back_to(headers: &HeaderMap) -> String {
    let base = crate::base_url(headers);
    headers
        .get(header::REFERER)
        .and_then(|v| v.to_str().ok())
        .and_then(|r| r.strip_prefix(base.as_str()))
        .filter(|p| safe_local_path(p))
        .unwrap_or("/")
        .to_owned()
}

/// A path on this server: starts with one `/`, and has no backslash or
/// control character (browsers turn `/\host` into `//host`).
fn safe_local_path(p: &str) -> bool {
    p.starts_with('/')
        && !p.starts_with("//")
        && !p.bytes().any(|b| b == b'\\' || b.is_ascii_control())
}

/// `GET /theme/{auto|light|dark}`: remember the theme (cookie, 1 year) and
/// go back to the page the user came from (same-origin paths only).
async fn set_theme(headers: HeaderMap, Path(mode): Path<String>) -> Response {
    let mode = match mode.as_str() {
        "light" | "dark" => mode,
        _ => "auto".to_owned(),
    };
    let back = back_to(&headers);
    let cookie = format!("ol_theme={mode}; Path=/; SameSite=Lax; Max-Age=31536000");
    with_cookie(Redirect::to(&back).into_response(), &cookie)
}

/// `GET /nav/{collapsed|expanded}`: remember whether the sidebar shows only
/// icons (cookie, 1 year), then go back.
async fn set_nav(headers: HeaderMap, Path(mode): Path<String>) -> Response {
    let mode = if mode == "collapsed" {
        "collapsed"
    } else {
        "expanded"
    };
    let back = back_to(&headers);
    let cookie = format!("ol_nav={mode}; Path=/; SameSite=Lax; Max-Age=31536000");
    with_cookie(Redirect::to(&back).into_response(), &cookie)
}

/// Apply the theme cookie to HTML pages (`<html data-theme="…">`), so the
/// page renders in the right theme with no script and no flash.
pub(crate) async fn theme_layer(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let attrs = {
        let cookie = |name: &str| {
            req.headers()
                .get_all(header::COOKIE)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .flat_map(|v| v.split(';'))
                .filter_map(|kv| kv.trim().split_once('='))
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_owned())
        };
        let mut attrs = String::new();
        if let Some(t) = cookie("ol_theme").filter(|v| v == "light" || v == "dark") {
            attrs.push_str(&format!(" data-theme=\"{t}\""));
        }
        if cookie("ol_nav").as_deref() == Some("collapsed") {
            attrs.push_str(" data-nav=\"collapsed\"");
        }
        attrs
    };
    let resp = next.run(req).await;
    if attrs.is_empty() {
        return resp;
    }
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
        &format!("<html lang=\"en\"{attrs}>"),
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
    let f = q.filters(&auth::tz_for(&state, &headers, &p.timezone));
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
        (Ok(b), Ok(g)) => {
            let original = crate::resolve_frames(&state, &p.id, &g).await;
            html(
                StatusCode::OK,
                openlens_ui::errors(&info, &b, &g, &original, &f),
            )
        }
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
    let f = q.filters(&auth::tz_for(&state, &headers, &p.timezone));
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
    /// Playback position to open at, milliseconds (comment links).
    t: Option<u64>,
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
            openlens_ui::replays(
                &info,
                &l,
                q.u.as_deref(),
                &auth::tz_for(&state, &headers, &p.timezone),
                &state.meta.replay_comment_counts(&p.id).unwrap_or_default(),
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

/// The session's OpenLens events (page views, clicks, submits, errors,
/// success and custom events), for the markers on the player's timeline.
async fn replay_timeline(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, session)): Path<(String, String)>,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !openlens_replay::valid_id(&session) || project(&state, id.clone()).await.is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let sql = format!(
        "SELECT event, CAST(date_part('epoch', timestamp) * 1000 AS BIGINT) AS ms, feature, path, \
         regexp_match(properties, '\"\\$message\":\"([^\"]{{0,120}})')[1] AS message, \
         properties LIKE '%\"$rage\":true%' AS rage \
         FROM events WHERE session_id = '{session}' \
         AND event NOT IN ('$scroll', '$vital', '$identify') ORDER BY timestamp LIMIT 1000"
    );
    match state.query.sql(&id, &sql).await {
        Ok(r) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({ "events": r.rows })),
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
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

async fn projects_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(viewer) = auth::viewer(&state, &headers) else {
        return to_login();
    };
    let meta = state.meta.clone();
    let Ok(Ok(ps)) = tokio::task::spawn_blocking(move || meta.projects()).await else {
        return html(
            StatusCode::INTERNAL_SERVER_ERROR,
            openlens_ui::message("Oops", "The metadata store is unavailable."),
        );
    };
    let since = (chrono::Utc::now() - chrono::Duration::days(7)).format("%Y-%m-%d");
    let sql = format!(
        "SELECT count(*) AS n, CAST(max(timestamp) AS VARCHAR) AS last FROM events WHERE date >= '{since}'"
    );
    let mut list = Vec::new();
    for p in ps.into_iter().filter(|p| viewer.can_see(&p.id)) {
        let row = state
            .query
            .sql(&p.id, &sql)
            .await
            .ok()
            .and_then(|r| r.rows.into_iter().next());
        list.push(openlens_ui::ProjectSummary {
            recent_events: row
                .as_ref()
                .and_then(|r| r.get("n"))
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
            last_event: row
                .as_ref()
                .and_then(|r| r.get("last"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            id: p.id,
            name: p.name,
            timezone: p.timezone,
        });
    }
    list.sort_by_key(|p| std::cmp::Reverse(p.recent_events));
    html(StatusCode::OK, openlens_ui::projects(&list, viewer.admin))
}

#[derive(Deserialize)]
struct HeatmapQuery {
    path: String,
    session: Option<String>,
}

async fn click_map_for(
    state: &AppState,
    p: &openlens_meta::Project,
    path: &str,
) -> Result<(openlens_engine::ClickMap, String), openlens_engine::EngineError> {
    let map = crate::load_map(state, &p.id).await;
    let src = openlens_engine::source(map.as_ref().map(|(m, _)| m));
    let f = openlens_engine::Filters {
        tz: p.timezone.clone(),
        ..openlens_engine::Filters::default()
    };
    Ok((
        openlens_engine::click_map(&state.query, &p.id, &src, &f, path).await?,
        src,
    ))
}

async fn heatmap_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<HeatmapQuery>,
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
    let (map, src) = match click_map_for(&state, &p, &q.path).await {
        Ok(m) => m,
        Err(e) => {
            return html(
                StatusCode::BAD_REQUEST,
                openlens_ui::message("Can't show that", &e.to_string()),
            );
        }
    };
    // A recorded visit of this page, to draw the heatmap over the real page.
    let f = openlens_engine::Filters {
        tz: auth::tz_for(&state, &headers, &p.timezone),
        ..openlens_engine::Filters::default()
    };
    let sessions = openlens_engine::sessions_on_path(&state.query, &p.id, &src, &f, &q.path)
        .await
        .unwrap_or_default();
    let root = state.data_dir.clone();
    let pid = p.id.clone();
    let recorded: std::collections::HashSet<String> =
        match tokio::task::spawn_blocking(move || openlens_replay::list(&root, &pid, None)).await {
            Ok(Ok(l)) => l.into_iter().map(|r| r.session_id).collect(),
            _ => Default::default(),
        };
    let overlay = sessions
        .into_iter()
        .find(|s| recorded.contains(s))
        .map(|s| {
            format!(
                "/p/{}/heatmap/overlay?path={}&session={s}",
                p.id,
                openlens_ui::url_enc(&q.path)
            )
        });
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    html(
        StatusCode::OK,
        openlens_ui::heatmap(&info, &map, overlay.as_deref()),
    )
}

async fn heatmap_json(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<HeatmapQuery>,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(p) = project(&state, id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match click_map_for(&state, &p, &q.path).await {
        Ok((m, _)) => (
            [(header::CACHE_CONTROL, "no-store")],
            axum::Json(serde_json::json!({
                "columns": openlens_engine::MAP_COLUMNS,
                "row": openlens_engine::MAP_ROW,
                "map": m,
            })),
        )
            .into_response(),
        Err(_) => StatusCode::BAD_REQUEST.into_response(),
    }
}

async fn heatmap_overlay(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<HeatmapQuery>,
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
    let Some(session) = q.session.filter(|s| openlens_replay::valid_id(s)) else {
        return html(
            StatusCode::NOT_FOUND,
            openlens_ui::message("No recording", "Pick a page with a recorded visit."),
        );
    };
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    player_csp(html(
        StatusCode::OK,
        openlens_ui::heatmap_overlay(&info, &q.path, &session),
    ))
}

async fn digest_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
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
    let mailed = std::env::var("OPENLENS_SMTP_URL").is_ok_and(|s| !s.is_empty())
        && std::env::var("OPENLENS_DIGEST_TO").is_ok_and(|s| s.contains('@'));
    match crate::digest::compute(&state, &p, &crate::base_url(&headers)).await {
        Ok((d, _)) => {
            let info = openlens_ui::ProjectInfo {
                id: &p.id,
                name: &p.name,
                key: &p.api_key,
            };
            html(StatusCode::OK, openlens_ui::digest(&info, &d, mailed))
        }
        Err(e) => html(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e.to_string()),
        ),
    }
}

/// Access rules for every dashboard route (D-016): a user only reaches the
/// projects they may see, and only admins change things (any non-GET under
/// a project) or manage users.
pub(crate) async fn access_layer(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = req.uri().path().to_owned();
    let viewer = auth::viewer(&state, req.headers());
    if let (Some(v), Some(rest)) = (&viewer, path.strip_prefix("/p/")) {
        let project = rest.split('/').next().unwrap_or("");
        if !v.can_see(project) {
            return html(
                StatusCode::FORBIDDEN,
                openlens_ui::message("Not for you", "You do not have access to this project."),
            );
        }
        if req.method() != axum::http::Method::GET
            && !v.admin
            && !crate::comments::is_comment_path(&path)
        {
            return html(
                StatusCode::FORBIDDEN,
                openlens_ui::message(
                    "Read-only",
                    "Your role can look but not change things. Ask an admin.",
                ),
            );
        }
    }
    if path == "/users" || path.starts_with("/users/") {
        match &viewer {
            None => return to_login(),
            Some(v) if !v.admin => {
                return html(
                    StatusCode::FORBIDDEN,
                    openlens_ui::message("Admins only", "Ask an admin to invite people."),
                );
            }
            _ => {}
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod back_tests {
    #[test]
    fn only_local_paths_are_followed() {
        for ok in ["/", "/p/app/events?env=prod", "/p/a/replays/s1#t", "/%0d"] {
            assert!(super::safe_local_path(ok), "{ok}");
        }
        for bad in [
            "//evil.com",
            "/\\evil.com",
            "/\\/evil.com",
            "https://evil.com",
            "evil",
            "/\tx",
        ] {
            assert!(!super::safe_local_path(bad), "{bad}");
        }
    }
}
