//! People and data explorer (#222): a person's profile, a breakdown page,
//! and read-only SQL for admins. The page, export and REST adapters of the
//! engine's `explore.*` commands (#507, D-042), which hold the logic and
//! the SQL (`openlens_engine::commands::explore`).

use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use openlens_engine::commands::explore::{self as cmd, FilteredInput};
use openlens_engine::commands::{Caller, Command, Error, Host};
use serde::Deserialize;

use crate::AppState;
use crate::api::FilterQuery;
use crate::auth;
use crate::web::html_page;

async fn project(state: &AppState, id: &str) -> Option<openlens_meta::Project> {
    state.project(id).await
}

/// The input of a report over `f`.
fn filtered(p: &openlens_meta::Project, f: &openlens_engine::Filters) -> FilteredInput {
    FilteredInput {
        project: p.id.clone(),
        filters: f.clone(),
    }
}

/// The page shown when a report cannot be computed.
fn cant_show(e: &str) -> Response {
    html_page(
        StatusCode::BAD_REQUEST,
        openlens_ui::message("Can't show that", e),
    )
}

fn not_found(what: &str) -> Response {
    html_page(
        StatusCode::NOT_FOUND,
        openlens_ui::message("Not found", what),
    )
}

/// `GET /p/{id}/people/{distinct_id}`.
pub(crate) async fn person(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, who)): Path<(String, String)>,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let input = cmd::PersonInput {
        project: p.id.clone(),
        distinct_id: who.clone(),
    };
    let prof = match cmd::GetPerson::run(&state, &Caller::default(), input).await {
        Ok(prof) => prof,
        Err(Error::NotFound) => return not_found("No such project."),
        Err(e) => return not_found(&e.to_string()),
    };
    let tz = auth::tz_for(&state, &headers, &p.timezone);
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    html_page(
        StatusCode::OK,
        openlens_ui::person(
            &info,
            &who,
            prof.person.as_ref(),
            &prof.roles,
            &prof.replays,
            &prof.events,
            &tz,
            auth::viewer(&state, &headers).is_some_and(|v| v.admin),
        ),
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
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let Ok(q) = serde_urlencoded::from_str::<BreakdownQuery>(&raw.unwrap_or_default()) else {
        return not_found("Unreadable filters.");
    };
    let by = q.by.unwrap_or_else(|| "role".into());
    let f = q.f.filters(&auth::tz_for(&state, &headers, &p.timezone));
    let map = crate::load_map(&state, &p.id).await;
    let input = cmd::BreakdownInput {
        project: p.id.clone(),
        filters: f.clone(),
        by: by.clone(),
    };
    let caller = Caller::default();
    let (b, rows) = tokio::join!(
        crate::boards::board(&state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))),
        cmd::Breakdown::run(&state, &caller, input)
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
        (Err(e), _) => cant_show(&e.to_string()),
        (_, Err(e)) => cant_show(&e.to_string()),
    }
}

/// `GET /p/{id}/sql`: the query box and the schema (admins).
pub(crate) async fn sql_page(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    html_page(
        StatusCode::OK,
        openlens_ui::sql(&info, cmd::DEFAULT_SQL, None, None),
    )
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
    _: crate::access::ProjectWrite,
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(f): Form<SqlForm>,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let input = cmd::SqlInput {
        project: p.id.clone(),
        sql: f.sql.clone(),
    };
    match cmd::Sql::run(&state, &Caller::default(), input).await {
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

#[derive(Deserialize)]
pub(crate) struct AcqParams {
    #[serde(flatten)]
    f: FilterQuery,
    by: Option<String>,
    touch: Option<String>,
}

/// `GET /p/{id}/acquisition?by=channel|source|campaign|referrer&touch=first|last` (#226).
pub(crate) async fn acquisition(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let Ok(q) = serde_urlencoded::from_str::<AcqParams>(&raw.unwrap_or_default()) else {
        return not_found("Unreadable filters.");
    };
    let f = q.f.filters(&auth::tz_for(&state, &headers, &p.timezone));
    let map = crate::load_map(&state, &p.id).await;
    let input = cmd::AcquisitionInput {
        project: p.id.clone(),
        filters: f.clone(),
        by: q.by,
        touch: q.touch,
    };
    let caller = Caller::default();
    let (b, a) = tokio::join!(
        crate::boards::board(&state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))),
        cmd::GetAcquisition::run(&state, &caller, input)
    );
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    match (b, a) {
        (Ok(b), Ok(a)) => html_page(
            StatusCode::OK,
            openlens_ui::acquisition(&info, &b, &f, &a.rows, a.by, a.last),
        ),
        (Err(e), _) => cant_show(&e.to_string()),
        (_, Err(e)) => cant_show(&e.to_string()),
    }
}

