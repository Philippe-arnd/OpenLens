//! The feature map in the dashboard (#221): read it with its live status,
//! lint and diff an edit before saving it (admins), its history, and
//! downloads as YAML or JSON. Same parser and validation as the API/CLI.

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::AppState;
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

/// What the last 7 days say about the map: uses and users per feature,
/// hits per success event, raw roles no role matches.
async fn live(
    state: &AppState,
    project: &str,
    map: Option<&openlens_featuremap::FeatureMap>,
) -> openlens_ui::MapLive {
    let mut out = openlens_ui::MapLive::default();
    let Some(m) = map else { return out };
    let since = (chrono::Utc::now() - chrono::Duration::days(7)).format("%Y-%m-%d");
    let src = openlens_engine::source(Some(m));
    let uses = format!(
        "SELECT feature, count(*) AS n, count(DISTINCT distinct_id) AS u FROM {src} \
         WHERE date >= '{since}' AND feature IS NOT NULL GROUP BY feature"
    );
    let roles = format!(
        "SELECT DISTINCT role FROM raw_events WHERE date >= '{since}' AND role IS NOT NULL LIMIT 200"
    );
    let successes: Vec<String> = m
        .features
        .iter()
        .filter_map(|f| f.success.clone())
        .filter(|s| s.len() <= 200 && !s.contains('\''))
        .collect();
    let hits = (!successes.is_empty()).then(|| {
        format!(
            "SELECT event, count(*) AS n FROM events WHERE date >= '{since}' AND event IN ({}) GROUP BY event",
            successes
                .iter()
                .map(|s| format!("'{s}'"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    });
    let (a, b) = tokio::join!(
        state.query.sql(project, &uses),
        state.query.sql(project, &roles)
    );
    let int = |r: &serde_json::Map<String, serde_json::Value>, k: &str| {
        r.get(k).and_then(serde_json::Value::as_i64).unwrap_or(0)
    };
    let txt = |r: &serde_json::Map<String, serde_json::Value>, k: &str| {
        r.get(k).and_then(|v| v.as_str()).map(str::to_owned)
    };
    if let Ok(r) = a {
        for row in &r.rows {
            if let Some(f) = txt(row, "feature") {
                out.uses.push((f, int(row, "n"), int(row, "u")));
            }
        }
    }
    if let Ok(r) = b {
        for row in &r.rows {
            if let Some(raw) = txt(row, "role")
                && m.resolve_role(&raw).is_none()
                && !m.roles.is_empty()
            {
                out.unmapped_roles.push(raw);
            }
        }
    }
    if let Some(sql) = hits
        && let Ok(r) = state.query.sql(project, &sql).await
    {
        for row in &r.rows {
            if let Some(e) = txt(row, "event") {
                out.success_hits.push((e, int(row, "n")));
            }
        }
    }
    out
}

#[derive(Deserialize, Default)]
pub(crate) struct PageQuery {
    #[serde(default)]
    v: Option<i64>,
    #[serde(default)]
    done: Option<String>,
}

/// `GET /p/{id}/features[?v=N]`: the map (or version N) and its status.
pub(crate) async fn page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Response {
    let Some(v) = auth::viewer(&state, &headers) else {
        return Redirect::to("/login").into_response();
    };
    let Some(p) = project(&state, &id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let current = state.meta.feature_map(&p.id).ok().flatten();
    let shown = match q.v {
        Some(n) => state
            .meta
            .feature_map_version(&p.id, n)
            .ok()
            .flatten()
            .map(|y| (n, y)),
        None => current.clone(),
    };
    let yaml = shown.as_ref().map_or_else(String::new, |(_, y)| y.clone());
    let parsed = openlens_featuremap::parse(&yaml);
    let live = live(&state, &p.id, parsed.map.as_ref()).await;
    let history = state.meta.feature_map_versions(&p.id).unwrap_or_default();
    let health = match parsed.map.as_ref() {
        Some(m) => {
            let f = openlens_engine::Filters {
                tz: p.timezone.clone(),
                ..openlens_engine::Filters::default()
            };
            openlens_engine::map_health(&state.query, &p.id, m, &f)
                .await
                .ok()
        }
        None => None,
    };
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let notice = q.done.as_deref().map(|d| match d {
        "same" => "Nothing changed: the map is the same as the stored one.".to_owned(),
        n => format!("Saved as version {n}."),
    });
    html_page(
        StatusCode::OK,
        openlens_ui::feature_map(&openlens_ui::MapView {
            p: &info,
            yaml: &yaml,
            version: shown.as_ref().map(|(n, _)| *n),
            latest: current.as_ref().map(|(n, _)| *n),
            map: parsed.map.as_ref(),
            issues: &parsed.issues,
            live: &live,
            history: &history,
            health: health.as_ref(),
            admin: v.admin,
            draft: None,
            notice: notice.as_deref(),
        }),
    )
}

#[derive(Deserialize)]
pub(crate) struct EditForm {
    yaml: String,
    /// `check` (lint + diff) or `save`.
    #[serde(default)]
    action: String,
    /// Why (kept with the version).
    #[serde(default)]
    message: Option<String>,
}

/// `POST /p/{id}/features`: check (lint and diff) or save an edit (admins).
pub(crate) async fn edit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<EditForm>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return html_page(
            StatusCode::FORBIDDEN,
            openlens_ui::message("Refused", "Cross-site form."),
        );
    }
    let Some(p) = project(&state, &id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    if f.yaml.len() > 512 * 1024 {
        return html_page(
            StatusCode::PAYLOAD_TOO_LARGE,
            openlens_ui::message("Too big", "A feature map is at most 512 kB."),
        );
    }
    let yaml = f.yaml.replace("\r\n", "\n");
    let mut parsed = openlens_featuremap::parse(&yaml);
    if let Some(next) = &parsed.map {
        let prev = crate::load_map(&state, &p.id).await;
        let w = rename_warnings(prev.as_ref().map(|(m, _)| m), next);
        parsed.issues.extend(w);
    }
    if f.action == "save" && parsed.map.is_some() {
        let meta = state.meta.clone();
        let (pid, y) = (p.id.clone(), yaml.clone());
        let before = state.meta.feature_map(&p.id).ok().flatten().map(|(v, _)| v);
        let author = crate::privacy::actor(&state, &headers);
        let message = f.message.clone().filter(|m| !m.trim().is_empty());
        return match tokio::task::spawn_blocking(move || {
            meta.save_feature_map_by(&pid, &y, Some(&author), message.as_deref())
        })
        .await
        {
            Ok(Ok(v)) => {
                let done = if Some(v) == before {
                    "same".to_owned()
                } else {
                    v.to_string()
                };
                crate::privacy::audit(
                    &state,
                    &headers,
                    Some(&p.id),
                    "feature_map.save",
                    &format!("version {v}"),
                );
                Redirect::to(&format!("/p/{}/features?done={done}", p.id)).into_response()
            }
            _ => html_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                openlens_ui::message("Oops", "The metadata store is unavailable."),
            ),
        };
    }
    let current = state.meta.feature_map(&p.id).ok().flatten();
    let stored = current
        .as_ref()
        .map_or_else(String::new, |(_, y)| y.clone());
    let live = live(&state, &p.id, parsed.map.as_ref()).await;
    let history = state.meta.feature_map_versions(&p.id).unwrap_or_default();
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let status = if parsed.map.is_some() {
        StatusCode::OK
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    };
    html_page(
        status,
        openlens_ui::feature_map(&openlens_ui::MapView {
            p: &info,
            yaml: &yaml,
            version: None,
            latest: current.as_ref().map(|(n, _)| *n),
            map: parsed.map.as_ref(),
            issues: &parsed.issues,
            live: &live,
            history: &history,
            health: None,
            admin: true,
            draft: Some(&stored),
            notice: None,
        }),
    )
}

#[derive(Deserialize, Default)]
pub(crate) struct DownloadQuery {
    #[serde(default)]
    v: Option<i64>,
}

/// `GET /p/{id}/features/map.yaml` and `/features/map.json` (`?v=N` for a version).
pub(crate) async fn download(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, file)): Path<(String, String)>,
    Query(q): Query<DownloadQuery>,
) -> Response {
    if !auth::has_session(&state, &headers) {
        return Redirect::to("/login").into_response();
    }
    let yaml = match q.v {
        Some(n) => state.meta.feature_map_version(&id, n).ok().flatten(),
        None => state.meta.feature_map(&id).ok().flatten().map(|(_, y)| y),
    };
    let Some(yaml) = yaml else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (body, mime, ext) = match file.as_str() {
        "map.yaml" => (yaml, "application/yaml", "yaml"),
        "map.json" => match openlens_featuremap::parse(&yaml).map {
            Some(m) => (
                serde_json::to_string_pretty(&m).unwrap_or_default(),
                "application/json",
                "json",
            ),
            None => return StatusCode::UNPROCESSABLE_ENTITY.into_response(),
        },
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let mut r = body.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    if let Ok(v) = HeaderValue::from_str(&format!(
        "attachment; filename=\"openlens.features-{id}.{ext}\""
    )) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    r
}

/// `renamed_from` keys that were not features of the previous version: kept
/// (the app may have tagged them without declaring them), but flagged (#240).
pub(crate) fn rename_warnings(
    previous: Option<&openlens_featuremap::FeatureMap>,
    next: &openlens_featuremap::FeatureMap,
) -> Vec<openlens_featuremap::Issue> {
    let Some(prev) = previous else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, f) in next.features.iter().enumerate() {
        for (j, old) in f.renamed_from.iter().enumerate() {
            if prev.current_key(old).is_none() {
                out.push(openlens_featuremap::Issue {
                    level: "warning",
                    path: format!("features[{i}].renamed_from[{j}]"),
                    message: format!(
                        "`{old}` was not a feature of the previous version: fine if the app tagged it, otherwise remove it"
                    ),
                });
            }
        }
    }
    out
}

