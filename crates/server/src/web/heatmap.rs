//! Click maps. Moved out of web.rs as is (#307).

use super::*;

#[derive(Deserialize)]
pub(super) struct HeatmapQuery {
    path: String,
    session: Option<String>,
    /// The heatmap page's tab (#548): `rage` or `scroll`; clicks otherwise.
    mode: Option<String>,
}

pub(super) async fn click_map_for(
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

pub(super) async fn heatmap_page(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<HeatmapQuery>,
) -> Response {
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
    // The most viewed pages: the picker, and this page's labels and scroll
    // depth (#548). Without them the map still shows.
    let (sessions, pages) = tokio::join!(
        openlens_engine::sessions_on_path(&state.query, &p.id, &src, &f, &q.path),
        openlens_engine::pages(&state.query, &p.id, &src, &f)
    );
    let (sessions, pages) = (sessions.unwrap_or_default(), pages.unwrap_or_default());
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
        openlens_ui::heatmap(
            &info,
            &map,
            overlay.as_deref(),
            &openlens_ui::HeatmapView {
                mode: q.mode.as_deref().unwrap_or("clicks"),
                pages: &pages,
            },
        ),
    )
}

pub(super) async fn heatmap_json(
    _: crate::access::SessionData,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HeatmapQuery>,
) -> Response {
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

pub(super) async fn heatmap_overlay(
    _: crate::access::Session,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HeatmapQuery>,
) -> Response {
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
