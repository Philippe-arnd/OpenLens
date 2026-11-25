//! The second factor for password sign-in (#440).

use super::*;

/// A form POST: status, the session cookie it sets (if any), the body.
async fn send(
    h: &Harness,
    uri: &str,
    cookie: &str,
    body: &str,
) -> (StatusCode, Option<String>, String) {
    let r = router(h.state.clone())
        .oneshot(
            Request::post(uri)
                .header("cookie", cookie)
                .header("origin", "http://localhost:8686")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = r.status();
    let set = r
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("ol_session="))
        .map(|v| v.split(';').next().unwrap().to_owned());
    (status, set, body_text(r).await)
}

/// RFC 4648 base32 (no padding), for the test's own authenticator.
fn unbase32(s: &str) -> Vec<u8> {
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'2'..=b'7' => c - b'2' + 26,
            _ => continue,
        };
        acc = (acc << 5) | u32::from(v);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// What an authenticator app shows at `unix` (RFC 6238, SHA-1, 6 digits).
fn code_at(key: &[u8], unix: i64) -> String {
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, key);
    let tag = ring::hmac::sign(&k, &((unix / 30) as u64).to_be_bytes());
    let d = tag.as_ref();
    let off = (d[19] & 0x0f) as usize;
    let w = u32::from_be_bytes([d[off] & 0x7f, d[off + 1], d[off + 2], d[off + 3]]);
    format!("{:06}", w % 1_000_000)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn between<'a>(s: &'a str, from: &str, to: &str) -> &'a str {
    let rest = &s[s.find(from).unwrap_or_else(|| panic!("{from} in {s}")) + from.len()..];
    &rest[..rest.find(to).unwrap()]
}

