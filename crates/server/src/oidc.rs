//! Optional single sign-on with OpenID Connect (D-016).
//!
//! Off unless `OPENLENS_OIDC_ISSUER`, `OPENLENS_OIDC_CLIENT_ID` and
//! `OPENLENS_OIDC_CLIENT_SECRET` are set. The redirect URI to register at the
//! provider is `<public url>/oidc/callback`.
//!
//! Authorization code flow with PKCE (S256), `state` and `nonce` kept in a
//! short-lived cookie signed with the admin token. The ID token comes
//! straight from the token endpoint over TLS with the client secret, so, as
//! OpenID Connect Core 3.1.3.7 allows, TLS stands in for its signature; the
//! issuer, audience, expiry and nonce are still checked. Only people already
//! invited (`/users`) can sign in, matched on a verified email: SSO proves
//! who someone is, the team list says what they may see.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use rand::Rng;
use serde::Deserialize;
use sha2::Digest;

use crate::{AppState, auth};

const COOKIE: &str = "ol_oidc";
const FLOW_SECS: i64 = 600;

struct Config {
    issuer: String,
    client_id: String,
    client_secret: String,
}

fn config() -> Option<Config> {
    let get = |k: &str| {
        std::env::var(k)
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    Some(Config {
        issuer: get("OPENLENS_OIDC_ISSUER")?
            .trim_end_matches('/')
            .to_owned(),
        client_id: get("OPENLENS_OIDC_CLIENT_ID")?,
        client_secret: get("OPENLENS_OIDC_CLIENT_SECRET")?,
    })
}

/// Is SSO configured?
pub(crate) fn enabled() -> bool {
    config().is_some()
}

#[derive(Clone, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
}

static DISCOVERY: Mutex<Option<(Instant, String, Discovery)>> = Mutex::new(None);

