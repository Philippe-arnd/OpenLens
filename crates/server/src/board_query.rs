//! The board's query string (D-055, D-060), shared by the board page and
//! `GET /api/projects/{id}/board`, and the confirmation a long window
//! needs before it runs (#514, D-061): a date range or a past release
//! that reads more than 30 days of events answers with its estimate (a
//! page whose "Run it" starts a background job (#515), or a 409 from the
//! API) until it is asked again with `confirm=1` or run as a job.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use openlens_engine::commands::board::{EstimateInput, GetEstimate};
use openlens_engine::commands::{Caller, Command};
use openlens_engine::{Estimate, Filters, Window};
use openlens_featuremap::FeatureMap;
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::api::FilterQuery;

/// The board's query: the filters, and `window=fixed` for the fixed 7-,
/// 14- and 30-day windows instead of the release window (D-055), or
/// `since=<release>` or `from=&to=` (`YYYY-MM-DD`, the project's days) for
/// a past release or a date range (D-060). The board page reads the same.
#[derive(Deserialize, Default)]
pub(crate) struct BoardQuery {
    #[serde(flatten)]
    pub(crate) f: FilterQuery,
    window: Option<String>,
    since: Option<String>,
    from: Option<String>,
    to: Option<String>,
    /// `1`: run a long window without asking again (D-061).
    confirm: Option<String>,
}

impl BoardQuery {
    /// The board's filters: the window asked for, else the release window
    /// unless `window=fixed`.
    pub(crate) fn filters(&self, tz: &str) -> Result<Filters, openlens_engine::EngineError> {
        let f = self.f.filters(tz);
        let asked = openlens_engine::Window::asked(
            self.since.as_deref(),
            self.from.as_deref(),
            self.to.as_deref(),
            &f.tz,
            chrono::Utc::now(),
        )?;
        Ok(Filters {
            window: asked.unwrap_or(match self.window.as_deref().map(str::trim) {
                Some("fixed") => openlens_engine::Window::Fixed,
                _ => openlens_engine::Window::Release,
            }),
            ..f
        })
    }

    /// Whether the query confirms a long window (`confirm=1`).
    pub(crate) fn confirmed(&self) -> bool {
        matches!(
            self.confirm.as_deref().map(str::trim),
            Some("1" | "true" | "yes")
        )
    }

    /// The input of `board.estimate` for this query in `project`.
    pub(crate) fn estimate_input(&self, project: &str) -> EstimateInput {
        EstimateInput {
            project: project.to_owned(),
            environment: self.f.env.clone(),
            role: self.f.role.clone(),
            release: self.f.release.clone(),
            internal: matches!(self.f.internal.as_deref(), Some("1" | "true" | "on")),
            since: self.since.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
        }
    }

    /// The board's own parameters in this query, `confirm` left out: the
    /// hidden fields of the confirm page's "Run it" form, which starts a
    /// background job (#515).
    pub(crate) fn params(&self) -> Vec<(&'static str, String)> {
        let f = &self.f;
        let pairs = [
            ("env", &f.env),
            ("role", &f.role),
            ("release", &f.release),
            ("internal", &f.internal),
            ("tz", &f.tz),
            ("since", &self.since),
            ("from", &self.from),
            ("to", &self.to),
        ];
        pairs
            .iter()
            .filter_map(|(k, v)| {
                let v = v.as_deref().filter(|v| !v.is_empty())?;
                Some((*k, v.to_owned()))
            })
            .collect()
    }
}

/// The estimate of the window `f` when it must be confirmed first: a date
/// range or a past release, not confirmed by `q`, not already computed
/// (a cached board reads nothing), reading more than 30 days of events.
/// The estimate is the `board.estimate` command's.
///
/// # Errors
/// Why the estimate failed, for the person (the board would fail alike).
pub(crate) async fn unconfirmed(
    state: &AppState,
    project: &str,
    q: &BoardQuery,
    f: &Filters,
    map: Option<(&FeatureMap, i64)>,
) -> Result<Option<Estimate>, String> {
    if q.confirmed()
        || !matches!(f.window, Window::Range { .. } | Window::Since { .. })
        || crate::boards::warm(state, project, f, map)
    {
        return Ok(None);
    }
    let caller = Caller {
        tz: Some(f.tz.clone()),
        ..Caller::default()
    };
    match GetEstimate::run(state, &caller, q.estimate_input(project)).await {
        Ok(e) => Ok(e.filter(|e| e.confirm)),
        Err(e) => Err(e.to_string()),
    }
}

/// The API's answer to a window that needs confirming: 409, with the
/// estimate and how to run it anyway.
pub(crate) fn conflict(e: &Estimate) -> Response {
    let message = format!(
        "{}: {} days read from the events ({} MB, roughly {:.0} s); ask again with confirm=1 to run it",
        e.label,
        e.raw_days,
        e.bytes / 1_000_000,
        e.secs.ceil()
    );
    (
        StatusCode::CONFLICT,
        axum::Json(json!({
            "code": "conflict",
            "message": message,
            "error": message,
            "estimate": e,
        })),
    )
        .into_response()
}

/// `GET /api/projects/{id}/board/estimate`: what the board's window would
/// read, with the board's parameters (`board.estimate`): days from the
/// rollups and from the events, bytes, rows, a rough time, and whether it
/// needs `confirm=1`. `null` for the fixed windows.
pub(crate) async fn api_estimate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    axum::extract::Query(q): axum::extract::Query<BoardQuery>,
) -> Response {
    if let Some(r) = crate::api::guard(&state, &headers) {
        return r;
    }
    let p = match crate::api::known_project(&state, &project).await {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let f = match q.filters(&p.timezone) {
        Ok(f) => f,
        Err(e) => return crate::apierr::json(StatusCode::BAD_REQUEST, e),
    };
    if matches!(f.window, Window::Fixed) {
        return axum::Json(serde_json::Value::Null).into_response();
    }
    let caller = Caller {
        tz: Some(f.tz),
        ..Caller::default()
    };
    match GetEstimate::run(&state, &caller, q.estimate_input(&p.id)).await {
        Ok(e) => axum::Json(e).into_response(),
        Err(openlens_engine::commands::Error::Invalid(m)) => {
            crate::apierr::json(StatusCode::BAD_REQUEST, m)
        }
        Err(e) => crate::apierr::json(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}
