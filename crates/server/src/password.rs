//! Email + password sign-in (#428). The admin token stays the recovery
//! path: it always works on the login page, whatever the passwords.
//!
//! - Hashes are Argon2id (RustCrypto `argon2`, default parameters: 19 MiB,
//!   2 passes), a random 16-byte salt each, stored as a PHC string.
//! - A wrong password, an unknown email, a disabled account or one with no
//!   password all get the same answer after the same delay.
//! - After [`MAX_FAILS`] failures for one email within [`WINDOW`], that email
//!   is refused for the rest of the window, even with the right password.
//! - Changing a password signs out that user's other sessions: a session
//!   issued before `password_at` is refused ([`crate::auth::viewer`]).
//! - `OPENLENS_PASSWORD_LOGIN=off` turns it off (SSO-only installs).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHash, SaltString};
use argon2::{Argon2, PasswordHasher, PasswordVerifier};

/// Failures allowed per email within [`WINDOW`].
pub(crate) const MAX_FAILS: u32 = 5;
/// The failure window.
pub(crate) const WINDOW: Duration = Duration::from_secs(15 * 60);
/// Shortest password accepted.
pub(crate) const MIN_LEN: usize = 12;

/// Is password sign-in on?
pub(crate) fn enabled() -> bool {
    !std::env::var("OPENLENS_PASSWORD_LOGIN").is_ok_and(|v| v.trim() == "off")
}

/// Hash a password (blocking: about 50 ms).
pub(crate) fn hash(password: &str) -> Result<String, String> {
    let salt = SaltString::encode_b64(&rand::random::<[u8; 16]>()).map_err(|e| e.to_string())?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// Does `password` match the stored hash (blocking)?
pub(crate) fn verify(stored: &str, password: &str) -> bool {
    PasswordHash::new(stored).is_ok_and(|h| {
        Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok()
    })
}

/// A few of the most used passwords, refused whatever their length.
const COMMON: &[&str] = &[
    "123456789012",
    "password1234",
    "passwordpassword",
    "qwertyuiop12",
    "111111111111",
    "azertyuiop12",
    "iloveyou1234",
    "administrator",
    "changeme1234",
    "letmein12345",
    "welcome12345",
    "openlens1234",
    "000000000000",
    "abcdefghijkl",
    "motdepasse12",
];

/// Why a new password is refused, if it is.
pub(crate) fn check_new(password: &str, again: &str) -> Result<(), &'static str> {
    if password != again {
        return Err("The two passwords differ.");
    }
    if password.chars().count() < MIN_LEN {
        return Err("Use at least 12 characters (a few words work well).");
    }
    if password.len() > 200 {
        return Err("Use at most 200 characters.");
    }
    let lower = password.to_lowercase();
    let mut chars = lower.chars();
    let one_char = chars.next().is_some_and(|first| chars.all(|c| c == first));
    if COMMON.contains(&lower.as_str()) || one_char {
        return Err("That password is too common; pick another one.");
    }
    Ok(())
}

static FAILS: Mutex<Option<HashMap<String, (u32, Instant)>>> = Mutex::new(None);

/// Is this email locked out after too many failures?
pub(crate) fn locked(email: &str) -> bool {
    let Ok(mut g) = FAILS.lock() else { return true };
    let m = g.get_or_insert_with(HashMap::new);
    m.retain(|_, (_, at)| at.elapsed() < WINDOW);
    m.get(email).is_some_and(|(n, _)| *n >= MAX_FAILS)
}

/// Count a failure for this email.
pub(crate) fn failed(email: &str) {
    if let Ok(mut g) = FAILS.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        if m.len() > 10_000 {
            m.retain(|_, (_, at)| at.elapsed() < WINDOW);
        }
        let e = m.entry(email.to_owned()).or_insert((0, Instant::now()));
        e.0 += 1;
    }
}

/// Forget the failures of this email (a good sign-in).
pub(crate) fn succeeded(email: &str) {
    if let Ok(mut g) = FAILS.lock()
        && let Some(m) = g.as_mut()
    {
        m.remove(email);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn hashes_verify_and_never_hold_the_password() {
        let h = hash("correct horse battery").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(!h.contains("correct horse"));
        assert!(verify(&h, "correct horse battery"));
        assert!(!verify(&h, "correct horse batterz"));
        assert!(!verify("not a hash", "x"));
        assert_ne!(h, hash("correct horse battery").unwrap(), "salted");
    }

    #[test]
    fn weak_or_mistyped_passwords_are_refused() {
        assert!(check_new("short", "short").is_err());
        assert!(check_new("long enough pass", "long enough pasz").is_err());
        assert!(check_new("Password1234", "Password1234").is_err(), "common");
        assert!(check_new("aaaaaaaaaaaaaa", "aaaaaaaaaaaaaa").is_err());
        assert!(check_new("violet tram seven", "violet tram seven").is_ok());
    }

    #[test]
    fn five_failures_lock_the_email_until_the_window_ends() {
        let e = "lock-test@example.com";
        for _ in 0..MAX_FAILS {
            assert!(!locked(e));
            failed(e);
        }
        assert!(locked(e));
        succeeded(e);
        assert!(!locked(e));
    }
}
