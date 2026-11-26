//! The public status page (#561, D-079): `/status/<slug>` (HTML),
//! `/status/<slug>.json`, `/status/<slug>/badge.svg`,
//! `/status/<slug>/feed.atom` and `/status/<slug>/icon`, for anyone; and
//! its settings, on Settings → Sharing and through the API.
//!
//! The public routes take no sign-in and set no cookie. Each request
//! takes a token from its address's bucket first (a bucket of its own, so
//! a flood of status requests never takes capture's tokens), then the
//! slug is checked for its shape, and only then looked up. An unknown or
//! disabled slug answers 404 with no body, like a route that does not
//! exist. Every route reads the same `status_page.public` result, kept
//! [`TTL`] per page in a bounded cache; saving the settings clears it.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use openlens_engine::commands::status_page::{
    self as cmd, Component, GetPublic, ProjectInput, Public, PublicInput, SetInput, View,
    valid_slug,
};
use openlens_engine::commands::{Caller, Command, Error};
use serde::Deserialize;

use crate::AppState;
use crate::capture::{Peer, client_ip};
use crate::web::html_page;

/// How long one computed page is served.
pub(crate) const TTL: Duration = Duration::from_secs(30);
/// Pages kept at most.
pub(crate) const CACHED: usize = 1_000;

/// Requests a second per client address, and the burst allowed: a person
/// opening a few pages, a badge on a README behind a proxy, a feed reader;
/// not a scan.
const IP_RATE: f64 = 2.0;
const IP_BURST: f64 = 60.0;

pub(crate) fn routes() -> Router<AppState> {
    let public = Router::new()
        .route("/status/{slug}", get(page))
        .route("/status/{slug}/badge.svg", get(badge))
        .route("/status/{slug}/feed.atom", get(feed))
        .route("/status/{slug}/icon", get(icon))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(10),
        ));
    Router::new()
        .merge(public)
        .route("/p/{project}/settings/status-page", post(page_save))
        .route(
            "/api/projects/{project}/status-page",
            get(api_get).put(api_set),
        )
}

fn limited(state: &AppState, peer: &Peer, headers: &HeaderMap) -> bool {
    let Some(ip) = client_ip(peer, headers) else {
        return false;
    };
    !state
        .caches
        .status_buckets
        .lock()
        .is_ok_and(|mut b| b.take(&ip, 1.0, IP_RATE, IP_BURST))
}

fn too_many() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, HeaderValue::from_static("30"))],
    )
        .into_response()
}

/// A 404 with no body: an unknown and a disabled page look the same.
fn absent() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

