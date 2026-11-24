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

mod heatmap;
mod login;
mod replays;
use heatmap::*;
use login::*;
use replays::*;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        .route("/login", get(login_page).post(login))
        .route("/login/email", post(login_email))
        .route("/login/password", post(login_password))
        .route("/me/password", post(crate::users::me_password))
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
        .route("/l/{token}", get(login_link_page).post(login_link))
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
        .route(
            "/p/{project}/timeline",
            get(crate::timeline::page).post(crate::timeline::add),
        )
        .route(
            "/p/{project}/timeline/{nid}/delete",
            post(crate::timeline::delete),
        )
        .route("/p/{project}/settings/share", post(crate::share::create))
        .route(
            "/p/{project}/settings/share/{lid}/revoke",
            post(crate::share::revoke),
        )
        .route("/s/{token}", get(crate::share::view))
        .route(
            "/p/{project}/settings/alerts",
            post(crate::settings::alerts),
        )
        .route(
            "/p/{project}/settings/alerts/test",
            post(crate::settings::alerts_test),
        )
        .route("/p/{project}/uptime", get(crate::settings::uptime))
        .route(
            "/settings/ai",
            get(crate::ask::ai_page).post(crate::ask::ai_save),
        )
        .route(
            "/p/{project}/ask",
            get(crate::ask::page).post(crate::ask::ask),
        )
        .route("/p/{project}/ask/confirm", post(crate::ask::confirm))
        .route("/p/{project}/ask/clear", post(crate::ask::clear))
        .route("/p/{project}/ask/privacy", post(crate::ask::privacy))
        .route("/p/{project}/ask/sent", get(crate::ask::sent))
        .route("/p/{project}/ask/export.md", get(crate::ask::export))
        .route(
            "/p/{project}/tickets",
            get(crate::tickets::list).post(crate::tickets::create),
        )
        .route("/p/{project}/tickets/new", get(crate::tickets::new))
        .route("/p/{project}/tickets/target", post(crate::tickets::target))
        .route(
            "/settings/tokens",
            get(crate::tokens::page).post(crate::tokens::create),
        )
        .route("/settings/tokens/{id}/revoke", post(crate::tokens::revoke))
        .route(
            "/settings/sessions/revoke",
            post(crate::tokens::sign_out_everywhere),
        )
        .route("/settings/storage", get(crate::storage::page))
        .route("/settings/storage/s3", post(crate::storage::s3))
        .route("/settings/storage/cold", post(crate::storage::cold))
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
        .route(
            "/p/{project}/people/{who}/forget",
            post(crate::privacy::forget),
        )
        .route(
            "/p/{project}/people/{who}/export.json",
            get(crate::privacy::export),
        )
        .route("/audit", get(crate::privacy::audit_page))
        .route("/p/{project}/breakdown", get(crate::explore::breakdown))
        .route("/p/{project}/audience", get(crate::explore::audience))
        .route("/p/{project}/forms", get(crate::explore::forms))
        .route("/p/{project}/acquisition", get(crate::explore::acquisition))
        .route("/p/{project}/churn", get(crate::explore::churn))
        .route(
            "/p/{project}/churn/audience",
            post(crate::explore::make_audience),
        )
        .route(
            "/p/{project}/churn/{file}",
            get(crate::explore::at_risk_export),
        )
        .route("/p/{project}/revenue", get(crate::explore::revenue))
        .route(
            "/p/{project}/goals",
            get(crate::goals::page).post(crate::goals::create),
        )
        .route(
            "/p/{project}/goals/{gid}/delete",
            post(crate::goals::delete),
        )
        .route(
            "/p/{project}/objectives",
            post(crate::goals::create_objective),
        )
        .route(
            "/p/{project}/objectives/{oid}/delete",
            post(crate::goals::delete_objective),
        )
        .route("/p/{project}/flows", get(crate::explore::flows))
        .route("/p/{project}/live", get(crate::explore::live))
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

