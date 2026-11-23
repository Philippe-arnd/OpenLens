//! People and data explorer (#222): a person's profile, a breakdown page,
//! and read-only SQL for admins, over the same engine calls as the API.

use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::AppState;
use crate::api::FilterQuery;
use crate::auth;
use crate::web::html_page;

async fn project(state: &AppState, id: &str) -> Option<openlens_meta::Project> {
    let meta = state.meta.clone();
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || meta.project(&id))
        .await
        .ok()?
        .ok()?
}

fn not_found(what: &str) -> Response {
    html_page(
        StatusCode::NOT_FOUND,
        openlens_ui::message("Not found", what),
    )
}

/// `GET /p/{id}/people/{distinct_id}`.
pub(crate) async fn person(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, who)): Path<(String, String)>,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return Redirect::to("/login").into_response();
    }
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    if who.is_empty() || who.len() > 200 {
        return not_found("No such person.");
    }
    let tz = auth::tz_for(&state, &headers, &p.timezone);
    let map = crate::load_map(&state, &p.id).await;
    let (meta, root, pid, w) = (
        state.meta.clone(),
        state.data_dir.clone(),
        p.id.clone(),
        who.clone(),
    );
    let stored = tokio::task::spawn_blocking(move || {
        let person = meta.person(&pid, &w).ok().flatten();
        let roles = meta.role_history(&pid, &w).unwrap_or_default();
        let replays = openlens_replay::list(&root, &pid, Some(&w)).unwrap_or_default();
        (person, roles, replays)
    })
    .await;
    let Ok((person, roles, replays)) = stored else {
        return not_found("The metadata store is unavailable.");
    };
    let events = openlens_engine::person_events(
        &state.query,
        &p.id,
        map.as_ref().map(|(m, _)| m),
        &who,
        100,
    )
    .await
    .unwrap_or_default();
    if person.is_none() && events.is_empty() {
        return not_found("Nobody with this id in the last 90 days.");
    }
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    html_page(
        StatusCode::OK,
        openlens_ui::person(&info, &who, person.as_ref(), &roles, &replays, &events, &tz),
    )
}

#[derive(Deserialize)]
pub(crate) struct BreakdownQuery {
    #[serde(flatten)]
    f: FilterQuery,
    by: Option<String>,
}

/// `GET /p/{id}/breakdown?by=role|plan|account|environment|release|page|feature`.
pub(crate) async fn breakdown(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return Redirect::to("/login").into_response();
    }
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let Ok(q) = serde_urlencoded::from_str::<BreakdownQuery>(&raw.unwrap_or_default()) else {
        return not_found("Unreadable filters.");
    };
    let by = q.by.unwrap_or_else(|| "role".into());
    let f = q.f.filters(&auth::tz_for(&state, &headers, &p.timezone));
    let map = crate::load_map(&state, &p.id).await;
    let (b, rows) = tokio::join!(
        openlens_engine::board(&state.query, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))),
        openlens_engine::breakdown(&state.query, &p.id, map.as_ref().map(|(m, _)| m), &f, &by)
    );
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    match (b, rows) {
        (Ok(b), Ok(rows)) => html_page(
            StatusCode::OK,
            openlens_ui::breakdown(&info, &b, &f, &by, &rows),
        ),
        (Err(e), _) | (_, Err(e)) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e.to_string()),
        ),
    }
}

fn admin_only(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    match auth::viewer(state, headers) {
        None => Some(Redirect::to("/login").into_response()),
        Some(v) if !v.admin => Some(html_page(
            StatusCode::FORBIDDEN,
            openlens_ui::message("Admins only", "SQL shows every property of every event."),
        )),
        _ => None,
    }
}

/// `GET /p/{id}/sql`: the query box and the schema (admins).
pub(crate) async fn sql_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(r) = admin_only(&state, &headers) {
        return r;
    }
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let sql = "SELECT event, count(*) AS n FROM events\nWHERE date >= current_date - INTERVAL '7 days'\nGROUP BY event ORDER BY n DESC";
    html_page(StatusCode::OK, openlens_ui::sql(&info, sql, None, None))
}

#[derive(Deserialize)]
pub(crate) struct SqlForm {
    sql: String,
    #[serde(default)]
    format: String,
}

/// `POST /p/{id}/sql`: run it (read-only, the API's limits); `format=csv`
/// downloads the result.
pub(crate) async fn sql_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<SqlForm>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return html_page(
            StatusCode::FORBIDDEN,
            openlens_ui::message("Refused", "Cross-site form."),
        );
    }
    if let Some(r) = admin_only(&state, &headers) {
        return r;
    }
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    match state.query.sql(&p.id, &f.sql).await {
        Ok(res) if f.format == "csv" => {
            let mut r = csv(&res).into_response();
            let h = r.headers_mut();
            h.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/csv; charset=utf-8"),
            );
            if let Ok(v) =
                HeaderValue::from_str(&format!("attachment; filename=\"{}-query.csv\"", p.id))
            {
                h.insert(header::CONTENT_DISPOSITION, v);
            }
            r
        }
        Ok(res) => html_page(
            StatusCode::OK,
            openlens_ui::sql(&info, &f.sql, Some(&res), None),
        ),
        Err(e) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::sql(&info, &f.sql, None, Some(&e.to_string())),
        ),
    }
}

/// A result as CSV (RFC 4180 quoting; cells that a spreadsheet would run
/// as a formula are prefixed with `'`).
pub(crate) fn csv(res: &openlens_query::QueryResult) -> String {
    let cell = |s: &str| {
        let s = if s.starts_with(['=', '+', '-', '@']) {
            format!("'{s}")
        } else {
            s.to_owned()
        };
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s
        }
    };
    let mut out = res
        .columns
        .iter()
        .map(|c| cell(c))
        .collect::<Vec<_>>()
        .join(",");
    out.push('\n');
    for row in &res.rows {
        let line = res
            .columns
            .iter()
            .map(|c| match row.get(c) {
                None | Some(serde_json::Value::Null) => String::new(),
                Some(serde_json::Value::String(s)) => cell(s),
                Some(v) => cell(&v.to_string()),
            })
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&line);
        out.push('\n');
    }
    out
}
