//! Sign-in, sign-out, login links and the first-start welcome. Moved out of web.rs as is (#307).

use super::*;

pub(super) async fn login_page() -> Response {
    html(
        StatusCode::OK,
        openlens_ui::login(false, crate::oidc::enabled(), email_login().is_some()),
    )
}

#[derive(Deserialize)]
pub(super) struct LoginForm {
    token: String,
}

pub(super) async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<LoginForm>,
) -> Response {
    if crate::admin_token_matches(&state, f.token.trim()) {
        let c = auth::set_cookie(
            &auth::new_session(&state, auth::OWNER),
            &headers,
            auth::SESSION_SECS,
        );
        let to = after_owner_login(&state).await;
        return with_cookie(Redirect::to(to).into_response(), &c);
    }
    // Slow down guessing.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    html(
        StatusCode::UNAUTHORIZED,
        openlens_ui::login(true, crate::oidc::enabled(), email_login().is_some()),
    )
}

/// Sign-in links by email: needs SMTP and the public URL, which the link
/// is built from (never from the request's Host header, which a caller
/// controls). Returns (SMTP URL, sender, public URL).
pub(super) fn email_login() -> Option<(String, String, String)> {
    let (smtp, from) = crate::digest::login_mail()?;
    let base = std::env::var("OPENLENS_PUBLIC_URL")
        .or_else(|_| std::env::var("SERVICE_URL_OPENLENS"))
        .ok()
        .filter(|u| u.starts_with("https://") || u.starts_with("http://localhost"))?;
    Some((smtp, from, base.trim_end_matches('/').to_owned()))
}

/// At most one email per address every 10 minutes, and 30 an hour in all.
pub(super) fn email_allowed(state: &AppState, email: &str) -> bool {
    use std::time::{Duration, Instant};
    let now = Instant::now();
    let sent = &state.caches.login_emails;
    let lately = sent
        .get(&email.to_owned())
        .and_then(|v| v.last().copied())
        .is_some_and(|t| now.duration_since(t) < Duration::from_secs(600));
    if lately {
        return false;
    }
    // The empty key holds every send of the last hour.
    let ok = sent
        .update(String::new(), |all| {
            let mut all = all.unwrap_or_default();
            all.retain(|t| now.duration_since(*t) < Duration::from_secs(3600));
            if all.len() >= 30 {
                return (Some(all), false);
            }
            all.push(now);
            (Some(all), true)
        })
        .unwrap_or(false);
    if ok {
        sent.insert(email.to_owned(), vec![now]);
    }
    ok
}

#[derive(Deserialize)]
pub(super) struct EmailForm {
    email: String,
}

/// "Email me a login link": the same answer whether the address has an
/// account or not, and the email goes out in the background.
pub(super) async fn login_email(
    State(state): State<AppState>,
    Form(f): Form<EmailForm>,
) -> Response {
    let done = html(
        StatusCode::OK,
        openlens_ui::message(
            "Check your inbox",
            "If this address was invited, a login link is on its way. It works once, within 15 minutes.",
        ),
    );
    let Some((smtp, from, base)) = email_login() else {
        return done;
    };
    let email = f.email.trim().to_lowercase();
    if email.len() > 200 || !email.contains('@') {
        return done;
    }
    tokio::spawn(async move {
        let meta = state.meta.clone();
        let e = email.clone();
        let st = state.clone();
        // Only a real, active account counts against the sending budget, so
        // made-up addresses cannot use it up (review of v0.3.0).
        let link = tokio::task::spawn_blocking(move || {
            let u = meta.user_by_email(&e).ok()??;
            if u.disabled || !email_allowed(&st, &e) {
                return None;
            }
            meta.create_login_link_for(Some(&u.id), 15 * 60).ok()
        })
        .await
        .ok()
        .flatten();
        if let Some(token) = link {
            let body = format!(
                "Here is your OpenLens login link. It works once, within 15 minutes:\n\n{base}/l/{token}\n\nIf you did not ask for it, ignore this email.\n"
            );
            if let Err(err) =
                crate::digest::send_mail(&smtp, &from, &[email], "Your OpenLens login link", &body)
                    .await
            {
                tracing::warn!(%err, "login email failed");
            }
        }
    });
    done
}