/// The page of `slug`: from the cache, else `status_page.public`. `Err`
/// is the status to answer with no body.
async fn load(state: &AppState, slug: &str) -> Result<Arc<Public>, StatusCode> {
    if !valid_slug(slug) {
        return Err(StatusCode::NOT_FOUND);
    }
    if let Some(v) = state.caches.status_pages.get(&slug.to_owned()) {
        return Ok(v);
    }
    let input = PublicInput {
        slug: slug.to_owned(),
    };
    match GetPublic::run(state, &Caller::default(), input).await {
        Ok(v) => {
            let v = Arc::new(v);
            state.caches.status_pages.insert(slug.to_owned(), v.clone());
            Ok(v)
        }
        Err(Error::NotFound) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::warn!(error = %e, "status page could not be computed");
            Err(StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

/// The headers every public answer carries.
fn public_headers(r: &mut Response, cache: &'static str) {
    let h = r.headers_mut();
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
}

/// `GET /status/{slug}` (the page) and `GET /status/{slug}.json` (the
/// same data).
pub(crate) async fn page(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    if limited(&state, &peer, &headers) {
        return too_many();
    }
    let (slug, json) = match raw.strip_suffix(".json") {
        Some(s) => (s, true),
        None => (raw.as_str(), false),
    };
    let v = match load(&state, slug).await {
        Ok(v) => v,
        Err(s) if s == StatusCode::NOT_FOUND => return absent(),
        Err(s) => return s.into_response(),
    };
    if json {
        let mut r = Json(&*v).into_response();
        public_headers(&mut r, "public, max-age=30");
        r.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
        return r;
    }
    let mut r = (
        StatusCode::OK,
        axum::response::Html(openlens_ui::status_page(&v, slug).into_string()),
    )
        .into_response();
    public_headers(&mut r, "public, max-age=30");
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'self'; img-src 'self' data:; form-action 'none'; frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    r
}

#[derive(Deserialize, Default)]
pub(crate) struct BadgeQuery {
    #[serde(default)]
    window: Option<String>,
}

/// XML text: escaped, and without the characters XML cannot carry.
fn xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if c.is_control() || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// The badge's two texts and colour for `window` (`state`, or one of
/// the availability windows); `None` for another window.
pub(crate) fn badge_parts(v: &Public, window: &str) -> Option<(String, String, &'static str)> {
    let colour = |state: &str| match state {
        "operational" => "#2b7a3d",
        "degraded" | "maintenance" => "#9a6700",
        "down" => "#b3263e",
        _ => "#66667a",
    };
    if window == "state" {
        let value = match v.state {
            "operational" => "operational",
            "degraded" => "degraded",
            "down" => "down",
            "maintenance" => "maintenance",
            _ => "unknown",
        };
        return Some(("status".into(), value.into(), colour(v.state)));
    }
    if !cmd::WINDOWS.contains(&window) {
        return None;
    }
    let a = v.availability(window);
    let value = a.map_or_else(|| "no data".to_owned(), |a| format!("{:.2}%", a * 100.0));
    let state = match a {
        None => "unknown",
        Some(a) if a >= 0.999 => "operational",
        Some(a) if a >= 0.99 => "degraded",
        Some(_) => "down",
    };
    Some((format!("uptime {window}"), value, colour(state)))
}

/// A flat two-part badge.
fn badge_svg(label: &str, value: &str, colour: &str) -> String {
    let w = |s: &str| 12 + 7 * s.chars().count();
    let (lw, vw) = (w(label), w(value));
    let (label, value) = (xml(label), xml(value));
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{t}\" height=\"20\" role=\"img\" aria-label=\"{label}: {value}\">\
<title>{label}: {value}</title>\
<rect width=\"{lw}\" height=\"20\" rx=\"3\" fill=\"#44444f\"/>\
<rect x=\"{lw}\" width=\"{vw}\" height=\"20\" fill=\"{colour}\"/>\
<g fill=\"#ffffff\" font-family=\"Verdana,DejaVu Sans,sans-serif\" font-size=\"11\" text-anchor=\"middle\">\
<text x=\"{lx}\" y=\"14\">{label}</text><text x=\"{vx}\" y=\"14\">{value}</text></g></svg>",
        t = lw + vw,
        lx = lw / 2,
        vx = lw + vw / 2,
    )
}

/// `GET /status/{slug}/badge.svg[?window=state|24h|7d|30d|90d]`.
pub(crate) async fn badge(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Query(q): Query<BadgeQuery>,
) -> Response {
    if limited(&state, &peer, &headers) {
        return too_many();
    }
    let v = match load(&state, &slug).await {
        Ok(v) => v,
        Err(s) if s == StatusCode::NOT_FOUND => return absent(),
        Err(s) => return s.into_response(),
    };
    let window = q.window.as_deref().unwrap_or("state");
    let Some((label, value, colour)) = badge_parts(&v, window) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let mut r = badge_svg(&label, &value, colour).into_response();
    public_headers(&mut r, "public, max-age=60");
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("image/svg+xml"),
    );
    // An SVG opened on its own must not run anything.
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"),
    );
    r
}

fn rfc3339(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0).map_or_else(
        || "1970-01-01T00:00:00Z".to_owned(),
        |d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    )
}