/// The provider's endpoints, cached for an hour.
async fn discovery(cfg: &Config) -> Result<Discovery, String> {
    if let Ok(g) = DISCOVERY.lock()
        && let Some((at, issuer, d)) = g.as_ref()
        && *issuer == cfg.issuer
        && at.elapsed() < Duration::from_secs(3600)
    {
        return Ok(d.clone());
    }
    let url = format!("{}/.well-known/openid-configuration", cfg.issuer);
    let d: Discovery = tokio::task::spawn_blocking(move || -> Result<Discovery, String> {
        ureq::get(&url)
            .call()
            .map_err(|e| e.to_string())?
            .body_mut()
            .read_json()
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if !d.authorization_endpoint.starts_with("https://")
        || !d.token_endpoint.starts_with("https://")
    {
        return Err("the provider's endpoints must be https".into());
    }
    if let Ok(mut g) = DISCOVERY.lock() {
        *g = Some((Instant::now(), cfg.issuer.clone(), d.clone()));
    }
    Ok(d)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64url without padding.
pub(crate) fn b64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 2);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        let take = chunk.len() + 1;
        for i in 0..take {
            out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// Decode base64url (with or without padding).
pub(crate) fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.trim_end_matches('=').bytes() {
        let v = B64.iter().position(|&b| b == c).or(match c {
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        })? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

fn random(n: usize) -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::rng();
    (0..n)
        .map(|_| CHARS[rng.random_range(0..CHARS.len())] as char)
        .collect()
}

fn challenge(verifier: &str) -> String {
    b64url(&sha2::Sha256::digest(verifier.as_bytes()))
}

fn page(status: StatusCode, title: &str, text: &str) -> Response {
    crate::web::html_page(status, openlens_ui::message(title, text))
}

/// `GET /oidc/start`: off to the provider.
pub(crate) async fn start(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(cfg) = config() else {
        return page(
            StatusCode::NOT_FOUND,
            "Not available",
            "Single sign-on is not configured.",
        );
    };
    let d = match discovery(&cfg).await {
        Ok(d) => d,
        Err(err) => {
            tracing::warn!(%err, "OIDC discovery failed");
            return page(
                StatusCode::BAD_GATEWAY,
                "Sign-in unavailable",
                "The identity provider cannot be reached.",
            );
        }
    };
    let (st, nonce, verifier) = (random(32), random(32), random(64));
    let exp = chrono::Utc::now().timestamp() + FLOW_SECS;
    let payload = format!("{st}.{nonce}.{verifier}.{exp}");
    let sig = auth::sign(&state.admin_token, &format!("oidc:{payload}"));
    let redirect_uri = format!("{}/oidc/callback", crate::base_url(&headers));
    let url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={st}&nonce={nonce}&code_challenge={}&code_challenge_method=S256",
        d.authorization_endpoint,
        openlens_ui::url_enc(&cfg.client_id),
        openlens_ui::url_enc(&redirect_uri),
        openlens_ui::url_enc("openid email"),
        challenge(&verifier),
    );
    let secure = crate::base_url(&headers).starts_with("https://");
    let cookie = format!(
        "{COOKIE}={payload}.{sig}; Path=/oidc; HttpOnly; SameSite=Lax; Max-Age={FLOW_SECS}{}",
        if secure { "; Secure" } else { "" }
    );
    let mut r = Redirect::to(&url).into_response();
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        r.headers_mut().append(header::SET_COOKIE, v);
    }
    r
}

#[derive(Deserialize)]
pub(crate) struct Callback {
    code: Option<String>,
    state: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: serde_json::Value,
    exp: i64,
    nonce: Option<String>,
    email: Option<String>,
    email_verified: Option<bool>,
}

/// Check an ID token's claims; returns the verified email.
fn verified_email(
    id_token: &str,
    issuer: &str,
    client_id: &str,
    nonce: &str,
    now: i64,
) -> Result<String, &'static str> {
    let payload = id_token.split('.').nth(1).ok_or("malformed ID token")?;
    let claims: Claims =
        serde_json::from_slice(&b64url_decode(payload).ok_or("malformed ID token")?)
            .map_err(|_| "malformed ID token")?;
    if claims.iss.trim_end_matches('/') != issuer.trim_end_matches('/') {
        return Err("wrong issuer");
    }
    let aud_ok = match &claims.aud {
        serde_json::Value::String(a) => a == client_id,
        serde_json::Value::Array(a) => a.iter().any(|x| x.as_str() == Some(client_id)),
        _ => false,
    };
    if !aud_ok {
        return Err("wrong audience");
    }
    if claims.exp <= now {
        return Err("expired ID token");
    }
    if claims.nonce.as_deref() != Some(nonce) {
        return Err("wrong nonce");
    }
    if claims.email_verified == Some(false) {
        return Err("email not verified");
    }
    claims
        .email
        .filter(|e| e.contains('@'))
        .ok_or("no email in the ID token")
}

