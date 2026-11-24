//! Settings → API tokens and sessions (#342): list, create (the value is
//! shown once) and revoke scoped API tokens, and *Sign out everywhere*,
//! which refuses every session cookie issued so far. Admins only.

use std::sync::atomic::Ordering;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};

use crate::AppState;
use crate::auth;
use crate::web::html_page;

fn guard(state: &AppState, headers: &HeaderMap) -> Result<auth::Viewer, Response> {
    match auth::viewer(state, headers) {
        None => Err(Redirect::to("/login").into_response()),
        Some(v) if !v.admin => Err(html_page(
            StatusCode::FORBIDDEN,
            openlens_ui::message("Admins only", "API tokens and sessions are for admins."),
        )),
        Some(v) => Ok(v),
    }
}

async fn render(
    state: &AppState,
    created: Option<String>,
    notice: Option<&str>,
    error: Option<&str>,
) -> Response {
    let meta = state.meta.clone();
    let loaded = tokio::task::spawn_blocking(move || {
        Ok::<_, openlens_meta::MetaError>((meta.api_tokens()?, meta.projects()?))
    })
    .await;
    let Ok(Ok((tokens, projects))) = loaded else {
        return html_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            openlens_ui::message("Unavailable", "The metadata store is unavailable."),
        );
    };
    let view = openlens_ui::TokensView {
        tokens: tokens
            .into_iter()
            .map(|t| openlens_ui::TokenRow {
                id: t.id,
                label: t.label,
                scope: t.scope,
                projects: t.projects,
                created_at: t.created_at,
                last_used: t.last_used,
            })
            .collect(),
        projects: projects.into_iter().map(|p| (p.id, p.name)).collect(),
        created,
        notice: notice.map(str::to_owned),
        error: error.map(str::to_owned),
    };
    let mut r = html_page(StatusCode::OK, openlens_ui::tokens(&view));
    // The page can carry a fresh token: never cache it.
    r.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    r
}

/// `GET /settings/tokens`.
pub(crate) async fn page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = guard(&state, &headers) {
        return r;
    }
    render(&state, None, None, None).await
}

/// `POST /settings/tokens`: create one; its value is shown once.
pub(crate) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err(r) = guard(&state, &headers) {
        return r;
    }
    // One `project` field per ticked project (none: every project), which
    // a struct cannot take from a form: read the pairs.
    let pairs: Vec<(String, String)> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let (mut label, mut scope, mut picked) = (String::new(), String::new(), Vec::new());
    for (k, v) in pairs {
        match k.as_str() {
            "label" => label = v.trim().to_owned(),
            "scope" => scope = v,
            "project" if picked.len() < 100 => picked.push(v),
            _ => {}
        }
    }
    if label.is_empty() {
        return render(
            &state,
            None,
            None,
            Some("Give the token a label: what it is for."),
        )
        .await;
    }
    let projects = (!picked.is_empty()).then_some(picked);
    let meta = state.meta.clone();
    let made = tokio::task::spawn_blocking(move || {
        meta.create_api_token(&scope, projects.as_deref(), &label)
    })
    .await;
    match made {
        Ok(Ok((t, plain))) => {
            crate::privacy::audit(
                &state,
                &headers,
                None,
                "token.create",
                &format!("{} {}", t.id, t.scope),
            );
            render(&state, Some(plain), None, None).await
        }
        Ok(Err(e)) => render(&state, None, None, Some(&e.to_string())).await,
        Err(_) => render(&state, None, None, Some("The token could not be created.")).await,
    }
}

/// `POST /settings/tokens/{id}/revoke`.
pub(crate) async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = guard(&state, &headers) {
        return r;
    }
    let meta = state.meta.clone();
    let key = id.clone();
    match tokio::task::spawn_blocking(move || meta.revoke_api_token(&key)).await {
        Ok(Ok(true)) => {
            crate::privacy::audit(&state, &headers, None, "token.revoke", &id);
            render(&state, None, Some("Token revoked."), None).await
        }
        _ => render(&state, None, None, Some("No such token.")).await,
    }
}

/// `POST /settings/sessions/revoke`: every session cookie issued so far
/// stops working; the admin's own session is issued again so they stay in.
/// API tokens and the admin token keep working.
pub(crate) async fn sign_out_everywhere(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let v = match guard(&state, &headers) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let next = state.session_gen.load(Ordering::Relaxed).saturating_add(1);
    let meta = state.meta.clone();
    let saved = tokio::task::spawn_blocking(move || {
        meta.set_setting("session_generation", &next.to_string())
    })
    .await;
    if !matches!(saved, Ok(Ok(()))) {
        return render(
            &state,
            None,
            None,
            Some("Could not sign out the other sessions."),
        )
        .await;
    }
    state.session_gen.store(next, Ordering::Relaxed);
    crate::privacy::audit(&state, &headers, None, "sessions.revoke", &next.to_string());
    let cookie = auth::set_cookie(
        &auth::new_session(&state, &v.id),
        &headers,
        auth::SESSION_SECS,
    );
    let mut r = render(
        &state,
        None,
        Some("Every other session is signed out."),
        None,
    )
    .await;
    if let Ok(c) = axum::http::HeaderValue::from_str(&cookie) {
        r.headers_mut().append(axum::http::header::SET_COOKIE, c);
    }
    r
}

/// The generation in force, read once at start (`0` before any sign-out).
pub fn session_generation(meta: &openlens_meta::Meta) -> u64 {
    meta.setting("session_generation")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}
