//! Email + password sign-in (#428). The admin token stays the recovery
//! path: it always works on the login page, whatever the passwords.
//!
//! - Hashes are Argon2id (RustCrypto `argon2`, default parameters: 19 MiB,
//!   2 passes), a random 16-byte salt each, stored as a PHC string.
//! - A wrong password, an unknown email, a disabled account or one with no
//!   password all get the same answer after the same delay.
//! - Failures slow the next attempt down instead of locking the account
//!   (no one can lock someone else out): after [`FREE_FAILS`] failures for one
//!   email within [`WINDOW`], each attempt waits [`backoff`] first, and only
//!   one attempt per email runs at a time. One client address gets at most
//!   [`MAX_IP_FAILS`] failures per window, whatever the emails.
//! - The counters live in memory: a restart clears them (one pod).
//! - Changing a password signs out that user's other sessions: a session
//!   issued before `password_at` is refused ([`crate::auth::viewer`]).
//! - `OPENLENS_PASSWORD_LOGIN=off` turns it off (SSO-only installs).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHash, SaltString};
use argon2::{Argon2, PasswordHasher, PasswordVerifier};

/// Failures per email before attempts slow down.
pub(crate) const FREE_FAILS: u32 = 3;
/// Failures one client address may make within [`WINDOW`].
pub(crate) const MAX_IP_FAILS: u32 = 30;
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

/// Failures per key (`e:<email>` or `ip:<address>`), and whether an
/// attempt for it is running.
#[derive(Clone, Copy)]
struct Fails {
    n: u32,
    at: Instant,
    busy: bool,
}

static FAILS: Mutex<Option<HashMap<String, Fails>>> = Mutex::new(None);

/// The wait before an attempt after `n` failures: none for the first
/// [`FREE_FAILS`], then 1 s, 2 s, 4 s… up to 30 s.
pub(crate) fn backoff(n: u32) -> Duration {
    match n.checked_sub(FREE_FAILS) {
        None => Duration::ZERO,
        Some(k) => Duration::from_secs((1u64 << k.min(5)).min(30)),
    }
}

fn with<T>(f: impl FnOnce(&mut HashMap<String, Fails>) -> T) -> Option<T> {
    let mut g = FAILS.lock().ok()?;
    let m = g.get_or_insert_with(HashMap::new);
    if m.len() > 10_000 {
        m.retain(|_, v| v.busy || v.at.elapsed() < WINDOW);
    }
    Some(f(m))
}

fn count(m: &HashMap<String, Fails>, key: &str) -> u32 {
    m.get(key)
        .filter(|v| v.at.elapsed() < WINDOW)
        .map_or(0, |v| v.n)
}

/// One sign-in attempt for an email, from a client address (`None`: direct
/// local traffic, not limited per address). Hold it while checking the
/// password, then call [`Attempt::done`].
pub(crate) struct Attempt {
    email: String,
    ip: Option<String>,
    /// How long to wait before checking the password.
    pub(crate) wait: Duration,
    finished: bool,
}

impl Attempt {
    /// Start an attempt; `None` when it is refused at once: this address
    /// failed too often, or another attempt for this email is running.
    pub(crate) fn begin(email: &str, ip: Option<&str>) -> Option<Attempt> {
        let e = format!("e:{email}");
        with(|m| {
            if let Some(ip) = ip
                && count(m, &format!("ip:{ip}")) >= MAX_IP_FAILS
            {
                return None;
            }
            let n = count(m, &e);
            let v = m.entry(e).or_insert(Fails {
                n: 0,
                at: Instant::now(),
                busy: false,
            });
            if v.busy {
                return None;
            }
            if v.at.elapsed() >= WINDOW {
                v.n = 0;
            }
            v.busy = true;
            Some(Attempt {
                email: email.to_owned(),
                ip: ip.map(str::to_owned),
                wait: backoff(n),
                finished: false,
            })
        })
        .flatten()
    }

    /// The password was right (`true`: forget this email's failures) or
    /// wrong (`false`: count one for the email and the address).
    pub(crate) fn done(mut self, ok: bool) {
        self.finished = true;
        let (email, ip) = (format!("e:{}", self.email), self.ip.take());
        with(|m| {
            if ok {
                m.remove(&email);
                return;
            }
            let mut bump = |k: String| {
                let v = m.entry(k).or_insert(Fails {
                    n: 0,
                    at: Instant::now(),
                    busy: false,
                });
                if v.at.elapsed() >= WINDOW {
                    v.n = 0;
                }
                v.n += 1;
                v.at = Instant::now();
                v.busy = false;
            };
            bump(email);
            if let Some(ip) = ip {
                bump(format!("ip:{ip}"));
            }
        });
    }
}

impl Drop for Attempt {
    /// An attempt dropped without [`Attempt::done`] (an error on the way)
    /// frees its email.
    fn drop(&mut self) {
        if !self.finished {
            let e = format!("e:{}", self.email);
            with(|m| {
                if let Some(v) = m.get_mut(&e) {
                    v.busy = false;
                }
            });
        }
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
    fn failures_slow_down_one_email_never_lock_it() {
        let e = "backoff-test@example.com";
        for i in 0..FREE_FAILS + 2 {
            let a = Attempt::begin(e, None).unwrap();
            assert_eq!(a.wait, backoff(i));
            assert!(Attempt::begin(e, None).is_none(), "one at a time");
            a.done(false);
        }
        assert_eq!(backoff(FREE_FAILS), Duration::from_secs(1));
        assert_eq!(backoff(FREE_FAILS + 1), Duration::from_secs(2));
        assert_eq!(backoff(FREE_FAILS + 20), Duration::from_secs(30));
        // Still allowed (slowly), and a good sign-in forgets the failures.
        Attempt::begin(e, None).unwrap().done(true);
        assert_eq!(Attempt::begin(e, None).unwrap().wait, Duration::ZERO);
    }

    #[test]
    fn one_address_has_a_failure_budget_across_emails() {
        let ip = Some("203.0.113.77");
        for i in 0..MAX_IP_FAILS {
            Attempt::begin(&format!("ip-test-{i}@example.com"), ip)
                .unwrap()
                .done(false);
        }
        assert!(Attempt::begin("ip-test-last@example.com", ip).is_none());
        assert!(Attempt::begin("ip-test-last@example.com", Some("203.0.113.78")).is_some());
    }
}
