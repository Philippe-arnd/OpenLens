//! The second factor for password sign-in (#440): time-based one-time
//! codes from an authenticator app (RFC 6238 over RFC 4226: HMAC-SHA1,
//! 6 digits, 30 s steps), and single-use recovery codes.
//!
//! - The secret (20 random bytes) is sealed with the server key
//!   ([`crate::secrets`]); the store never holds it in clear. A restored
//!   backup without the key loses the second factors.
//! - A code is accepted for the current step, the one before or the one
//!   after (clock drift), and each step works once: the store keeps the
//!   last one used.
//! - 8 recovery codes, shown once, stored as SHA-256 hashes; each works
//!   once.
//! - After a correct password, an enrolled account gets a signed
//!   challenge (5 minutes, bound to the user and their current
//!   credentials). It is not a session and opens nothing by itself.
//! - Only password sign-in asks for it: login links, single sign-on and
//!   the admin token are separate paths (SECURITY.md).

use ring::hmac;

use crate::AppState;

/// Seconds per step.
const STEP: i64 = 30;
/// How long a challenge stays valid.
pub(crate) const CHALLENGE_SECS: i64 = 5 * 60;
/// Recovery codes made at enrolment.
pub(crate) const RECOVERY_CODES: usize = 8;
/// The name an authenticator app shows.
const ISSUER: &str = "OpenLens";

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 base32, no padding (what authenticator apps take).
pub(crate) fn base32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(B32[((acc >> bits) & 31) as usize]));
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(char::from(B32[((acc << (5 - bits)) & 31) as usize]));
    }
    out
}

/// A new secret.
pub(crate) fn new_secret() -> [u8; 20] {
    rand::random()
}

/// The 6-digit code of `counter` (RFC 4226, HMAC-SHA1).
fn hotp(key: &[u8], counter: u64) -> u32 {
    let tag = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, key),
        &counter.to_be_bytes(),
    );
    let d = tag.as_ref();
    let off = usize::from(d.last().copied().unwrap_or(0) & 0x0f);
    let word = d
        .get(off..off + 4)
        .and_then(|w| <[u8; 4]>::try_from(w).ok())
        .map_or(0, |w| u32::from_be_bytes(w) & 0x7fff_ffff);
    word % 1_000_000
}

/// The 30 s step of a Unix time.
pub(crate) fn step_of(unix: i64) -> i64 {
    unix.div_euclid(STEP)
}

/// The step `code` belongs to, if it is the code of the step of `now`,
/// the one before or the one after. Each candidate is compared in
/// constant time.
pub(crate) fn matching_step(key: &[u8], code: &str, now: i64) -> Option<i64> {
    let code = code.trim();
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let t = step_of(now);
    let mut found = None;
    for s in [t - 1, t, t + 1] {
        let Ok(counter) = u64::try_from(s) else {
            continue;
        };
        let want = format!("{:06}", hotp(key, counter));
        if crate::token_matches(code, &want) && found.is_none() {
            found = Some(s);
        }
    }
    found
}

/// The `otpauth://` link an authenticator app reads from the QR code.
pub(crate) fn otpauth_uri(email: &str, secret_b32: &str) -> String {
    let enc = |s: &str| -> String {
        s.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                    char::from(b).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect()
    };
    format!(
        "otpauth://totp/{ISSUER}:{}?secret={secret_b32}&issuer={ISSUER}&algorithm=SHA1&digits=6&period=30",
        enc(email)
    )
}

/// The QR code of `text` as inline SVG, without the XML declaration.
pub(crate) fn qr_svg(text: &str) -> Option<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let svg = code
        .render::<qrcode::render::svg::Color<'_>>()
        .min_dimensions(200, 200)
        .build();
    let at = svg.find("<svg")?;
    svg.get(at..).map(str::to_owned)
}

/// Fresh recovery codes, as shown to the user (`xxxxx-xxxxx`).
pub(crate) fn new_recovery_codes() -> Vec<String> {
    (0..RECOVERY_CODES)
        .map(|_| {
            // 7 random bytes are 12 base32 characters; 10 are kept (50 bits).
            let raw = base32(&rand::random::<[u8; 7]>()).to_lowercase();
            let a = raw.get(..5).unwrap_or_default();
            let b = raw.get(5..10).unwrap_or_default();
            format!("{a}-{b}")
        })
        .collect()
}

/// The stored hash of a recovery code, whatever its case, spaces or
/// dashes as typed.
pub(crate) fn recovery_hash(code: &str) -> String {
    let norm: String = code
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    openlens_meta::sha256_hex(&format!("totp-recovery:{norm}"))
}

/// What a challenge signs: the user, its expiry, and when their
/// credentials last changed (a reset or a change voids it).
fn challenge_msg(exp: i64, user: &str, changed: i64) -> String {
    format!("totp-challenge:{exp}:{user}:{changed}")
}