/// `GET /oidc/callback`: back from the provider.
pub(crate) async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Callback>,
) -> Response {
    let Some(cfg) = config() else {
        return page(
            StatusCode::NOT_FOUND,
            "Not available",
            "Single sign-on is not configured.",
        );
    };
    let fail = |why: &str| {
        tracing::info!(why, "OIDC sign-in refused");
        page(
            StatusCode::UNAUTHORIZED,
            "Sign-in failed",
            "Single sign-on did not complete. Try again, or ask an admin for a login link.",
        )
    };
    let (Some(code), Some(got_state)) = (q.code, q.state) else {
        return fail("missing code or state");
    };
    let Some(raw) = auth::cookie(&headers, COOKIE) else {
        return fail("no flow cookie");
    };
    let Some((payload, sig)) = raw.rsplit_once('.') else {
        return fail("bad flow cookie");
    };
    if !crate::token_matches(
        sig,
        &auth::sign(&state.admin_token, &format!("oidc:{payload}")),
    ) {
        return fail("bad flow signature");
    }
    let parts: Vec<&str> = payload.split('.').collect();
    let [st, nonce, verifier, exp] = parts[..] else {
        return fail("bad flow cookie");
    };
    let now = chrono::Utc::now().timestamp();
    if exp.parse::<i64>().unwrap_or(0) < now || !crate::token_matches(st, &got_state) {
        return fail("state mismatch or flow expired");
    }
    let d = match discovery(&cfg).await {
        Ok(d) => d,
        Err(_) => return fail("discovery failed"),
    };
    let redirect_uri = format!("{}/oidc/callback", crate::base_url(&headers));
    let form = [
        ("grant_type".to_owned(), "authorization_code".to_owned()),
        ("code".to_owned(), code),
        ("redirect_uri".to_owned(), redirect_uri),
        ("client_id".to_owned(), cfg.client_id.clone()),
        ("client_secret".to_owned(), cfg.client_secret.clone()),
        ("code_verifier".to_owned(), verifier.to_owned()),
    ];
    let endpoint = d.token_endpoint.clone();
    let token = tokio::task::spawn_blocking(move || -> Result<TokenResponse, String> {
        ureq::post(&endpoint)
            .send_form(form.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .map_err(|e| e.to_string())?
            .body_mut()
            .read_json()
            .map_err(|e| e.to_string())
    })
    .await;
    let Ok(Ok(token)) = token else {
        return fail("token exchange failed");
    };
    let email = match verified_email(&token.id_token, &d.issuer, &cfg.client_id, nonce, now) {
        Ok(e) => e,
        Err(why) => return fail(why),
    };
    let meta = state.meta.clone();
    let user = tokio::task::spawn_blocking(move || meta.user_by_email(&email)).await;
    let Ok(Ok(Some(user))) = user else {
        return page(
            StatusCode::FORBIDDEN,
            "Not invited",
            "You signed in, but nobody invited this email to OpenLens yet. Ask an admin to add you under Team.",
        );
    };
    if user.disabled {
        return page(
            StatusCode::FORBIDDEN,
            "Sign-in disabled",
            "An admin disabled sign-in for this account. Ask them to enable it again.",
        );
    }
    let _ = state.meta.touch_sign_in(&user.id);
    let session = auth::set_cookie(
        &auth::new_session(&state, &user.id),
        &headers,
        auth::SESSION_SECS,
    );
    let mut r = Redirect::to("/").into_response();
    for c in [session, format!("{COOKIE}=; Path=/oidc; Max-Age=0")] {
        if let Ok(v) = HeaderValue::from_str(&c) {
            r.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    r
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    fn token(claims: serde_json::Value) -> String {
        format!(
            "{}.{}.sig",
            b64url(br#"{"alg":"RS256"}"#),
            b64url(claims.to_string().as_bytes())
        )
    }

    #[test]
    fn base64url_round_trips_and_pkce_matches_the_rfc_example() {
        for s in ["", "f", "fo", "foo", "foob", "fooba", "foobar"] {
            assert_eq!(b64url_decode(&b64url(s.as_bytes())).unwrap(), s.as_bytes());
        }
        // RFC 7636, appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn id_token_claims_are_all_checked() {
        let ok = serde_json::json!({"iss":"https://id.example.com","aud":"cid","exp":2_000_000_000,"nonce":"n1","email":"ana@example.com","email_verified":true});
        let now = 1_900_000_000;
        assert_eq!(
            verified_email(
                &token(ok.clone()),
                "https://id.example.com/",
                "cid",
                "n1",
                now
            )
            .unwrap(),
            "ana@example.com"
        );
        let bad = |k: &str, v: serde_json::Value| {
            let mut c = ok.clone();
            c[k] = v;
            verified_email(&token(c), "https://id.example.com", "cid", "n1", now)
        };
        assert_eq!(
            bad("iss", "https://evil.example.com".into()).unwrap_err(),
            "wrong issuer"
        );
        assert_eq!(bad("aud", "other".into()).unwrap_err(), "wrong audience");
        assert!(bad("aud", serde_json::json!(["other", "cid"])).is_ok());
        assert_eq!(
            bad("exp", 1_800_000_000.into()).unwrap_err(),
            "expired ID token"
        );
        assert_eq!(bad("nonce", "n2".into()).unwrap_err(), "wrong nonce");
        assert_eq!(
            bad("email_verified", false.into()).unwrap_err(),
            "email not verified"
        );
        assert!(verified_email("garbage", "x", "cid", "n1", now).is_err());
    }
}
