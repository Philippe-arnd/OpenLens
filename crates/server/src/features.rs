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

#[derive(Deserialize, Default)]
pub(crate) struct PageQuery {
    #[serde(default)]
    v: Option<i64>,
    #[serde(default)]
    done: Option<String>,
}

/// `GET /p/{id}/features[?v=N]`: the map (or version N) and its status.
pub(crate) async fn page(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Response {
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
    let live = openlens_engine::reads::map_live(&state.query, &p.id, parsed.map.as_ref()).await;
    let history = state.meta.feature_map_versions(&p.id).unwrap_or_default();
    let (health, roles) = match parsed.map.as_ref() {
        Some(m) => {
            let f = openlens_engine::Filters {
                tz: p.timezone.clone(),
                ..openlens_engine::Filters::default()
            };
            // The version shown: its rolled-up days come from the rollups.
            let version = shown.as_ref().map(|(n, _)| *n);
            // Who uses what (#620): the full roles × features matrix.
            let (health, roles) = tokio::join!(
                openlens_engine::map_health_in(&state.query, &p.id, m, version, &f),
                openlens_engine::role_matrix(&state.query, &p.id, m, version, &f)
            );
            (health.ok(), roles.ok())
        }
        None => (None, None),
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
            roles: roles.as_ref(),
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
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<EditForm>,
) -> Response {
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
    let live = openlens_engine::reads::map_live(&state.query, &p.id, parsed.map.as_ref()).await;
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
            roles: None,
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

#[derive(Deserialize, Default)]
pub(crate) struct FeatureQuery {
    /// A string: inside a flattened struct, urlencoded numbers do not parse.
    #[serde(default)]
    v: Option<String>,
    #[serde(default)]
    tab: Option<String>,
    #[serde(flatten)]
    filters: crate::api::FilterQuery,
}

/// `GET /p/{id}/features/{key}`: one feature's page (#411), or the map
/// download for `map.yaml` / `map.json` (kept at their URLs).
pub(crate) async fn feature_or_download(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<FeatureQuery>,
) -> Response {
    if key == "map.yaml" || key == "map.json" {
        return download(
            crate::access::Session,
            State(state),
            Path((id, key)),
            Query(DownloadQuery {
                v: q.v.as_deref().and_then(|v| v.parse().ok()),
            }),
        )
        .await;
    }
    let Some(p) = project(&state, &id).await else {
        return html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    let gone = || {
        html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message_html(
                "No such feature",
                maud::html! { "This key is not in the feature map. " a href={ "/p/" (id) "/features" } { "Back to the feature map" } },
            ),
        )
    };
    let Some(m) = state
        .meta
        .feature_map(&p.id)
        .ok()
        .flatten()
        .and_then(|(_, y)| openlens_featuremap::parse(&y).map)
    else {
        return gone();
    };
    let Some(current) = m.current_key(&key).map(str::to_owned) else {
        return gone();
    };
    if current != key {
        return axum::response::Redirect::to(&format!("/p/{}/features/{current}", p.id))
            .into_response();
    }
    let Some(feature) = m.features.iter().find(|f| f.key == current) else {
        return gone();
    };
    let f = q.filters.filters(&p.timezone);
    let history = crate::map_history(&state, &p.id, &f.environment).await;
    let tab = q
        .tab
        .as_deref()
        .and_then(|t| openlens_ui::TABS.iter().find(|x| **x == t))
        .copied()
        .unwrap_or("overview");
    // Its funnel and paths to success (#620), on the Overview tab only,
    // beside its context.
    let paths = async {
        if tab == "overview" {
            openlens_engine::feature_paths(&state.query, &p.id, &m, &history, &f, &current)
                .await
                .ok()
        } else {
            None
        }
    };
    // What its users said (#718), on the Overview tab: texts for admins.
    let said = async {
        if tab == "overview" {
            crate::feedback::of_feature(&state, &headers, &p.id, &current, &f.environment).await
        } else {
            None
        }
    };
    let (ctx, paths, said) = tokio::join!(
        openlens_engine::feature_context(&state.query, &p.id, &m, &history, &f, &current),
        paths,
        said
    );
    let ctx = match ctx {
        Ok(Some(c)) => {
            crate::record_release_maps(&state, &p.id, &f, &c.releases).await;
            c
        }
        Ok(None) => return gone(),
        Err(e) => {
            return html_page(
                StatusCode::BAD_REQUEST,
                openlens_ui::message("Can't show that", &e.to_string()),
            );
        }
    };
    let peers: Vec<&openlens_featuremap::Feature> = m
        .features
        .iter()
        .filter(|x| x.area == feature.area)
        .collect();
    let at = peers.iter().position(|x| x.key == current).unwrap_or(0);
    let near = |i: Option<usize>| {
        i.and_then(|i| peers.get(i))
            .map(|x| (x.key.as_str(), x.name.as_str()))
    };
    let versions: Vec<(i64, String, Option<String>)> = state
        .meta
        .feature_map_versions(&p.id)
        .unwrap_or_default()
        .into_iter()
        .map(|v| (v.version, v.created_at, v.author))
        .collect();
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    // The latest sessions that used it, kept when they have a recording.
    let recorded: std::collections::HashSet<String> = if tab == "replays" {
        let (root, pid) = (state.data_dir.clone(), p.id.clone());
        tokio::task::spawn_blocking(move || openlens_replay::list(&root, &pid, None))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
            .into_iter()
            .map(|r| r.session_id)
            .collect()
    } else {
        Default::default()
    };
    let replays: Vec<&openlens_engine::maphealth::FeatureSession> = ctx
        .sessions
        .iter()
        .filter(|x| recorded.contains(&x.session_id))
        .take(20)
        .collect();
    html_page(
        StatusCode::OK,
        openlens_ui::feature_page(&openlens_ui::FeaturePage {
            p: &info,
            feature,
            ctx: &ctx,
            tab,
            filters: &f,
            replays: &replays,
            versions: &versions,
            prev: near(at.checked_sub(1)),
            next: near(Some(at + 1)),
            paths: paths.as_ref(),
            feedback: said.as_ref(),
        }),
    )
}

/// `GET /p/{id}/features/map.yaml` and `/features/map.json` (`?v=N` for a version).
pub(crate) async fn download(
    _: crate::access::Session,
    State(state): State<AppState>,
    Path((id, file)): Path<(String, String)>,
    Query(q): Query<DownloadQuery>,
) -> Response {
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
) -> Result<(openlens_meta::Project, openlens_featuremap::FeatureMap, i64), Box<Response>> {
    let bad = |s: StatusCode, m: &str| Box::new(crate::apierr::json(s, m));
    if !auth::has_bearer(state, headers) {
        return Err(bad(StatusCode::UNAUTHORIZED, "admin bearer token required"));
    }
    let Some(p) = project(state, id).await else {
        return Err(bad(StatusCode::NOT_FOUND, "unknown project"));
    };
    match crate::load_map(state, &p.id).await {
        Some((m, version)) => Ok((p, m, version)),
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
    let (p, m, version) = match api_guard(&state, &headers, &id).await {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let f = q.filters(&p.timezone);
    match openlens_engine::map_health_in(&state.query, &p.id, &m, Some(version), &f).await {
        Ok(h) => axum::Json(h).into_response(),
        Err(e) => crate::apierr::json(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

/// `GET /api/projects/{id}/feature-map/relations`: the relations inferred
/// between features from 30 days of sessions (#483), on their own.
pub(crate) async fn api_relations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<crate::api::FilterQuery>,
) -> Response {
    let (p, m, version) = match api_guard(&state, &headers, &id).await {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let f = q.filters(&p.timezone);
    match openlens_engine::feature_relations(&state.query, &p.id, &m, Some(version), &f).await {
        Ok(r) => axum::Json(serde_json::json!({ "relations": r })).into_response(),
        Err(e) => crate::apierr::json(StatusCode::BAD_REQUEST, e.to_string()),
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
    let (p, m, _) = match api_guard(&state, &headers, &id).await {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let f = q.filters(&p.timezone);
    let history = crate::map_history(&state, &p.id, &f.environment).await;
    match openlens_engine::feature_context(&state.query, &p.id, &m, &history, &f, &key).await {
        Ok(Some(c)) => {
            crate::record_release_maps(&state, &p.id, &f, &c.releases).await;
            axum::Json(c).into_response()
        }
        Ok(None) => crate::apierr::json(
            StatusCode::NOT_FOUND,
            format!("no feature `{key}` in the map"),
        ),
        Err(e) => crate::apierr::json(StatusCode::BAD_REQUEST, e.to_string()),
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