pub(super) async fn logout() -> Response {
    let mut r = Redirect::to("/login").into_response();
    for c in auth::clear_cookies() {
        if let Ok(v) = HeaderValue::from_str(&c) {
            r.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    r
}

/// GET of a login link: a "Continue" page that leaves the link valid
/// (#315). It does not look the token up: an expired link shows "Link
/// expired" after the POST, and the GET tells nothing about a token.
pub(super) async fn login_link_page(Path(token): Path<String>) -> Response {
    if token.len() > 100
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return link_expired().await;
    }
    let mut r = html(
        StatusCode::OK,
        openlens_ui::login_continue(&format!("/l/{token}")),
    );
    let h = r.headers_mut();
    h.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    h.insert(
        axum::http::header::REFERRER_POLICY,
        axum::http::HeaderValue::from_static("no-referrer"),
    );
    r
}

pub(super) async fn link_expired() -> Response {
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    html(
        StatusCode::UNAUTHORIZED,
        openlens_ui::message(
            "Link expired",
            "This login link was already used or has expired. Ask your agent for a new one.",
        ),
    )
}

/// POST of a login link (the "Continue" button): uses it, once.
pub(super) async fn login_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    let meta = state.meta.clone();
    let who = match tokio::task::spawn_blocking(move || meta.consume_login_link(&token)).await {
        Ok(Ok(Some(user))) => Some(user.unwrap_or_else(|| auth::OWNER.to_owned())),
        _ => None,
    };
    let Some(who) = who else {
        return link_expired().await;
    };
    if who != auth::OWNER {
        let _ = state.meta.touch_sign_in(&who);
    }
    let c = auth::set_cookie(
        &auth::new_session(&state, &who),
        &headers,
        auth::SESSION_SECS,
    );
    let to = if who == auth::OWNER {
        after_owner_login(&state).await
    } else {
        "/"
    };
    with_cookie(Redirect::to(to).into_response(), &c)
}

/// Where the owner goes after logging in: the welcome page while nobody
/// has an account yet (the first setup), else the board.
pub(super) async fn after_owner_login(state: &AppState) -> &'static str {
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || meta.users()).await {
        Ok(Ok(users)) if users.is_empty() => "/welcome",
        _ => "/",
    }
}

/// The welcome page is for the owner session only (admin token or owner
/// link): a user already has an account.
pub(super) fn owner_only(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    match auth::session_id(state, headers) {
        Some(id) if id == auth::OWNER => None,
        Some(_) => Some(Redirect::to("/").into_response()),
        None => Some(to_login()),
    }
}

pub(super) async fn welcome_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(r) = owner_only(&state, &headers) {
        return r;
    }
    html(
        StatusCode::OK,
        openlens_ui::welcome(crate::oidc::enabled(), None),
    )
}

#[derive(Deserialize)]
pub(super) struct WelcomeForm {
    email: String,
}

/// Create the owner's admin account and continue the session as that user.
pub(super) async fn welcome_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<WelcomeForm>,
) -> Response {
    if let Some(r) = owner_only(&state, &headers) {
        return r;
    }
    let meta = state.meta.clone();
    match tokio::task::spawn_blocking(move || meta.upsert_user(&f.email, "admin", None)).await {
        Ok(Ok(u)) => {
            let c = auth::set_cookie(
                &auth::new_session(&state, &u.id),
                &headers,
                auth::SESSION_SECS,
            );
            with_cookie(Redirect::to("/").into_response(), &c)
        }
        Ok(Err(e)) => html(
            StatusCode::BAD_REQUEST,
            openlens_ui::welcome(crate::oidc::enabled(), Some(&e.to_string())),
        ),
        Err(_) => html(
            StatusCode::INTERNAL_SERVER_ERROR,
            openlens_ui::message("Oops", "Try again."),
        ),
    }
}