/// `GET /p/{id}/forms` (#232).
pub(crate) async fn forms(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let Ok(q) = serde_urlencoded::from_str::<FilterQuery>(&raw.unwrap_or_default()) else {
        return not_found("Unreadable filters.");
    };
    let f = q.filters(&auth::tz_for(&state, &headers, &p.timezone));
    let map = crate::load_map(&state, &p.id).await;
    let caller = Caller::default();
    let (b, list) = tokio::join!(
        crate::boards::board(&state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))),
        cmd::Forms::run(&state, &caller, filtered(&p, &f))
    );
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    match (b, list) {
        (Ok(b), Ok(list)) => html_page(StatusCode::OK, openlens_ui::forms(&info, &b, &f, &list)),
        (Err(e), _) => cant_show(&e.to_string()),
        (_, Err(e)) => cant_show(&e.to_string()),
    }
}

/// `GET /p/{id}/revenue` (#231).
pub(crate) async fn revenue(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let Ok(q) = serde_urlencoded::from_str::<FilterQuery>(&raw.unwrap_or_default()) else {
        return not_found("Unreadable filters.");
    };
    let f = q.filters(&auth::tz_for(&state, &headers, &p.timezone));
    let map = crate::load_map(&state, &p.id).await;
    let found =
        crate::boards::with_revenue(&state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))).await;
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let found = found
        .map_err(|e| e.to_string())
        .and_then(|(b, r)| Ok((b, r?)));
    match found {
        Ok((b, r)) => html_page(
            StatusCode::OK,
            openlens_ui::revenue(&info, &b, &f, r.as_ref()),
        ),
        Err(e) => html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't show that", &e),
        ),
    }
}

/// `GET /p/{id}/churn` (#247).
pub(crate) async fn churn(
    crate::access::Signed(viewer): crate::access::Signed,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let Ok(q) = serde_urlencoded::from_str::<FilterQuery>(&raw.unwrap_or_default()) else {
        return not_found("Unreadable filters.");
    };
    let f = q.filters(&auth::tz_for(&state, &headers, &p.timezone));
    let map = crate::load_map(&state, &p.id).await;
    let input = cmd::ChurnInput {
        project: p.id.clone(),
        filters: Some(f.clone()),
    };
    let caller = Caller::default();
    let (b, c) = tokio::join!(
        crate::boards::board(&state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))),
        cmd::Churn::run(&state, &caller, input)
    );
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    let churn_event = map.as_ref().and_then(|(m, _)| m.churn_event.clone());
    let extras = openlens_ui::ChurnExtras {
        admin: viewer.admin,
        exit: exit_answers(&state, &p.id, churn_event.as_deref()).await,
        audiences: audiences(&state, &p.id).await,
        churn_event,
    };
    match (b, c) {
        (Ok(b), Ok(c)) => html_page(
            StatusCode::OK,
            openlens_ui::churn(&info, &b, &f, &c, &extras),
        ),
        (Err(e), _) => cant_show(&e.to_string()),
        (_, Err(e)) => cant_show(&e.to_string()),
    }
}

/// Answers of the surveys shown on the map's `churn_event` (an exit
/// survey, #269).
async fn exit_answers(
    state: &AppState,
    project: &str,
    churn_event: Option<&str>,
) -> Vec<openlens_engine::SurveyResults> {
    let Some(ev) = churn_event else {
        return Vec::new();
    };
    let keys: Vec<String> = crate::surveys::load(state, project)
        .await
        .into_iter()
        .filter(|s| s.trigger == ev)
        .map(|s| s.key)
        .collect();
    if keys.is_empty() {
        return Vec::new();
    }
    crate::surveys::results(state, project)
        .await
        .into_iter()
        .filter(|r| keys.contains(&r.key))
        .collect()
}

/// Flags made from the users at risk (#269): how many of the treated and
/// of the held-out users were active in the last 7 days.
async fn audiences(state: &AppState, project: &str) -> Vec<openlens_ui::Audience> {
    let input = cmd::ProjectInput {
        project: project.to_owned(),
    };
    cmd::FlagAudiences::run(state, &Caller::default(), input)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|a| openlens_ui::Audience {
            flag: a.flag,
            enabled: a.enabled,
            treated: a.treated,
            treated_active: a.treated_active,
            held: a.held,
            held_active: a.held_active,
        })
        .collect()
}