pub(crate) fn to_login() -> Response {
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

async fn home(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
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

async fn project(state: &AppState, id: String) -> Option<openlens_meta::Project> {
    let meta = state.meta.clone();
    tokio::task::spawn_blocking(move || meta.project(&id))
        .await
        .ok()?
        .ok()?
}

async fn board(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<FilterQuery>,
) -> Response {
    let Some(p) = project(&state, id).await else {
        return html(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let f = q.filters(&auth::tz_for(&state, &headers, &p.timezone));
    let map = crate::load_map(&state, &p.id).await;
    let host = crate::public_url(&headers);
    if crate::boards::warm(&state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))) {
        return board_page(state, p, f, map, host).await;
    }
    // A cold cache means seconds of computing: send the page shell at once
    // (head, sidebar, a loading line), then the board when it is ready
    // (#301). The loading line hides itself, in CSS, once the board follows
    // it. Errors after the first byte show in the page (status 200).
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let shell = openlens_ui::board_shell(&info).into_string();
    let Some(cut) = shell.rfind("</main>") else {
        return board_page(state, p, f, map, host).await;
    };
    let (open, close) = (shell[..cut].to_owned(), shell[cut..].to_owned());
    let rest = async move {
        let page = board_page(state, p, f, map, host).await;
        let bytes = axum::body::to_bytes(page.into_body(), 64 * 1024 * 1024)
            .await
            .unwrap_or_default();
        let full = String::from_utf8_lossy(&bytes);
        let inner = match (full.find("<main>"), full.rfind("</main>")) {
            (Some(a), Some(b)) if a + 6 <= b => full[a + 6..b].to_owned(),
            _ => "<p class=\"alert\" role=\"alert\">Couldn't finish this page. Reload it.</p>"
                .to_owned(),
        };
        Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(inner + &close))
    };
    use futures::StreamExt;
    let stream = futures::stream::iter([Ok::<_, std::convert::Infallible>(
        axum::body::Bytes::from(open),
    )])
    .chain(futures::stream::once(rest));
    let mut r = html(StatusCode::OK, maud::PreEscaped(String::new()));
    r.headers_mut().remove(header::CONTENT_LENGTH);
    *r.body_mut() = axum::body::Body::from_stream(stream);
    r
}

/// The whole board page, computed now (or read from the cache).
async fn board_page(
    state: AppState,
    p: openlens_meta::Project,
    f: openlens_engine::Filters,
    map: Option<(openlens_featuremap::FeatureMap, i64)>,
    host: String,
) -> Response {
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let (board, revenue) = tokio::join!(
        crate::boards::board(&state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))),
        openlens_engine::revenue(&state.query, &p.id, map.as_ref().map(|(m, _)| m), &f)
    );
    let revenue = revenue.ok().flatten();
    let mut board = board;
    if let Ok(b) = board.as_mut() {
        b.objectives_at_risk = crate::goals::snapshot(&state, &p.id).0;
        let st = crate::alerts::status(&state, &p.id);
        if st.state == "down" {
            b.app_down = st.open.first().map(|(rule, since, _)| {
                let when = chrono::DateTime::from_timestamp(*since, 0)
                    .map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M UTC").to_string());
                format!(
                    "{} since {when}",
                    if rule == "traffic" {
                        "No traffic"
                    } else {
                        "The app is down"
                    }
                )
            });
        }
    }
    let releases: Vec<(String, String)> = board
        .as_ref()
        .map(|b| {
            b.releases
                .iter()
                .map(|r| (r.release.clone(), r.first_seen.clone()))
                .collect()
        })
        .unwrap_or_default();
    let marks = crate::timeline::collect(&state, &p.id, &releases, 14);
    match board {
        Ok(b) => html(
            StatusCode::OK,
            openlens_ui::board(
                &info,
                &b,
                &f,
                &host,
                &crate::reword::plain(&state, &b.insights)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| (p.title, p.next_step))
                    .collect::<Vec<_>>(),
                revenue.as_ref(),
                &marks,
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
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Response {
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
        openlens_engine::filter_options(&state.query, &p.id, &f, map.as_ref().map(|(m, _)| m)),
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
    let base = crate::public_url(headers);
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

tokio::task_local! {
    /// The theme and sidebar choice of the request being served (#300).
    static PREFS: openlens_ui::Prefs;
}

/// The current request's display choices, for `openlens_ui` (a page
/// rendered outside a request gets the defaults).
pub(crate) fn current_prefs() -> openlens_ui::Prefs {
    PREFS.try_with(|p| *p).unwrap_or_default()
}

/// Read the theme and sidebar cookies once and keep them for the request,
/// so the template writes them on `<html>` (#300): no response is read
/// back or rewritten. Only known values are kept.
pub(crate) async fn theme_layer(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let prefs = {
        let cookie = |name: &str| auth::cookie(req.headers(), name);
        openlens_ui::Prefs {
            theme: match cookie("ol_theme").as_deref() {
                Some("light") => Some("light"),
                Some("dark") => Some("dark"),
                _ => None,
            },
            nav_collapsed: cookie("ol_nav").as_deref() == Some("collapsed"),
        }
    };
    PREFS.scope(prefs, next.run(req)).await
}

tokio::task_local! {
    /// The project whose menu an instance page keeps (#403).
    static NAV_PROJECT: Option<openlens_ui::NavProject>;
}

/// For `openlens_ui`: the current request's [`NAV_PROJECT`].
pub(crate) fn current_nav_project() -> Option<openlens_ui::NavProject> {
    NAV_PROJECT.try_with(Clone::clone).ok().flatten()
}

/// Instance pages keep a project menu (#403): remember the last project
/// page visited (`ol_last` cookie), and on the instance pages show that
/// project when the viewer still sees it, else their only project.
pub(crate) async fn nav_layer(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = req.uri().path().to_owned();
    let visited = path
        .strip_prefix("/p/")
        .map(|rest| rest.split('/').next().unwrap_or(""))
        .filter(|id| openlens_replay::valid_id(id))
        .map(str::to_owned);
    let instance = [
        "/settings/tokens",
        "/settings/ai",
        "/settings/storage",
        "/users",
        "/me",
    ]
    .iter()
    .any(|p| path == *p || path.starts_with(&format!("{p}/")));
    let nav = if instance {
        auth::viewer(&state, req.headers()).and_then(|viewer| {
            let projects = state.meta.projects().ok()?;
            let seen: Vec<_> = projects
                .into_iter()
                .filter(|p| viewer.can_see(&p.id))
                .collect();
            let last = auth::cookie(req.headers(), "ol_last");
            let pick = seen
                .iter()
                .find(|p| last.as_deref() == Some(p.id.as_str()))
                .or_else(|| seen.first().filter(|_| seen.len() == 1))?;
            Some(openlens_ui::NavProject {
                id: pick.id.clone(),
                name: pick.name.clone(),
                key: pick.api_key.clone(),
            })
        })
    } else {
        None
    };
    let r = NAV_PROJECT.scope(nav, next.run(req)).await;
    match visited {
        Some(id)
            if r.status().is_success()
                && r.headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.starts_with("text/html")) =>
        {
            with_cookie(
                r,
                &format!("ol_last={id}; Path=/; SameSite=Lax; HttpOnly; Max-Age=31536000"),
            )
        }
        _ => r,
    }
}

async fn errors_page(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<FilterQuery>,
) -> Response {
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
        openlens_engine::filter_options(&state.query, &p.id, &f, map.as_ref().map(|(m, _)| m)),
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
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<FilterQuery>,
) -> Response {
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
        openlens_engine::filter_options(&state.query, &p.id, &f, map.as_ref().map(|(m, _)| m)),
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

async fn projects_page(
    crate::access::Signed(viewer): crate::access::Signed,
    State(state): State<AppState>,
) -> Response {
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

async fn digest_page(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Some(p) = project(&state, id).await else {
        return html(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let mailed = std::env::var("OPENLENS_SMTP_URL").is_ok_and(|s| !s.is_empty())
        && std::env::var("OPENLENS_DIGEST_TO").is_ok_and(|s| s.contains('@'));
    match crate::digest::compute(&state, &p, &crate::public_url(&headers)).await {
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
    // Project pages need a session (or the bearer token): fail closed here
    // rather than in each handler (#294). The icon too: it would tell who
    // a project id belongs to, and it fetches from the app's site.
    if viewer.is_none() && path.starts_with("/p/") && !auth::has_bearer(&state, req.headers()) {
        // Data files (JSON, CSV) answer 401; pages send to the login.
        let file = path
            .rsplit('/')
            .next()
            .is_some_and(|last| last.contains('.'));
        if file {
            return (StatusCode::UNAUTHORIZED, "sign in first").into_response();
        }
        return to_login();
    }
    if let (Some(v), Some(rest)) = (&viewer, path.strip_prefix("/p/")) {
        let project = rest.split('/').next().unwrap_or("");
        if !v.can_see(project) {
            return html(
                StatusCode::FORBIDDEN,
                openlens_ui::message("Not for you", "You do not have access to this project."),
            );
        }
        // Who may change what is the handler's to say: a handler that
        // changes a project takes `access::ProjectWrite` (#339).
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
