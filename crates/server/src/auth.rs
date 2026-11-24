//! Dashboard and API authentication.
//!
//! - API: `Authorization: Bearer <admin token>`.
//! - Dashboard: a session cookie `<expiry>.<user>.<hmac>` signed with the
//!   session key (HMAC-SHA256). `user` is `owner` (admin token, owner links)
//!   or a user id (D-016); a removed user's sessions stop working at once.
//!   The signature covers the session generation: *Sign out everywhere*
//!   (#342) bumps it, and every cookie issued before stops working.
//!   Stateless otherwise, and survives restarts.
//! - One-time login links (`/l/<token>`) for handing the board to a PM.

use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::{AppState, token_matches};

/// Cookie name.
pub(crate) const COOKIE: &str = "ol_session";
/// Session lifetime: 30 days.
pub(crate) const SESSION_SECS: i64 = 30 * 24 * 3600;

pub(crate) fn sign(key: &str, msg: &str) -> String {
    // HMAC accepts keys of any length, so this cannot fail.
    match Hmac::<Sha256>::new_from_slice(key.as_bytes()) {
        Ok(mut mac) => {
            mac.update(msg.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        }
        Err(_) => String::new(),
    }
}

/// The owner: whoever holds the admin token.
pub(crate) const OWNER: &str = "owner";

/// Who is looking at the dashboard.
#[derive(Debug, Clone)]
pub(crate) struct Viewer {
    /// Can change things (flags, users).
    pub admin: bool,
    /// Projects they may see (`None`: all).
    pub projects: Option<Vec<String>>,
    /// `OWNER` or the user's id.
    pub id: String,
    /// The time zone they read times in, if they chose one (#215).
    pub tz: Option<String>,
}

impl Viewer {
    /// May this viewer see `project`?
    pub(crate) fn can_see(&self, project: &str) -> bool {
        self.projects
            .as_ref()
            .is_none_or(|p| p.iter().any(|x| x == project))
    }
}

/// A fresh session cookie value for `user` (`OWNER` or a user id).
pub(crate) fn new_session(state: &AppState, user: &str) -> String {
    let exp = chrono::Utc::now().timestamp() + SESSION_SECS;
    format!(
        "{exp}.{user}.{}",
        sign(&state.session_key, &session_msg(state, exp, user))
    )
}

/// What a session cookie signs. Generation 0 (no sign-out yet) keeps the
/// format of earlier releases, so an upgrade logs nobody out.
fn session_msg(state: &AppState, exp: i64, user: &str) -> String {
    match state.session_gen.load(std::sync::atomic::Ordering::Relaxed) {
        0 => format!("session:{exp}:{user}"),
        g => format!("session:{g}:{exp}:{user}"),
    }
}

/// The user a session cookie belongs to, if it is valid.
fn session_user(state: &AppState, value: &str) -> Option<String> {
    let (exp, rest) = value.split_once('.')?;
    let exp_n = exp.parse::<i64>().ok()?;
    if exp_n <= chrono::Utc::now().timestamp() {
        return None;
    }
    // `<expiry>.<hmac>` cookies from before users existed are refused.
    let (user, sig) = rest.split_once('.')?;
    let ok = user.len() <= 120
        && token_matches(
            sig,
            &sign(&state.session_key, &session_msg(state, exp_n, user)),
        );
    ok.then(|| user.to_owned())
}

/// Who the session cookie belongs to (`OWNER` or a user id), if valid.
pub(crate) fn session_id(state: &AppState, headers: &HeaderMap) -> Option<String> {
    session_user(state, &cookie(headers, COOKIE)?)
}

/// The logged-in dashboard viewer, if any.
pub(crate) fn viewer(state: &AppState, headers: &HeaderMap) -> Option<Viewer> {
    let id = session_user(state, &cookie(headers, COOKIE)?)?;
    if id == OWNER {
        return Some(Viewer {
            admin: true,
            projects: None,
            tz: state.meta.setting("owner_timezone").ok().flatten(),
            id,
        });
    }
    let u = state.meta.user(&id).ok()??;
    if u.disabled {
        return None;
    }
    Some(Viewer {
        admin: u.role == "admin",
        projects: u.projects,
        id,
        tz: u.timezone,
    })
}

/// The time zone this request reads times and days in (#215, D-018): the
/// viewer's own choice, else the project's, else UTC.
pub(crate) fn tz_for(state: &AppState, headers: &HeaderMap, project_tz: &str) -> String {
    viewer(state, headers)
        .and_then(|v| v.tz)
        .filter(|t| t.parse::<chrono_tz::Tz>().is_ok())
        .unwrap_or_else(|| project_tz.to_owned())
}

pub(crate) fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all("cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
}

/// Is the request from a logged-in dashboard user?
pub(crate) fn has_session(state: &AppState, headers: &HeaderMap) -> bool {
    viewer(state, headers).is_some()
}

/// Does the request carry the admin bearer token, or a scoped API token
/// that [`crate::scopes::layer`] checked for this request?
pub(crate) fn has_bearer(state: &AppState, headers: &HeaderMap) -> bool {
    let admin = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| crate::admin_token_matches(state, t.trim()));
    admin
        || headers
            .get(crate::scopes::MARK)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| token_matches(v, &crate::scopes::mark(state)))
}

/// `Set-Cookie` header value for a session (Secure when served over HTTPS).
pub(crate) fn set_cookie(value: &str, headers: &HeaderMap, max_age: i64) -> String {
    let secure = crate::base_url(headers).starts_with("https://");
    format!(
        "{COOKIE}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    )
}