/// `GET /p/{id}/churn/at-risk.csv` or `.json` (admins, #269): the users at
/// risk now, for a CRM or a mailing.
pub(crate) async fn at_risk_export(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, file)): Path<(String, String)>,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let input = cmd::ChurnInput {
        project: p.id.clone(),
        filters: None,
    };
    let Ok(c) = cmd::Churn::run(&state, &Caller::default(), input).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    crate::privacy::audit(
        &state,
        &headers,
        Some(&p.id),
        "churn.export",
        &format!("{} users at risk ({file})", c.at_risk.len()),
    );
    let (body, mime) = match file.as_str() {
        "at-risk.json" => (
            serde_json::to_string_pretty(&c.at_risk).unwrap_or_default(),
            "application/json",
        ),
        "at-risk.csv" => {
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
            let mut s =
                String::from("distinct_id,role,last_day,idle_days,recent_days,before_days\n");
            for a in &c.at_risk {
                s.push_str(&format!(
                    "{},{},{},{},{},{}\n",
                    cell(&a.distinct_id),
                    cell(a.role.as_deref().unwrap_or("")),
                    a.last_day,
                    a.idle_days,
                    a.recent_days,
                    a.before_days
                ));
            }
            (s, "text/csv; charset=utf-8")
        }
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let mut r = body.into_response();
    let h = r.headers_mut();
    if let Ok(v) = axum::http::HeaderValue::from_str(mime) {
        h.insert(axum::http::header::CONTENT_TYPE, v);
    }
    if let Ok(v) = axum::http::HeaderValue::from_str(&format!(
        "attachment; filename=\"openlens-{}-{file}\"",
        p.id
    )) {
        h.insert(axum::http::header::CONTENT_DISPOSITION, v);
    }
    r
}

#[derive(serde::Deserialize)]
pub(crate) struct AudienceForm {
    #[serde(default)]
    holdout: Option<u8>,
}

/// `POST /p/{id}/churn/audience` (admins, #269): a flag, off, listing the
/// users at risk now with a control group, for a win-back nudge behind
/// `openlens.isEnabled(key)`.
pub(crate) async fn make_audience(
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Form(form): axum::extract::Form<AudienceForm>,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let input = cmd::MakeAudienceInput {
        project: p.id.clone(),
        holdout: form.holdout,
    };
    let key = match cmd::MakeAudience::run(&state, &Caller::default(), input).await {
        Ok(made) => made.key,
        Err(Error::NotFound) => return not_found("No such project."),
        Err(Error::Unavailable(e)) => return not_found(&e),
        Err(Error::Invalid(e)) => {
            return html_page(
                StatusCode::BAD_REQUEST,
                openlens_ui::message("Nobody at risk", &e),
            );
        }
        Err(Error::Storage(e)) => {
            return html_page(
                StatusCode::BAD_REQUEST,
                openlens_ui::message("Can't make it", &e),
            );
        }
    };
    crate::privacy::audit(&state, &headers, Some(&p.id), "flag.save", &key);
    Redirect::to(&format!("/p/{}/flags-edit?key={key}", p.id)).into_response()
}