/// A member with a password, signed in: (id, session).
async fn member(h: &Harness, email: &str, password: &str) -> (String, String) {
    let u = h.state.meta.upsert_user(email, "viewer", None).unwrap();
    let t = h
        .state
        .meta
        .create_login_link_for(Some(&u.id), 3600)
        .unwrap();
    let (_, link, _) = send(h, &format!("/l/{t}"), "", "").await;
    let (s, session, _) = send(
        h,
        "/me/password",
        &link.unwrap(),
        &format!("password={0}&again={0}", url_encode(password)),
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    (u.id, session.unwrap())
}

/// Turn the second factor on: (secret, recovery codes, fresh session).
async fn enrol(h: &Harness, session: &str, password: &str) -> (Vec<u8>, Vec<String>, String) {
    let (s, _, _) = send(h, "/me/totp/start", session, "").await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    let page = body_text(get(h, "/me/sign-in", Some(session), false).await).await;
    assert!(page.contains("class=\"qr\""), "a QR code");
    let key = unbase32(between(&page, "<code class=\"mono\">", "</code>"));
    let (s, fresh, page) = send(
        h,
        "/me/totp/enable",
        session,
        &format!(
            "current={}&code={}",
            url_encode(password),
            code_at(&key, now())
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{page}");
    let codes: Vec<String> = between(&page, "<pre class=\"copy\">", "</pre>")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    (key, codes, fresh.unwrap())
}

/// Password sign-in: (status, session cookie, the code step's challenge).
async fn password_login(
    h: &Harness,
    email: &str,
    password: &str,
) -> (StatusCode, Option<String>, Option<String>) {
    let (s, c, page) = send(
        h,
        "/login/password",
        "",
        &format!(
            "email={}&password={}",
            url_encode(email),
            url_encode(password)
        ),
    )
    .await;
    let ch = page
        .contains("name=\"challenge\"")
        .then(|| between(&page, "name=\"challenge\" value=\"", "\"").to_owned());
    (s, c, ch)
}

async fn code_login(h: &Harness, challenge: &str, code: &str) -> (StatusCode, Option<String>) {
    let (s, c, _) = send(
        h,
        "/login/totp",
        "",
        &format!(
            "challenge={}&code={}",
            url_encode(challenge),
            url_encode(code)
        ),
    )
    .await;
    (s, c)
}

#[tokio::test]
async fn a_second_factor_guards_password_sign_in() {
    let h = harness();
    let (email, pw) = ("[EMAIL_REDACTED]", "violet tram seven");
    let (_, s1) = member(&h, email, pw).await;
    // Not enrolled: the password alone signs in, as before.
    let (s, c, ch) = password_login(&h, email, pw).await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(c.is_some() && ch.is_none());

    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (key, codes, s2) = enrol(&h, &s1, pw).await;
    assert_eq!(codes.len(), 8);
    assert_ne!(
        get(&h, "/me", Some(&s1), false).await.status(),
        StatusCode::OK,
        "turning it on ends the other sessions"
    );
    assert_eq!(
        get(&h, "/me", Some(&s2), false).await.status(),
        StatusCode::OK
    );

    // Enrolled: the password gives a challenge, not a session.
    let (s, c, ch) = password_login(&h, email, pw).await;
    assert_eq!(s, StatusCode::OK);
    assert!(c.is_none(), "no session before the code");
    let ch = ch.expect("a code step");
    assert!(
        get(&h, "/me", Some(&format!("ol_session={ch}")), false)
            .await
            .status()
            .is_redirection(),
        "the challenge is not a session"
    );

    // A wrong code is refused.
    let good = code_at(&key, now() + 30);
    let last = good.as_bytes()[5] - b'0';
    let wrong = format!("{}{}", &good[..5], (last + 1) % 10);
    let (s, c) = code_login(&h, &ch, &wrong).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(c.is_none());

    // The right one signs in, once: replaying it is refused.
    let (s, c) = code_login(&h, &ch, &good).await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(c.is_some());
    let (_, _, ch) = password_login(&h, email, pw).await;
    let ch = ch.unwrap();
    let (s, c) = code_login(&h, &ch, &good).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "a used code is refused");
    assert!(c.is_none());

    // A recovery code works once.
    let (s, c) = code_login(&h, &ch, &codes[0].to_uppercase()).await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(c.is_some());
    let (_, _, ch) = password_login(&h, email, pw).await;
    let (s, _) = code_login(&h, &ch.unwrap(), &codes[0]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "a recovery code is single-use");

    // Turning it off needs the password and a code as well.
    let (s, _, _) = send(
        &h,
        "/me/totp/disable",
        &s2,
        &format!("current={}&code=", url_encode(pw)),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _, _) = send(
        &h,
        "/me/totp/disable",
        &s2,
        &format!("current={}&code={}", url_encode(pw), codes[1]),
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    let (s, c, ch) = password_login(&h, email, pw).await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(c.is_some() && ch.is_none(), "back to the password alone");
}

#[tokio::test]
async fn an_admin_reset_clears_the_second_factor() {
    let h = harness();
    let token = login_cookie(&h).await;
    let (email, pw) = ("[EMAIL_REDACTED]", "violet tram seven");
    let (id, s1) = member(&h, email, pw).await;
    enrol(&h, &s1, pw).await;
    assert!(h.state.meta.totp_enabled(&id).unwrap());
    let (s, _, page) = send(&h, &format!("/users/{id}/reset-password"), &token, "").await;
    assert_eq!(s, StatusCode::OK, "{page}");
    assert!(h.state.meta.totp_of(&id).unwrap().is_none(), "factor gone");
    // The reset link, a new password, and the password alone signs in.
    let link = between(&page, "/l/", "<").to_owned();
    let (_, fresh, _) = send(&h, &format!("/l/{link}"), "", "").await;
    let (s, _, _) = send(
        &h,
        "/me/password",
        &fresh.unwrap(),
        "password=amber+kite+river&again=amber+kite+river",
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    let (s, c, ch) = password_login(&h, email, "amber kite river").await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(c.is_some() && ch.is_none());
}
