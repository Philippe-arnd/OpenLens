//! The second factor for password sign-in (#440): the code step of the
//! login, and setting it up or turning it off on *My profile → Sign-in*.
//! The algorithm, codes and challenges are in [`crate::totp`].

use super::*;
use crate::totp;

/// The second-factor card for a user with a password.
pub(crate) fn totp_card(state: &AppState, u: &openlens_meta::User) -> openlens_ui::TotpCard {
    let Ok(Some(t)) = state.meta.totp_of(&u.id) else {
        return openlens_ui::TotpCard::Off;
    };
    if let Some(at) = t.enabled_at {
        return openlens_ui::TotpCard::On {
            since: chrono::DateTime::from_timestamp(at, 0)
                .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_default(),
            recovery_left: t.recovery_left,
        };
    }
    // A pending secret the server key no longer opens: start again.
    let Some(key) = totp::open_key(state, &t.sealed) else {
        return openlens_ui::TotpCard::Off;
    };
    let b32 = totp::base32(&key);
    let grouped = b32
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    openlens_ui::TotpCard::Pending {
        qr_svg: totp::qr_svg(&totp::otpauth_uri(&u.email, &b32)),
        key: grouped,
    }
}

fn no_store(mut r: Response) -> Response {
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// Is the second factor on for this user? A store error counts as on, so
/// it fails closed.
pub(super) fn enrolled(state: &AppState, user: &str) -> bool {
    state.meta.totp_enabled(user).unwrap_or(true)
}

/// Sign in: record it and issue the session.
pub(super) fn sign_in(state: &AppState, u: &openlens_meta::User, headers: &HeaderMap) -> Response {
    let _ = state.meta.touch_sign_in(&u.id);
    let c = auth::set_cookie(
        &auth::new_session(state, &u.id),
        headers,
        auth::SESSION_SECS,
    );
    with_cookie(Redirect::to("/").into_response(), &c)
}

/// The code form for `user`, with a fresh challenge.
pub(super) fn code_form(state: &AppState, user: &str, status: StatusCode) -> Response {
    let ch = totp::challenge(state, user, chrono::Utc::now().timestamp());
    no_store(html(
        status,
        openlens_ui::login_code(&ch, status != StatusCode::OK),
    ))
}

#[derive(Deserialize)]
pub(super) struct CodeForm {
    #[serde(default)]
    challenge: String,
    #[serde(default)]
    code: String,
}

/// `POST /login/totp`: the code (or a recovery code) after the password.
/// Behind the same slow-down and per-address budget as the password
/// (keyed by the account's email), and audited.
pub(super) async fn login_totp(
    State(state): State<AppState>,
    peer: crate::capture::Peer,
    headers: HeaderMap,
    Form(f): Form<CodeForm>,
) -> Response {
    let now = chrono::Utc::now().timestamp();
    let start_over = || {
        no_store(html(
            StatusCode::UNAUTHORIZED,
            openlens_ui::message(
                "Sign in again",
                "This step expired or is not valid. Sign in again with your email and password.",
            ),
        ))
    };
    if !crate::password::enabled() || f.code.len() > 40 {
        return start_over();
    }
    let Some(id) = totp::check_challenge(&state, &f.challenge, now) else {
        return start_over();
    };
    let Some(u) = crate::users::load(&state, &id)
        .await
        .filter(|u| !u.disabled)
    else {
        return start_over();
    };
    let ip = crate::capture::client_ip(&peer, &headers);
    let Some(attempt) = crate::password::Attempt::begin(&u.email, ip.as_deref()) else {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        return code_form(&state, &u.id, StatusCode::UNAUTHORIZED);
    };
    tokio::time::sleep(attempt.wait).await;
    let st = state.clone();
    let (uid, code) = (u.id.clone(), f.code.clone());
    let ok = tokio::task::spawn_blocking(move || totp::verify(&st, &uid, &code, now))
        .await
        .ok()
        .flatten();
    let Some(how) = ok else {
        attempt.done(false);
        crate::privacy::audit_as(&state, &u.id, None, "auth.totp_failed", "");
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        return code_form(&state, &u.id, StatusCode::UNAUTHORIZED);
    };
    attempt.done(true);
    crate::privacy::audit_as(
        &state,
        &u.id,
        None,
        "auth.totp_ok",
        match how {
            totp::Accepted::App => "",
            totp::Accepted::Recovery => "recovery code",
        },
    );
    sign_in(&state, &u, &headers)
}

/// Check the current password of a signed-in user, slowed down and
/// audited like the sign-in page. `Err` carries the message to show.
async fn check_current(
    state: &AppState,
    u: &openlens_meta::User,
    current: &str,
    peer: &crate::capture::Peer,
    headers: &HeaderMap,
    what: &str,
) -> Result<crate::password::Attempt, &'static str> {
    let ip = crate::capture::client_ip(peer, headers);
    let Some(attempt) = crate::password::Attempt::begin(&u.email, ip.as_deref()) else {
        return Err("Too many attempts; wait a moment and try again.");
    };
    tokio::time::sleep(attempt.wait).await;
    let meta = state.meta.clone();
    let (id, pw) = (u.id.clone(), current.to_owned());
    let ok = current.len() <= 200
        && tokio::task::spawn_blocking(move || {
            meta.password_of(&id)
                .ok()
                .flatten()
                .is_some_and(|(hash, _)| crate::password::verify(&hash, &pw))
        })
        .await
        .unwrap_or(false);
    if ok {
        Ok(attempt)
    } else {
        attempt.done(false);
        crate::privacy::audit_as(state, &u.id, None, "auth.password_failed", what);
        Err("The current password is not right.")
    }
}

