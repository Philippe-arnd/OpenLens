//! Cookieless mode (#273): the SDK stores nothing on the device and sends
//! a placeholder; here the anonymous visitor becomes a hash of a daily
//! salt, the project, the client address and the user agent (the address
//! is never stored), and sessions continue while the visitor is active
//! within 30 minutes. The salt changes every UTC day and the previous one
//! is overwritten, so a visitor can't be followed from one day to the next.

use std::collections::HashMap;
use std::sync::Mutex;

use axum::http::HeaderMap;
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};

const PLACEHOLDER: &str = "$cookieless";
const SESSION_GAP: i64 = 30 * 60;

static SESSIONS: Mutex<Option<HashMap<String, (String, i64)>>> = Mutex::new(None);

fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    let _ = SystemRandom::new().fill(&mut b);
    hex::encode(b)
}

/// Today's salt, made when the day changes (the old one is overwritten).
pub(crate) fn salt(meta: &openlens_meta::Meta, today: &str) -> String {
    if let Ok(Some(v)) = meta.setting("cookieless_salt")
        && let Some((day, s)) = v.split_once(':')
        && day == today
    {
        return s.to_owned();
    }
    let s = random_hex(16);
    let _ = meta.set_setting("cookieless_salt", &format!("{today}:{s}"));
    s
}

/// The visitor id for this request today.
pub(crate) fn visitor(salt: &str, project: &str, ip: &str, ua: &str) -> String {
    let h = Sha256::digest(format!("{salt}\n{project}\n{ip}\n{ua}").as_bytes());
    format!("ck_{}", &hex::encode(h)[..20])
}

/// Replace the placeholders of cookieless events (no-op otherwise).
pub(crate) fn resolve(
    meta: &openlens_meta::Meta,
    events: &mut [openlens_event::Event],
    headers: &HeaderMap,
) {
    let marked = |e: &openlens_event::Event| {
        e.distinct_id == PLACEHOLDER || e.session_id.as_deref() == Some(PLACEHOLDER)
    };
    if !events.iter().any(marked) {
        return;
    }
    let now = chrono::Utc::now();
    let today = now.format("%Y-%m-%d").to_string();
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some(project) = events.first().map(|e| e.project.clone()) else {
        return;
    };
    // Without an address (no proxy in front), each request is its own visitor.
    let id = match crate::audience::client_ip(headers) {
        Some(ip) => visitor(&salt(meta, &today), &project, &ip.to_string(), ua),
        None => format!("ck_{}", random_hex(10)),
    };
    let sid = {
        let Ok(mut g) = SESSIONS.lock() else { return };
        let m = g.get_or_insert_with(HashMap::new);
        let t = now.timestamp();
        if m.len() > 100_000 {
            m.retain(|_, (_, last)| t - *last < SESSION_GAP);
        }
        let e = m
            .entry(id.clone())
            .or_insert_with(|| (format!("cks_{}", random_hex(8)), t));
        if t - e.1 > SESSION_GAP {
            e.0 = format!("cks_{}", random_hex(8));
        }
        e.1 = t;
        e.0.clone()
    };
    for e in events.iter_mut() {
        if e.distinct_id == PLACEHOLDER {
            e.distinct_id.clone_from(&id);
        }
        if e.session_id.as_deref() == Some(PLACEHOLDER) {
            e.session_id = Some(sid.clone());
        }
        // The daily id must never become a durable link to a login.
        if e.event == "$identify"
            && e.properties.contains(PLACEHOLDER)
            && let Ok(serde_json::Value::Object(mut p)) =
                serde_json::from_str::<serde_json::Value>(&e.properties)
        {
            p.remove("$anon_distinct_id");
            if let Ok(s) = serde_json::to_string(&serde_json::Value::Object(p)) {
                e.properties = s;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    #[test]
    fn the_visitor_id_changes_with_the_day_salt() {
        let a = super::visitor("salt1", "p", "203.0.113.1", "UA");
        assert_eq!(a, super::visitor("salt1", "p", "203.0.113.1", "UA"));
        assert_ne!(
            a,
            super::visitor("salt2", "p", "203.0.113.1", "UA"),
            "a new day, a new id"
        );
        assert_ne!(a, super::visitor("salt1", "p", "203.0.113.2", "UA"));
        assert!(a.starts_with("ck_") && !a.contains("203"));
        let m = openlens_meta::Meta::in_memory().unwrap();
        let s1 = super::salt(&m, "2026-11-23");
        assert_eq!(s1, super::salt(&m, "2026-11-23"));
        let s2 = super::salt(&m, "2026-11-24");
        assert_ne!(s1, s2);
        assert!(
            !m.setting("cookieless_salt")
                .ok()
                .flatten()
                .unwrap_or_default()
                .contains(&s1),
            "yesterday's salt is gone"
        );
    }
}
