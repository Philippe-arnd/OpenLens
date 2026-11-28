//! Read-only share links (#229): an admin makes a link to a project's
//! board or digest, with fixed filters, an expiry and optional embedding;
//! anyone with the link sees it without logging in. No SQL, people,
//! replays or settings are reachable from it.

use std::time::Duration;

use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::AppState;
use crate::web::html_page;

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
        // Plain http only for the machine itself, by exact host: a prefix
        // test let `http://localhost.example.com` through (review of v0.3.0).
        let local = o.strip_prefix("http://").is_some_and(|rest| {
            let host = rest.rsplit_once(':').map_or(rest, |(h, port)| {
                if port.bytes().all(|b| b.is_ascii_digit()) {
                    h
                } else {
                    rest
                }
            });
            host == "localhost" || host == "127.0.0.1"
        });
        let ok = (o.starts_with("https://") || local)
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
    _: crate::access::ProjectWrite,
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
            let url = format!("{}/s/{token}", crate::public_url(&headers));
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
    _: crate::access::ProjectWrite,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, lid)): Path<(String, String)>,
) -> Response {
    // Say so when nothing was revoked: a leaked link must never look
    // revoked while it still serves the board.
    match state.meta.revoke_share_link(&id, &lid) {
        Ok(true) => {
            state.caches.share_pages.clear();
            crate::privacy::audit(&state, &headers, Some(&id), "share.revoke", &lid);
            Redirect::to(&format!("/p/{id}/settings/sharing")).into_response()
        }
        Ok(false) => html_page(
            StatusCode::NOT_FOUND,
            openlens_ui::message(
                "Link not found",
                "No share link with this id exists in this project, so nothing was revoked.",
            ),
        ),
        Err(_) => html_page(
            StatusCode::SERVICE_UNAVAILABLE,
            openlens_ui::message(
                "Link not revoked",
                "The metadata store did not answer, so the link still works. Try again in a moment.",
            ),
        ),
    }
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
    // Rendered views are kept a few minutes (token hash → html).
    let cached = state.caches.share_pages.get(&key);
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
                match crate::digest::compute(&state, &p, &crate::public_url(&headers)).await {
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
                let Ok(mut b) =
                    crate::boards::board(&state, &p.id, &f, map.as_ref().map(|(m, v)| (m, *v)))
                        .await
                else {
                    return not_found();
                };
                // The status strip's App and Incidents cells (#614).
                b.alerts = Some(openlens_engine::commands::alerts::status(
                    &state.meta,
                    &p.id,
                ));
                b.in_flight =
                    openlens_engine::commands::in_flight::in_flight(&state.meta, &p.id, &b);
                openlens_ui::render_shared(|| openlens_ui::board(&info, &b, &f, "", &[], None, &[]))
            };
            let h = markup.into_string();
            state.caches.share_pages.insert(key, h.clone());
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
        for bad in [
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com",
            "http://example.com",
        ] {
            assert!(super::origins(bad).is_err(), "{bad}");
        }
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