async fn api_guard(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
) -> Result<(openlens_meta::Project, openlens_featuremap::FeatureMap), Box<Response>> {
    let bad = |s: StatusCode, m: &str| {
        Box::new((s, axum::Json(serde_json::json!({ "error": m }))).into_response())
    };
    if !auth::has_bearer(state, headers) {
        return Err(bad(StatusCode::UNAUTHORIZED, "admin bearer token required"));
    }
    let Some(p) = project(state, id).await else {
        return Err(bad(StatusCode::NOT_FOUND, "unknown project"));
    };
    match crate::load_map(state, &p.id).await {
        Some((m, _)) => Ok((p, m)),
        None => Err(bad(
            StatusCode::NOT_FOUND,
            "no feature map yet: push one (openlens-cli features push)",
        )),
    }
}

/// `GET /api/projects/{id}/feature-map/health`: score, findings with fixes,
/// per-feature numbers, areas and the adoption ladder (#240).
pub(crate) async fn api_health(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<crate::api::FilterQuery>,
) -> Response {
    let (p, m) = match api_guard(&state, &headers, &id).await {
        Ok(x) => x,
        Err(r) => return *r,
    };
    match openlens_engine::map_health(&state.query, &p.id, &m, &q.filters(&p.timezone)).await {
        Ok(h) => axum::Json(h).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /api/projects/{id}/feature-map/features/{key}`: one feature in
/// depth, for agents (#240).
pub(crate) async fn api_feature(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<crate::api::FilterQuery>,
) -> Response {
    let (p, m) = match api_guard(&state, &headers, &id).await {
        Ok(x) => x,
        Err(r) => return *r,
    };
    match openlens_engine::feature_context(&state.query, &p.id, &m, &q.filters(&p.timezone), &key)
        .await
    {
        Ok(Some(c)) => axum::Json(c).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "error": format!("no feature `{key}` in the map") })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /api/projects/{id}/feature-map/versions`: who changed the map,
/// when and why (#240).
pub(crate) async fn api_versions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(p) = project(&state, &id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    axum::Json(serde_json::json!({
        "versions": state.meta.feature_map_versions(&p.id).unwrap_or_default()
    }))
    .into_response()
}
