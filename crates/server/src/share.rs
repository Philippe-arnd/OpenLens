//! Read-only share links (#229): an admin makes a link to a project's
//! board or digest, with fixed filters, an expiry and optional embedding;
//! anyone with the link sees it without logging in. No SQL, people,
//! replays or settings are reachable from it.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::AppState;
use crate::web::html_page;

/// Rendered views, kept a few minutes (token hash → (when, html)).
static CACHE: Mutex<Option<HashMap<String, (Instant, String)>>> = Mutex::new(None);
const CACHE_FOR: Duration = Duration::from_secs(300);

#[derive(Deserialize)]
pub(crate) struct NewLink {
    scope: String,
    #[serde(default)]
    release: String,
    /// Days, or 0 for never.
    #[serde(default)]
    days: Option<i64>,
    /// Space- or comma-separated origins allowed to embed it.
    #[serde(default)]
    embed: String,
}

/// Origins for `frame-ancestors`: https only (http on localhost).
pub(crate) fn origins(raw: &str) -> Result<Option<String>, String> {
    let list: Vec<String> = raw
        .split([' ', ',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_end_matches('/').to_ascii_lowercase())
        .collect();
    if list.is_empty() {
        return Ok(None);
    }
    for o in &list {
        let ok = (o.starts_with("https://")
            || o.starts_with("http://localhost")
            || o.starts_with("http://127.0.0.1"))
            && crate::icon::origin(o).as_deref() == Some(o.as_str());
        if !ok {
            return Err(format!(
                "`{o}`: an origin like https://intranet.example.com (no path, no wildcard)"
            ));
        }
    }
    if list.len() > 10 {
        return Err("at most 10 origins".into());
    }
    Ok(Some(list.join(" ")))
}

/// `POST /p/{id}/settings/share` (admins).
pub(crate) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<NewLink>,
) -> Response {
    let oops = |m: &str| {
        html_page(
            StatusCode::BAD_REQUEST,
            openlens_ui::message("Can't make that link", m),
        )
    };
    if !crate::flags::same_origin(&headers) {
        return oops("Cross-site form.");
    }
    let embed = match origins(&f.embed) {
        Ok(e) => e,
        Err(e) => return oops(&e),
    };
    let days = f.days.unwrap_or(30);
    if !(0..=366).contains(&days) {
        return oops("Expiry: 1 to 366 days, or never.");
    }
    let expires = (days > 0).then(|| chrono::Utc::now().timestamp() + days * 86400);
    let release = f.release.trim();
    let actor = crate::privacy::actor(&state, &headers);
    match state.meta.create_share_link(
        &id,
        &f.scope,
        "production",
        (!release.is_empty()).then_some(release),
        embed.as_deref(),
        expires,
        &actor,
    ) {
        Ok((link, token)) => {
            crate::privacy::audit(
                &state,
                &headers,
                Some(&id),
                "share.create",
                &format!("{} link {}", link.scope, link.id),
            );
            let url = format!("{}/s/{token}", crate::base_url(&headers));
            html_page(
                StatusCode::OK,
                openlens_ui::message(
                    "Share link ready",
                    &format!(
                        "Copy it now, it is shown once: {url}{}",
                        if link.embed.is_some() {
                            " (it can be embedded in an iframe on the origins you gave)"
                        } else {
                            ""
                        }
                    ),
                ),
            )
        }
        Err(e) => oops(&e.to_string()),
    }
}

/// `POST /p/{id}/settings/share/{lid}/revoke` (admins).
pub(crate) async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, lid)): Path<(String, String)>,
) -> Response {
    if !crate::flags::same_origin(&headers) {
        return html_page(
            StatusCode::FORBIDDEN,
            openlens_ui::message("Refused", "Cross-site form."),
        );
    }
    let _ = state.meta.revoke_share_link(&id, &lid);
    if let Ok(mut c) = CACHE.lock() {
        *c = None;
    }
    crate::privacy::audit(&state, &headers, Some(&id), "share.revoke", &lid);
    Redirect::to(&format!("/p/{id}/settings#share")).into_response()
}

fn not_found() -> Response {
    html_page(
        StatusCode::NOT_FOUND,
        openlens_ui::message("Link not valid", "This share link expired or was revoked."),
    )
}

/// `GET /s/{token}`: the shared board or digest, no login.
pub(crate) async fn view(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    if token.len() > 80 || !token.starts_with("sh_") {
        return not_found();
    }
    let Ok(Some(link)) = state.meta.share_link(&token) else {
        tokio::time::sleep(Duration::from_millis(200)).await;
        return not_found();
    };
    let key = crate::short_hash(&token);
    let cached = CACHE.lock().ok().and_then(|c| {
        c.as_ref()
            .and_then(|m| m.get(&key))
            .filter(|(at, _)| at.elapsed() < CACHE_FOR)
            .map(|(_, h)| h.clone())
    });
    let body = match cached {
        Some(h) => h,
        None => {
            let Ok(Some(p)) = state.meta.project(&link.project) else {
                return not_found();
            };
            let info = openlens_ui::ProjectInfo {
                id: &p.id,
                name: &p.name,
                key: "",
            };
            let markup = if link.scope == "digest" {
                match crate::digest::compute(&state, &p, &crate::base_url(&headers)).await {
                    Ok((d, _)) => {
                        openlens_ui::render_shared(|| openlens_ui::digest(&info, &d, false))
                    }
                    Err(_) => return not_found(),
                }
            } else {
                let f = openlens_engine::Filters {
                    environment: link.environment.clone(),
                    release: link.release.clone(),
                    tz: p.timezone.clone(),
                    ..openlens_engine::Filters::default()
                };
                let map = crate::load_map(&state, &p.id).await;
                let Ok(b) = openlens_engine::board(
                    &state.query,
                    &p.id,
                    &f,
                    map.as_ref().map(|(m, v)| (m, *v)),
                )
                .await
                else {
                    return not_found();
                };
                openlens_ui::render_shared(|| openlens_ui::board(&info, &b, &f, "", &[], None, &[]))
            };
            let h = markup.into_string();
            if let Ok(mut c) = CACHE.lock() {
                let m = c.get_or_insert_with(HashMap::new);
                m.retain(|_, (at, _)| at.elapsed() < CACHE_FOR);
                m.insert(key, (Instant::now(), h.clone()));
            }
            h
        }
    };
    let mut r = (StatusCode::OK, axum::response::Html(body)).into_response();
    let h = r.headers_mut();
    let frame = link
        .embed
        .as_deref()
        .map_or_else(|| "'none'".to_owned(), str::to_owned);
    if let Ok(v) = HeaderValue::from_str(&format!(
        "default-src 'none'; style-src 'self'; img-src 'self' data:; form-action 'none'; frame-ancestors {frame}; base-uri 'none'"
    )) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    if link.embed.is_none() {
        h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    }
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=60"),
    );
    h.insert("x-robots-tag", HeaderValue::from_static("noindex"));
    r
}

#[cfg(test)]
mod tests {
    #[test]
    fn embed_origins_are_exact_and_https() {
        assert_eq!(super::origins("").unwrap(), None);
        assert_eq!(
            super::origins("https://intranet.example.com/, http://localhost:3000")
                .unwrap()
                .as_deref(),
            Some("https://intranet.example.com http://localhost:3000")
        );
        assert!(super::origins("http://intranet.example.com").is_err());
        assert!(super::origins("https://*.example.com").is_err());
        assert!(super::origins("https://example.com/path").is_err());
    }
}
