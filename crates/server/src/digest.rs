//! Weekly digest: `GET /api/projects/{id}/digest` (JSON + Markdown), the
//! dashboard page, and the optional Monday email (D-016).
//!
//! Email is sent only when `OPENLENS_SMTP_URL` and `OPENLENS_DIGEST_TO` are
//! set, e.g. `smtps://user:pass@smtp.example.com:465` (or `smtp://…:587`,
//! STARTTLS) and `pm@example.com,ceo@example.com`. `OPENLENS_DIGEST_FROM`
//! defaults to the first recipient. On Mondays from 07:00 UTC, at most
//! once a week: one email when one project had users in the last two
//! weeks, and one email with a section per project, in the portfolio's
//! order, when several did (#717, D-118).
//!
//! `GET /api/digest` is the digest of every project the caller may see:
//! one project's digest when they see one, a section per project when
//! they see several.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{Datelike, Timelike, Utc};
use openlens_engine::commands::releases::{List, ListInput};
use openlens_engine::commands::{Caller, Command};
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
    // The week's production releases with their verdicts (#522, D-067);
    // the digest still goes out when they cannot be read.
    let input = ListInput {
        project: project.id.clone(),
        environment: Some("production".into()),
    };
    if let Ok(r) = List::run(state, &Caller::default(), input).await {
        d.releases = openlens_engine::release_lines(&r, Utc::now());
    }
    let md =
        openlens_engine::digest_markdown(&project.name, &format!("{base}/p/{}", project.id), &d);
    Ok((d, md))
}

/// The caller's projects in the portfolio's order (#717): what needs
/// attention first, then activity.
pub(crate) async fn ordered(
    state: &AppState,
    caller: &Caller,
) -> Result<Vec<openlens_meta::Project>, String> {
    use openlens_engine::commands::portfolio::{Get, GetInput};
    let rows = Get::run(state, caller, GetInput::default())
        .await
        .map_err(|e| e.to_string())?;
    let meta = state.meta.clone();
    let all = tokio::task::spawn_blocking(move || meta.projects())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    Ok(rows
        .projects
        .iter()
        .filter_map(|r| all.iter().find(|p| p.id == r.project).cloned())
        .collect())
}

/// The digest of several projects (#717), one section each in the order
/// given; a project whose digest fails says why in its section.
pub(crate) async fn compute_many(
    state: &AppState,
    projects: &[openlens_meta::Project],
    base: &str,
) -> (Vec<(String, openlens_engine::Digest)>, String) {
    let mut done = Vec::new();
    for p in projects {
        let d = compute(state, p, base).await.map(|(d, _)| d);
        done.push((p, d));
    }
    let urls: Vec<String> = done
        .iter()
        .map(|(p, _)| format!("{base}/p/{}", p.id))
        .collect();
    let sections: Vec<openlens_engine::DigestSection<'_>> = done
        .iter()
        .zip(&urls)
        .map(|((p, d), url)| openlens_engine::DigestSection {
            name: &p.name,
            board_url: url,
            digest: d.as_ref().map_err(ToString::to_string),
        })
        .collect();
    let md = openlens_engine::digests_markdown(&format!("{base}/portfolio"), &sections);
    let digests = done
        .into_iter()
        .filter_map(|(p, d)| Some((p.id.clone(), d.ok()?)))
        .collect();
    (digests, md)
}

/// `GET /api/digest` (#717): the digest of every project the caller may
/// see, one section per project in the portfolio's order; the project's
/// own digest when they see only one.
pub(crate) async fn api_all(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let caller = crate::commands::caller(&state, &headers);
    let projects = match ordered(&state, &caller).await {
        Ok(p) => p,
        Err(e) => return crate::apierr::json(StatusCode::SERVICE_UNAVAILABLE, e),
    };
    let base = crate::public_url(&headers);
    match projects.as_slice() {
        [] => crate::apierr::json(StatusCode::NOT_FOUND, "no project"),
        [p] => match compute(&state, p, &base).await {
            Ok((d, md)) => {
                Json(json!({ "project": p.id, "digest": d, "markdown": md })).into_response()
            }
            Err(e) => crate::apierr::json(StatusCode::BAD_REQUEST, e.to_string()),
        },
        many => {
            let (digests, md) = compute_many(&state, many, &base).await;
            let sections: Vec<serde_json::Value> = many
                .iter()
                .map(|p| {
                    let d = digests.iter().find(|(id, _)| *id == p.id).map(|(_, d)| d);
                    json!({
                        "project": p.id,
                        "name": p.name,
                        "digest": d,
                        "available": d.is_some(),
                    })
                })
                .collect();
            Json(json!({ "projects": sections, "markdown": md })).into_response()
        }
    }
}

/// `GET /api/projects/{id}/digest`.
pub(crate) async fn api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !auth::has_bearer(&state, &headers) {
        return crate::apierr::json(StatusCode::UNAUTHORIZED, "admin bearer token required");
    }
    let meta = state.meta.clone();
    let Ok(Ok(Some(p))) = tokio::task::spawn_blocking(move || meta.project(&id)).await else {
        return crate::apierr::json(StatusCode::NOT_FOUND, "unknown project");
    };
    match compute(&state, &p, &crate::public_url(&headers)).await {
        Ok((d, md)) => Json(json!({ "digest": d, "markdown": md })).into_response(),
        Err(e) => crate::apierr::json(StatusCode::BAD_REQUEST, e.to_string()),
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
    let base = base.trim_end_matches('/');
    // The recipients are the install's (`OPENLENS_DIGEST_TO`), who got
    // every project's digest before: the portfolio of every project.
    let projects = ordered(state, &Caller::default()).await.unwrap_or(projects);
    let mut active = Vec::new();
    for p in projects {
        let Ok((d, md)) = compute(state, &p, base).await else {
            continue;
        };
        if d.users.now == 0 && d.users.before == 0 {
            continue;
        }
        active.push((p, d, md));
    }
    let mut sent = 0;
    match active.as_slice() {
        [] => {}
        [(p, d, md)] => {
            let subject = format!("{}: your week ({} active users)", p.name, d.users.now);
            if let Err(err) = send(&mail, &subject, md).await {
                tracing::warn!(project = %p.id, %err, "digest email failed");
            } else {
                sent += 1;
            }
        }
        many => {
            // One email, a section per project, in the portfolio's order
            // (#717, D-118).
            let urls: Vec<String> = many
                .iter()
                .map(|(p, _, _)| format!("{base}/p/{}", p.id))
                .collect();
            let sections: Vec<openlens_engine::DigestSection<'_>> = many
                .iter()
                .zip(&urls)
                .map(|((p, d, _), url)| openlens_engine::DigestSection {
                    name: &p.name,
                    board_url: url,
                    digest: Ok(d),
                })
                .collect();
            let md = openlens_engine::digests_markdown(&format!("{base}/portfolio"), &sections);
            let users: i64 = many.iter().map(|(_, d, _)| d.users.now).sum();
            let subject = format!(
                "Your week across {} projects ({users} active users)",
                many.len()
            );
            if let Err(err) = send(&mail, &subject, &md).await {
                tracing::warn!(%err, "digest email failed");
            } else {
                sent += 1;
            }
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
        // A server that accepts and never answers gives up after 30 s,
        // instead of holding a blocking thread for lettre's default (#311).
        let transport = SmtpTransport::from_url(&url)
            .map_err(|e| e.to_string())?
            .timeout(Some(std::time::Duration::from_secs(30)))
            .build();
        transport.send(&msg).map(|_| ()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