/// The Atom feed of the page's announcements, newest first.
pub(crate) fn atom(v: &Public, page_url: &str) -> String {
    let updated = v
        .announcements
        .iter()
        .map(|a| a.updated_at)
        .max()
        .unwrap_or(v.generated_at);
    let mut out = String::new();
    let _ = write!(
        out,
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\">\
<title>{name} status</title><id>{url}</id><link rel=\"alternate\" type=\"text/html\" href=\"{url}\"/>\
<link rel=\"self\" href=\"{url}/feed.atom\"/><updated>{up}</updated><author><name>{name}</name></author>",
        name = xml(&v.name),
        url = xml(page_url),
        up = rfc3339(updated),
    );
    for a in &v.announcements {
        let mut text = a.body.clone();
        for u in &a.updates {
            let _ = write!(text, "\n\n{} ({}): {}", u.state, rfc3339(u.at), u.body);
        }
        let _ = write!(
            out,
            "<entry><title>{title} ({level})</title><id>{url}#a{id}</id><link href=\"{url}#a{id}\"/>\
<published>{start}</published><updated>{up}</updated><category term=\"{level}\"/>\
<content type=\"text\">{text}</content></entry>",
            title = xml(&a.title),
            level = xml(&a.level),
            url = xml(page_url),
            id = a.id,
            start = rfc3339(a.starts_at),
            up = rfc3339(a.updated_at),
            text = xml(text.trim()),
        );
    }
    out.push_str("</feed>\n");
    out
}

/// `GET /status/{slug}/feed.atom`.
pub(crate) async fn feed(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Response {
    if limited(&state, &peer, &headers) {
        return too_many();
    }
    let v = match load(&state, &slug).await {
        Ok(v) => v,
        Err(s) if s == StatusCode::NOT_FOUND => return absent(),
        Err(s) => return s.into_response(),
    };
    let url = format!("{}/status/{slug}", crate::public_url(&headers));
    let mut r = atom(&v, &url).into_response();
    public_headers(&mut r, "public, max-age=60");
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/atom+xml; charset=utf-8"),
    );
    r
}

/// `GET /status/{slug}/icon`: the project's stored favicon, or its
/// initial. Never fetches anything.
pub(crate) async fn icon(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Response {
    if limited(&state, &peer, &headers) {
        return too_many();
    }
    if !valid_slug(&slug) {
        return absent();
    }
    let meta = state.meta.clone();
    let found = tokio::task::spawn_blocking(move || {
        let row = meta.status_page_by_slug(&slug).ok().flatten()?;
        if !row.enabled {
            return None;
        }
        meta.project(&row.project).ok().flatten()
    })
    .await;
    match found {
        Ok(Some(p)) => crate::icon::stored(&state, &p.id, &p.name),
        _ => absent(),
    }
}

// Settings: the page's form and the API.

fn detail(v: &View) -> String {
    format!(
        "{} {} ({} components)",
        if v.settings.enabled { "on" } else { "off" },
        v.settings.slug.as_deref().unwrap_or("-"),
        v.settings.components.len()
    )
}

/// Saved: the old pages go at once.
fn saved(state: &AppState, headers: &HeaderMap, project: &str, v: &View) {
    state.caches.status_pages.clear();
    crate::privacy::audit(state, headers, Some(project), "status_page.set", &detail(v));
}

/// The settings form as `status_page.set`'s input: `enabled`, `slug`,
/// and per environment `show:<env>`, `name:<env>`, `rt:<env>`. Only the
/// project's own environments are read, so the form's size bounds
/// nothing but itself.
fn form_input(project: &str, monitored: &[String], f: &HashMap<String, String>) -> SetInput {
    let field = |k: String| f.get(&k).map(|v| v.trim().to_owned()).unwrap_or_default();
    let mut components = vec![Component {
        environment: "production".into(),
        name: field("name:production".into()),
        response_time: f.contains_key("rt:production"),
    }];
    for env in monitored {
        if f.contains_key(&format!("show:{env}")) {
            components.push(Component {
                environment: env.clone(),
                name: field(format!("name:{env}")),
                response_time: f.contains_key(&format!("rt:{env}")),
            });
        }
    }
    let slug = field("slug".into());
    SetInput {
        project: project.to_owned(),
        enabled: Some(f.contains_key("enabled")),
        slug: (!slug.is_empty()).then_some(slug),
        components: Some(components),
    }
}

/// `POST /p/{id}/settings/status-page`: `status_page.set`.
pub(crate) async fn page_save(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<HashMap<String, String>>,
) -> Response {
    let monitored = cmd::monitored(&state.meta, &id);
    let input = form_input(&id, &monitored, &f);
    match cmd::Set::run(&state, &Caller::default(), input).await {
        Ok(v) => {
            saved(&state, &headers, &id, &v);
            Redirect::to(&format!("/p/{id}/settings/sharing?done=status")).into_response()
        }
        Err(e) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't do that", &e.to_string()),
        ),
    }
}