/// `GET /api/projects/{id}/churn` (bearer): the same report as JSON.
pub(crate) async fn api_churn(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let input = cmd::ChurnInput {
        project: id,
        filters: None,
    };
    match cmd::Churn::run(&state, &Caller::default(), input).await {
        Ok(c) => axum::Json(c).into_response(),
        Err(Error::NotFound) => crate::apierr::json(StatusCode::NOT_FOUND, "unknown project"),
        Err(e) => crate::apierr::json(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

/// `GET /api/projects/{id}/vitals` (#408): every Web Vital explained, for
/// agents (the `vitals` MCP tool).
pub(crate) async fn api_vitals(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<crate::api::FilterQuery>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let Some(p) = project(&state, &id).await else {
        return crate::apierr::json(StatusCode::NOT_FOUND, "unknown project");
    };
    let f = q.filters(&p.timezone);
    match cmd::Vitals::run(&state, &Caller::default(), filtered(&p, &f)).await {
        Ok(v) => axum::Json(serde_json::json!({ "filters": f, "vitals": v })).into_response(),
        Err(Error::NotFound) => crate::apierr::json(StatusCode::NOT_FOUND, "unknown project"),
        Err(e) => crate::apierr::json(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

/// `GET /p/{id}/audience` (#225).
pub(crate) async fn audience(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let Ok(q) = serde_urlencoded::from_str::<FilterQuery>(&raw.unwrap_or_default()) else {
        return not_found("Unreadable filters.");
    };
    crate::audience::init(&state.data_dir);
    let f = q.filters(&auth::tz_for(&state, &headers, &p.timezone));
    let map = crate::load_map(&state, &p.id).await;
    let caller = Caller::default();
    let (b, a) = tokio::join!(
        crate::boards::board(&state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))),
        cmd::GetAudience::run(&state, &caller, filtered(&p, &f))
    );
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    match (b, a) {
        (Ok(b), Ok(a)) => html_page(
            StatusCode::OK,
            openlens_ui::audience(&info, &b, &f, &a, crate::audience::has_geo()),
        ),
        (Err(e), _) => cant_show(&e.to_string()),
        (_, Err(e)) => cant_show(&e.to_string()),
    }
}

#[derive(Deserialize)]
pub(crate) struct FlowParams {
    #[serde(flatten)]
    f: FilterQuery,
    from: Option<String>,
    to: Option<String>,
}

fn flows_input(
    p: &openlens_meta::Project,
    f: &openlens_engine::Filters,
    q: &FlowParams,
) -> cmd::FlowsInput {
    cmd::FlowsInput {
        project: p.id.clone(),
        filters: f.clone(),
        from: q.from.clone(),
        to: q.to.clone(),
    }
}

/// `GET /p/{id}/flows[?from=<feature>|to=success]` (#224).
pub(crate) async fn flows(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let Ok(q) = serde_urlencoded::from_str::<FlowParams>(&raw.unwrap_or_default()) else {
        return not_found("Unreadable filters.");
    };
    let f = q.f.filters(&auth::tz_for(&state, &headers, &p.timezone));
    let fq = cmd::flow_query(q.from.as_deref(), q.to.as_deref());
    let map = crate::load_map(&state, &p.id).await;
    let caller = Caller::default();
    let (b, flow) = tokio::join!(
        crate::boards::board(&state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v))),
        cmd::Flows::run(&state, &caller, flows_input(&p, &f, &q))
    );
    let features: Vec<(String, String)> = map
        .as_ref()
        .map(|(m, _)| {
            m.features
                .iter()
                .map(|x| (x.key.clone(), x.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    match (b, flow) {
        (Ok(b), Ok(flow)) => html_page(
            StatusCode::OK,
            openlens_ui::flows(
                &info,
                &b,
                &f,
                &flow,
                &features,
                fq.from.as_deref(),
                fq.to_success,
            ),
        ),
        (Err(e), _) => cant_show(&e.to_string()),
        (_, Err(e)) => cant_show(&e.to_string()),
    }
}

/// `GET /api/projects/{id}/flows[?from=&to=success]` (bearer): the same
/// flow as JSON, for the CLI and agents.
pub(crate) async fn api_flows(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let Some(p) = project(&state, &id).await else {
        return crate::apierr::json(StatusCode::NOT_FOUND, "unknown project");
    };
    let Ok(q) = serde_urlencoded::from_str::<FlowParams>(&raw.unwrap_or_default()) else {
        return crate::apierr::json(StatusCode::BAD_REQUEST, "bad query");
    };
    let f = q.f.filters(&p.timezone);
    match cmd::Flows::run(&state, &Caller::default(), flows_input(&p, &f, &q)).await {
        Ok(flow) => axum::Json(flow).into_response(),
        Err(Error::NotFound) => crate::apierr::json(StatusCode::NOT_FOUND, "unknown project"),
        Err(e) => crate::apierr::json(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

/// `GET /p/{id}/live` (#228).
pub(crate) async fn live(
    _: crate::access::Session,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Response {
    let Some(p) = project(&state, &id).await else {
        return not_found("No such project.");
    };
    let raw = raw.unwrap_or_default();
    let Ok(q) = serde_urlencoded::from_str::<FilterQuery>(&raw) else {
        return not_found("Unreadable filters.");
    };
    let f = q.filters(&auth::tz_for(&state, &headers, &p.timezone));
    let info = openlens_ui::ProjectInfo {
        id: &p.id,
        name: &p.name,
        key: &p.api_key,
    };
    // Keep the filters across refreshes (re-encoded, so nothing else rides along).
    let keep = serde_urlencoded::from_str::<Vec<(String, String)>>(&raw)
        .unwrap_or_default()
        .into_iter()
        .filter(|(k, _)| ["env", "role", "release", "internal"].contains(&k.as_str()))
        .collect::<Vec<_>>();
    let query = if keep.is_empty() {
        String::new()
    } else {
        format!(
            "?{}",
            serde_urlencoded::to_string(&keep).unwrap_or_default()
        )
    };
    match cmd::GetLive::run(&state, &Caller::default(), filtered(&p, &f)).await {
        Ok(l) => {
            let mut r = html_page(StatusCode::OK, openlens_ui::live(&info, &f, &l));
            if let Ok(v) = HeaderValue::from_str(&format!("10;url=/p/{}/live{query}", p.id)) {
                r.headers_mut().insert(header::REFRESH, v);
            }
            r
        }
        Err(e) => cant_show(&e.to_string()),
    }
}
