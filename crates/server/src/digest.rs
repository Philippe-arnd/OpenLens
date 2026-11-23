//! Weekly digest: `GET /api/projects/{id}/digest` (JSON + Markdown), the
//! dashboard page, and the optional Monday email (D-016).
//!
//! Email is sent only when `OPENLENS_SMTP_URL` and `OPENLENS_DIGEST_TO` are
//! set, e.g. `smtps://user:pass@smtp.example.com:465` (or `smtp://…:587`,
//! STARTTLS) and `pm@example.com,ceo@example.com`. `OPENLENS_DIGEST_FROM`
//! defaults to the first recipient. One email per project with events, on
//! Mondays from 07:00 UTC, at most once a week.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{Datelike, Timelike, Utc};
use serde_json::json;

use crate::{AppState, auth};

/// The digest of a project, computed now.
pub(crate) async fn compute(
    state: &AppState,
    project: &openlens_meta::Project,
    base: &str,
) -> Result<(openlens_engine::Digest, String), openlens_engine::EngineError> {
    let map = crate::load_map(state, &project.id).await;
    let mut d = openlens_engine::digest(
        &state.query,
        &project.id,
        map.as_ref().map(|(m, v)| (m, *v)),
    )
    .await?;
    let (at_risk, goals) = crate::goals::snapshot(state, &project.id);
    d.goals = goals;
    d.objectives_at_risk = at_risk;
    let md =
        openlens_engine::digest_markdown(&project.name, &format!("{base}/p/{}", project.id), &d);
    Ok((d, md))
}

/// `GET /api/projects/{id}/digest`.
pub(crate) async fn api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "admin bearer token required" })),
        )
            .into_response();
    }
    let meta = state.meta.clone();
    let Ok(Ok(Some(p))) = tokio::task::spawn_blocking(move || meta.project(&id)).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "unknown project" })),
        )
            .into_response();
    };
    match compute(&state, &p, &crate::base_url(&headers)).await {
        Ok((d, md)) => Json(json!({ "digest": d, "markdown": md })).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Email settings, read from the environment (all optional).
struct Mail {
    url: String,
    to: Vec<String>,
    from: String,
}

fn mail_settings() -> Option<Mail> {
    let url = std::env::var("OPENLENS_SMTP_URL")
        .ok()
        .filter(|s| !s.is_empty())?;
    let to: Vec<String> = std::env::var("OPENLENS_DIGEST_TO")
        .ok()?
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| s.contains('@'))
        .collect();
    let from = std::env::var("OPENLENS_DIGEST_FROM")
        .ok()
        .filter(|s| s.contains('@'))
        .or_else(|| to.first().cloned())?;
    (!to.is_empty()).then_some(Mail { url, to, from })
}

/// Called by the background loop: send this week's digests when it is time.
pub(crate) async fn maybe_send(state: &AppState) {
    let Some(mail) = mail_settings() else { return };
    let now = Utc::now();
    if now.weekday() != chrono::Weekday::Mon || now.hour() < 7 {
        return;
    }
    let week = format!("{}-W{:02}", now.iso_week().year(), now.iso_week().week());
    let meta = state.meta.clone();
    let w = week.clone();
    let already = tokio::task::spawn_blocking(move || {
        meta.setting("digest_sent_week").ok().flatten() == Some(w)
    })
    .await
    .unwrap_or(true);
    if already {
        return;
    }
    let meta = state.meta.clone();
    let Ok(Ok(projects)) = tokio::task::spawn_blocking(move || meta.projects()).await else {
        return;
    };
    let base = std::env::var("OPENLENS_PUBLIC_URL")
        .or_else(|_| std::env::var("SERVICE_URL_OPENLENS"))
        .unwrap_or_else(|_| "http://localhost:8686".into());
    let mut sent = 0;
    for p in projects {
        let Ok((d, md)) = compute(state, &p, base.trim_end_matches('/')).await else {
            continue;
        };
        if d.users.now == 0 && d.users.before == 0 {
            continue;
        }
        let subject = format!("{}: your week ({} active users)", p.name, d.users.now);
        if let Err(err) = send(&mail, &subject, &md).await {
            tracing::warn!(project = %p.id, %err, "digest email failed");
        } else {
            sent += 1;
        }
    }
    let meta = state.meta.clone();
    let _ = tokio::task::spawn_blocking(move || meta.set_setting("digest_sent_week", &week)).await;
    tracing::info!(sent, "weekly digests sent");
}

async fn send(mail: &Mail, subject: &str, body: &str) -> Result<(), String> {
    send_mail(&mail.url, &mail.from, &mail.to, subject, body).await
}

/// SMTP URL and sender for sign-in emails, when email is configured: the
/// sender is `OPENLENS_DIGEST_FROM`, or the first digest recipient.
pub(crate) fn login_mail() -> Option<(String, String)> {
    let url = std::env::var("OPENLENS_SMTP_URL")
        .ok()
        .filter(|s| !s.is_empty())?;
    let from = std::env::var("OPENLENS_DIGEST_FROM")
        .ok()
        .filter(|s| s.contains('@'))
        .or_else(|| {
            std::env::var("OPENLENS_DIGEST_TO")
                .ok()?
                .split(',')
                .map(|s| s.trim().to_owned())
                .find(|s| s.contains('@'))
        })?;
    Some((url, from))
}

/// Send a plain-text email.
pub(crate) async fn send_mail(
    url: &str,
    from: &str,
    to: &[String],
    subject: &str,
    body: &str,
) -> Result<(), String> {
    use lettre::message::header::ContentType;
    use lettre::{Message, SmtpTransport, Transport};
    let mut b = Message::builder()
        .from(from.parse().map_err(|e| format!("from: {e}"))?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN);
    for t in to {
        b = b.to(t.parse().map_err(|e| format!("to: {e}"))?);
    }
    let msg = b.body(body.to_owned()).map_err(|e| e.to_string())?;
    let url = url.to_owned();
    tokio::task::spawn_blocking(move || {
        let transport = SmtpTransport::from_url(&url)
            .map_err(|e| e.to_string())?
            .build();
        transport.send(&msg).map(|_| ()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