/// The Settings card of a project, for the *Sharing* tab.
pub(crate) async fn card(
    state: &AppState,
    p: &openlens_meta::Project,
    host: &str,
    admin: bool,
) -> Option<maud::Markup> {
    let input = ProjectInput {
        project: p.id.clone(),
    };
    let v = cmd::Get::run(state, &Caller::default(), input).await.ok()?;
    let mut rows: Vec<openlens_ui::StatusComponentRow> = v
        .settings
        .components
        .iter()
        .map(|c| openlens_ui::StatusComponentRow {
            environment: c.environment.clone(),
            name: c.name.clone(),
            shown: true,
            response_time: c.response_time,
        })
        .collect();
    for env in &v.monitored {
        if !rows.iter().any(|r| &r.environment == env) {
            rows.push(openlens_ui::StatusComponentRow {
                environment: env.clone(),
                ..openlens_ui::StatusComponentRow::default()
            });
        }
    }
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let view = openlens_ui::StatusPageCard {
        enabled: v.settings.enabled,
        slug: v.settings.slug.clone().unwrap_or_default(),
        url: v.path.as_ref().map(|path| format!("{host}{path}")),
        components: rows,
        admin,
    };
    Some(openlens_ui::status_page_card(&info, &view))
}

fn failed(e: Error) -> Response {
    let (status, m) = match e {
        Error::NotFound => (StatusCode::NOT_FOUND, "unknown project".to_owned()),
        Error::Invalid(m) => (StatusCode::UNPROCESSABLE_ENTITY, m),
        Error::Unavailable(m) => (StatusCode::BAD_REQUEST, m),
        Error::Storage(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
    };
    crate::apierr::json(status, m)
}

async fn open(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
) -> Result<openlens_meta::Project, Box<Response>> {
    if let Some(r) = crate::api::guard(state, headers) {
        return Err(Box::new(r));
    }
    crate::api::known_project(state, id).await
}

/// `GET /api/projects/{id}/status-page`: `status_page.get`.
async fn api_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let input = ProjectInput { project: p.id };
    match cmd::Get::run(&state, &Caller::default(), input).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => failed(e),
    }
}

/// `PUT /api/projects/{id}/status-page`: `status_page.set` (only the
/// fields given change).
async fn api_set(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(mut v): Json<serde_json::Value>,
) -> Response {
    let p = match open(&state, &headers, &id).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    if let Some(o) = v.as_object_mut() {
        o.insert("project".into(), serde_json::json!(p.id));
    }
    let input: SetInput = match serde_json::from_value(v) {
        Ok(i) => i,
        Err(e) => {
            return crate::apierr::json(StatusCode::UNPROCESSABLE_ENTITY, e.to_string());
        }
    };
    match cmd::Set::run(&state, &Caller::default(), input).await {
        Ok(v) => {
            saved(&state, &headers, &p.id, &v);
            Json(v).into_response()
        }
        Err(e) => failed(e),
    }
}