/// The signed-in account (not the owner) with a password, when password
/// sign-in is on; else the page to show instead.
async fn account(state: &AppState, v: &auth::Viewer) -> Result<openlens_meta::User, Box<Response>> {
    let user = if v.id == auth::OWNER {
        None
    } else {
        crate::users::load(state, &v.id).await
    };
    let err = |m: &str| {
        crate::users::me_view(
            state,
            v,
            user.as_ref(),
            None,
            Some(m),
            openlens_ui::MeTab::SignIn,
        )
    };
    if !crate::password::enabled() {
        return Err(Box::new(err(
            "Password sign-in is turned off (OPENLENS_PASSWORD_LOGIN=off).",
        )));
    }
    let Some(u) = user.clone() else {
        return Err(Box::new(err(
            "The admin token has no account: create one first.",
        )));
    };
    if state.meta.password_of(&u.id).ok().flatten().is_none() {
        return Err(Box::new(err("Set a password first.")));
    }
    Ok(u)
}

fn me_error(state: &AppState, v: &auth::Viewer, u: &openlens_meta::User, m: &str) -> Response {
    crate::users::me_view(state, v, Some(u), None, Some(m), openlens_ui::MeTab::SignIn)
}

/// `POST /me/totp/start`: a new secret, pending until a code confirms it.
pub(crate) async fn me_totp_start(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
) -> Response {
    let u = match account(&state, &v).await {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let sealed = match totp::seal_key(&state, &totp::new_secret()) {
        Ok(s) => s,
        Err(e) => return me_error(&state, &v, &u, &e),
    };
    match state.meta.totp_begin(&u.id, &sealed) {
        Ok(true) => Redirect::to("/me/sign-in#totp").into_response(),
        Ok(false) => me_error(&state, &v, &u, "The authenticator app is already on."),
        Err(e) => me_error(&state, &v, &u, &e.to_string()),
    }
}

/// `POST /me/totp/cancel`: drop a pending set-up.
pub(crate) async fn me_totp_cancel(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
) -> Response {
    if v.id != auth::OWNER {
        let _ = state.meta.totp_clear(&v.id, None);
    }
    Redirect::to("/me/sign-in").into_response()
}

#[derive(Deserialize)]
pub(crate) struct TotpForm {
    #[serde(default)]
    current: String,
    #[serde(default)]
    code: String,
}

/// `POST /me/totp/enable`: the current password and a code from the app
/// turn it on; the recovery codes are shown once, other sessions end.
pub(crate) async fn me_totp_enable(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
    peer: crate::capture::Peer,
    headers: HeaderMap,
    Form(f): Form<TotpForm>,
) -> Response {
    let u = match account(&state, &v).await {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let pending = match state.meta.totp_of(&u.id) {
        Ok(Some(t)) if t.enabled_at.is_none() => t,
        Ok(_) => return me_error(&state, &v, &u, "Start the set-up first."),
        Err(e) => return me_error(&state, &v, &u, &e.to_string()),
    };
    let attempt = match check_current(
        &state,
        &u,
        &f.current,
        &peer,
        &headers,
        "current password, authenticator set-up",
    )
    .await
    {
        Ok(a) => a,
        Err(m) => return me_error(&state, &v, &u, m),
    };
    let now = chrono::Utc::now().timestamp();
    let step = totp::open_key(&state, &pending.sealed)
        .and_then(|key| totp::matching_step(&key, &f.code, now));
    let Some(step) = step else {
        attempt.done(false);
        crate::privacy::audit_as(
            &state,
            &u.id,
            None,
            "auth.totp_failed",
            "authenticator set-up",
        );
        return me_error(
            &state,
            &v,
            &u,
            "That code is not right: check the time on your phone and try the next one.",
        );
    };
    attempt.done(true);
    let codes = totp::new_recovery_codes();
    let hashes: Vec<String> = codes.iter().map(|c| totp::recovery_hash(c)).collect();
    match state.meta.totp_enable(&u.id, step, &hashes, now) {
        Ok(true) => {}
        Ok(false) => return me_error(&state, &v, &u, "Start the set-up again."),
        Err(e) => return me_error(&state, &v, &u, &e.to_string()),
    }
    crate::privacy::audit_as(&state, &u.id, None, "auth.totp_on", "");
    let c = auth::set_cookie(
        &auth::new_session(&state, &u.id),
        &headers,
        auth::SESSION_SECS,
    );
    no_store(with_cookie(
        html(StatusCode::OK, openlens_ui::totp_recovery_codes(&codes)),
        &c,
    ))
}

/// `POST /me/totp/disable`: the current password and a code (or a
/// recovery code) turn it off; other sessions end.
pub(crate) async fn me_totp_disable(
    crate::access::Signed(v): crate::access::Signed,
    State(state): State<AppState>,
    peer: crate::capture::Peer,
    headers: HeaderMap,
    Form(f): Form<TotpForm>,
) -> Response {
    let u = match account(&state, &v).await {
        Ok(u) => u,
        Err(r) => return *r,
    };
    if !state.meta.totp_enabled(&u.id).unwrap_or(false) {
        return me_error(&state, &v, &u, "The authenticator app is not on.");
    }
    let attempt = match check_current(
        &state,
        &u,
        &f.current,
        &peer,
        &headers,
        "current password, authenticator off",
    )
    .await
    {
        Ok(a) => a,
        Err(m) => return me_error(&state, &v, &u, m),
    };
    let now = chrono::Utc::now().timestamp();
    let st = state.clone();
    let (uid, code) = (u.id.clone(), f.code.clone());
    let ok = tokio::task::spawn_blocking(move || totp::verify(&st, &uid, &code, now))
        .await
        .ok()
        .flatten();
    if ok.is_none() {
        attempt.done(false);
        crate::privacy::audit_as(&state, &u.id, None, "auth.totp_failed", "authenticator off");
        return me_error(&state, &v, &u, "That code is not right.");
    }
    attempt.done(true);
    if let Err(e) = state.meta.totp_clear(&u.id, Some(now)) {
        return me_error(&state, &v, &u, &e.to_string());
    }
    crate::privacy::audit_as(&state, &u.id, None, "auth.totp_off", "");
    let c = auth::set_cookie(
        &auth::new_session(&state, &u.id),
        &headers,
        auth::SESSION_SECS,
    );
    with_cookie(
        Redirect::to("/me/sign-in?done=totp-off").into_response(),
        &c,
    )
}
