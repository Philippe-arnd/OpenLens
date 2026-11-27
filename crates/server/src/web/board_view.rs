//! The board page, moved out of web.rs (#515), and the rendering of a
//! board a background job computed.

use super::*;

pub(super) async fn board(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<crate::board_query::BoardQuery>,
) -> Response {
    let Some(p) = project(&state, id).await else {
        return html(
            StatusCode::NOT_FOUND,
            openlens_ui::message("Not found", "No such project."),
        );
    };
    // The board page reads its period blocks since the current release
    // (D-055), or the date range or past release its picker asked for
    // (D-060).
    let f = match q.filters(&auth::tz_for(&state, &headers, &p.timezone)) {
        Ok(f) => f,
        Err(e) => {
            return html(
                StatusCode::BAD_REQUEST,
                openlens_ui::message("Can't show that", &e.to_string()),
            );
        }
    };
    let map = crate::load_map(&state, &p.id).await;
    // A long date range or past release shows its cost first (D-061).
    let m = map.as_ref().map(|(m, v)| (m, *v));
    match crate::board_query::unconfirmed(&state, &p.id, &q, &f, m).await {
        Ok(None) => {}
        Ok(Some(e)) => {
            let info = openlens_ui::ProjectInfo {
                id: &p.id,
                name: &p.name,
                key: &p.api_key,
            };
            // "Run it" starts a background job (#515).
            let run = q.params();
            return html(StatusCode::OK, openlens_ui::board_confirm(&info, &e, &run));
        }
        Err(e) => {
            return html(
                StatusCode::BAD_REQUEST,
                openlens_ui::message("Can't show that", &e),
            );
        }
    }
    let host = crate::public_url(&headers);
    let admin = auth::viewer(&state, &headers).is_some_and(|v| v.admin);
    let ready = crate::boards::ready(&state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v)));
    // Served from the cache, or computed last time in under a second: the
    // whole page at once, with no waiting panel (D-073).
    if ready.hit
        || ready
            .estimate
            .is_some_and(|d| d < std::time::Duration::from_secs(1))
    {
        return board_page(state, p, f, map, host, admin).await;
    }
    // A cold cache means seconds of computing: send the page shell at once
    // (head, sidebar, a waiting panel with the last compute time), then the
    // board when it is ready (#301, D-073). The panel hides itself, in CSS,
    // once the board follows it. Errors after the first byte show in the
    // page (status 200).
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    // Whole seconds, rounded up: "about 2 s" for 1.5 s.
    let wait = ready
        .estimate
        .map(|d| d.as_millis().div_ceil(1000).try_into().unwrap_or(u64::MAX));
    let shell = openlens_ui::board_shell(&info, wait).into_string();
    let Some(cut) = shell.rfind("</main>") else {
        return board_page(state, p, f, map, host, admin).await;
    };
    let (open, close) = (shell[..cut].to_owned(), shell[cut..].to_owned());
    // What the shell's `<main>` has before the panel (breadcrumbs, tabs):
    // the board page repeats it, so it is left out of the board sent after.
    let before = match (
        shell.find("<main>"),
        shell.find("<div class=\"board-waiting\""),
    ) {
        (Some(a), Some(b)) if a + 6 <= b => shell[a + 6..b].to_owned(),
        _ => String::new(),
    };
    let rest = async move {
        let page = board_page(state, p, f, map, host, admin).await;
        let bytes = axum::body::to_bytes(page.into_body(), 64 * 1024 * 1024)
            .await
            .unwrap_or_default();
        let full = String::from_utf8_lossy(&bytes);
        let inner = match (full.find("<main>"), full.rfind("</main>")) {
            (Some(a), Some(b)) if a + 6 <= b => {
                let inner = &full[a + 6..b];
                inner
                    .strip_prefix(before.as_str())
                    .unwrap_or(inner)
                    .to_owned()
            }
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
pub(super) async fn board_page(
    state: AppState,
    p: openlens_meta::Project,
    f: openlens_engine::Filters,
    map: Option<(openlens_featuremap::FeatureMap, i64)>,
    host: String,
    admin: bool,
) -> Response {
    let (board, revenue) =
        match crate::boards::with_revenue(&state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v)))
            .await
        {
            Ok((b, r)) => (Ok(b), r.ok().flatten()),
            Err(e) => (Err(e), None),
        };
    render_board(&state, &p, &f, &host, admin, board, revenue.as_ref())
}

/// The board page of a board already computed: by [`board_page`], or by
/// a background job (#515).
pub(crate) fn render_board(
    state: &AppState,
    p: &openlens_meta::Project,
    f: &openlens_engine::Filters,
    host: &str,
    admin: bool,
    mut board: openlens_engine::Result<openlens_engine::Board>,
    revenue: Option<&openlens_engine::Revenue>,
) -> Response {
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    // The status strip (#614): the app's state and open incidents, and
    // for admins the setup steps; read from the metadata, no query.
    if let Ok(b) = board.as_mut() {
        // Stored rows only (goals, tickets, flags), no query (#617).
        b.in_flight = openlens_engine::commands::in_flight::in_flight(&state.meta, &p.id, b);
        if admin {
            b.setup = Some(crate::settings::setup_count(state, &p.id));
        }
        b.alerts = Some(openlens_engine::commands::alerts::status(
            &state.meta,
            &p.id,
        ));
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
    // The marks cover the active-users chart: from the window's start to
    // now (D-055; a range may end in the past, D-060).
    let days = board
        .as_ref()
        .ok()
        .and_then(|b| b.window.as_ref())
        .map_or(14, |w| {
            chrono::NaiveDateTime::parse_from_str(&w.since, "%Y-%m-%d %H:%M").map_or(w.days, |t| {
                (chrono::Utc::now() - t.and_utc()).num_days() + 1
            })
        });
    let marks = crate::timeline::collect(state, &p.id, &releases, days);
    match board {
        Ok(b) => html(
            StatusCode::OK,
            openlens_ui::board(
                &info,
                &b,
                f,
                host,
                &crate::reword::plain(state, &b.insights)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| (p.title, p.next_step))
                    .collect::<Vec<_>>(),
                revenue,
                &marks,
            ),
        ),
        Err(e) => html(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e.to_string()),
        ),
    }
}