/// A challenge for `user` after a correct password: `<exp>.<user>.<sig>`.
pub(crate) fn challenge(state: &AppState, user: &str, now: i64) -> String {
    let exp = now + CHALLENGE_SECS;
    let changed = state
        .meta
        .password_changed_at(user)
        .ok()
        .flatten()
        .unwrap_or(0);
    format!(
        "{exp}.{user}.{}",
        crate::auth::sign(&state.session_key, &challenge_msg(exp, user, changed))
    )
}

/// The user a challenge was made for, while it is valid.
pub(crate) fn check_challenge(state: &AppState, token: &str, now: i64) -> Option<String> {
    if token.len() > 300 {
        return None;
    }
    let (exp, rest) = token.split_once('.')?;
    let exp: i64 = exp.parse().ok()?;
    if exp <= now || exp > now + CHALLENGE_SECS {
        return None;
    }
    let (user, sig) = rest.split_once('.')?;
    let changed = state
        .meta
        .password_changed_at(user)
        .ok()
        .flatten()
        .unwrap_or(0);
    let want = crate::auth::sign(&state.session_key, &challenge_msg(exp, user, changed));
    crate::token_matches(sig, &want).then(|| user.to_owned())
}

/// How a code was accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Accepted {
    /// A code from the app.
    App,
    /// A recovery code (now used).
    Recovery,
}

/// Check a code or a recovery code for an enrolled user, and use it
/// (blocking: reads and writes the store). `None` when it is wrong, used
/// already, or the secret cannot be opened (the server key changed).
pub(crate) fn verify(state: &AppState, user: &str, code: &str, now: i64) -> Option<Accepted> {
    let code = code.trim();
    if code.is_empty() || code.len() > 40 {
        return None;
    }
    let t = state.meta.totp_of(user).ok()??;
    t.enabled_at?;
    if code.bytes().all(|b| b.is_ascii_digit()) {
        let key = open_key(state, &t.sealed)?;
        let s = matching_step(&key, code, now)?;
        return state
            .meta
            .totp_use_step(user, s)
            .ok()?
            .then_some(Accepted::App);
    }
    state
        .meta
        .totp_use_recovery(user, &recovery_hash(code))
        .ok()?
        .then_some(Accepted::Recovery)
}

/// The raw secret from its sealed form.
pub(crate) fn open_key(state: &AppState, sealed: &str) -> Option<Vec<u8>> {
    let plain = crate::secrets::open(&state.data_dir, sealed).ok()?;
    hex::decode(plain).ok()
}

/// Seal a raw secret for the store.
pub(crate) fn seal_key(state: &AppState, key: &[u8]) -> Result<String, String> {
    crate::secrets::seal(&state.data_dir, &hex::encode(key))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn rfc6238_sha1_vectors() {
        // RFC 6238 Appendix B, SHA-1 key, last 6 digits.
        let key = b"12345678901234567890";
        for (t, code) in [
            (59, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
        ] {
            assert_eq!(matching_step(key, code, t), Some(step_of(t)), "{t}");
        }
    }

    #[test]
    fn one_step_of_drift_either_way_and_no_more() {
        let key = b"12345678901234567890";
        let now = 1_111_111_111;
        assert_eq!(matching_step(key, "050471", now + 30), Some(step_of(now)));
        assert_eq!(matching_step(key, "050471", now - 30), Some(step_of(now)));
        assert_eq!(matching_step(key, "050471", now + 90), None);
        assert_eq!(matching_step(key, "05047", now), None);
        assert_eq!(matching_step(key, "05047a", now), None);
    }

    #[test]
    fn base32_matches_rfc4648() {
        assert_eq!(base32(b""), "");
        assert_eq!(base32(b"f"), "MY");
        assert_eq!(base32(b"fo"), "MZXQ");
        assert_eq!(base32(b"foo"), "MZXW6");
        assert_eq!(base32(b"foob"), "MZXW6YQ");
        assert_eq!(base32(b"fooba"), "MZXW6YTB");
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
    }

    #[test]
    fn recovery_codes_are_distinct_and_hash_as_typed() {
        let codes = new_recovery_codes();
        assert_eq!(codes.len(), RECOVERY_CODES);
        let mut uniq = codes.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), codes.len());
        let c = &codes[0];
        assert_eq!(
            recovery_hash(c),
            recovery_hash(&c.to_uppercase().replace('-', " "))
        );
        assert_ne!(recovery_hash(c), recovery_hash(&codes[1]));
    }

    #[test]
    fn the_qr_code_is_inline_svg() {
        let svg = qr_svg(&otpauth_uri("ana@example.com", "MZXW6YTBOI")).unwrap();
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
    }
}
