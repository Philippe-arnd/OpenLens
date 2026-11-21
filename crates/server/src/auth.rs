//! Dashboard and API authentication.
//!
//! - API: `Authorization: Bearer <admin token>`.
//! - Dashboard: a session cookie `<expiry>.<hmac>` signed with the admin
//!   token (HMAC-SHA256). Stateless, survives restarts, and rotating the
//!   admin token logs everyone out.
//! - One-time login links (`/l/<token>`) for handing the board to a PM.

use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::{AppState, token_matches};

/// Cookie name.
pub(crate) const COOKIE: &str = "ol_session";
/// Session lifetime: 30 days.
pub(crate) const SESSION_SECS: i64 = 30 * 24 * 3600;

fn sign(key: &str, msg: &str) -> String {
    // HMAC accepts keys of any length, so this cannot fail.
    match Hmac::<Sha256>::new_from_slice(key.as_bytes()) {
        Ok(mut mac) => {
            mac.update(msg.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        }
        Err(_) => String::new(),
    }
}

/// A fresh session cookie value.
pub(crate) fn new_session(state: &AppState) -> String {
    let exp = chrono::Utc::now().timestamp() + SESSION_SECS;
    format!(
        "{exp}.{}",
        sign(&state.admin_token, &format!("session:{exp}"))
    )
}

fn valid_session(state: &AppState, value: &str) -> bool {
    let Some((exp, sig)) = value.split_once('.') else {
        return false;
    };
    let Ok(exp_n) = exp.parse::<i64>() else {
        return false;
    };
    exp_n > chrono::Utc::now().timestamp()
        && token_matches(sig, &sign(&state.admin_token, &format!("session:{exp}")))
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
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
    cookie(headers, COOKIE).is_some_and(|v| valid_session(state, &v))
}

/// Does the request carry the admin bearer token?
pub(crate) fn has_bearer(state: &AppState, headers: &HeaderMap) -> bool {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| token_matches(t.trim(), &state.admin_token))
}

/// `Set-Cookie` header value for a session (Secure when served over HTTPS).
pub(crate) fn set_cookie(value: &str, headers: &HeaderMap, max_age: i64) -> String {
    let secure = crate::base_url(headers).starts_with("https://");
    format!(
        "{COOKIE}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    )
}
