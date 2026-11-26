//! Automatic releases from the build fingerprint (#495, D-044): a new
//! fingerprint is named `YYYY-MM-DD.N`, an explicit release wins and becomes
//! the fingerprint's name for later events, past events keep theirs.

use super::*;

fn event(uuid: u32, env: &str, build: &str, release: Option<&str>) -> serde_json::Value {
    let mut props = serde_json::json!({ "$environment": env, "$build": build });
    if let Some(r) = release {
        props["$release"] = r.into();
    }
    serde_json::json!({
        "event": "$pageview",
        "distinct_id": "u1",
        "uuid": format!("0191f0c0-0000-7000-8000-{uuid:012}"),
        "properties": props,
    })
}

async fn capture(h: &Harness, events: Vec<serde_json::Value>) {
    let body = serde_json::json!({ "key": h.key, "batch": events }).to_string();
    let (status, v) = post(h, "/capture", body.into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK, "{v}");
}

/// The stored release of each event, by the last digits of its uuid.
async fn releases(h: &Harness) -> std::collections::BTreeMap<u32, Option<String>> {
    openlens_server::flush_now(&h.state).unwrap();
    let r = h
        .state
        .query
        .sql("default", "SELECT uuid, release FROM events")
        .await
        .unwrap();
    r.rows
        .iter()
        .map(|row| {
            let uuid = row["uuid"].as_str().unwrap();
            let n = uuid.rsplit('-').next().unwrap().parse().unwrap();
            (
                n,
                row.get("release")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
            )
        })
        .collect()
}

/// `(day, N)` of an automatic name.
fn split(name: &str) -> (&str, u32) {
    let (day, n) = name.rsplit_once('.').unwrap();
    assert!(
        chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").is_ok(),
        "{name}"
    );
    (day, n.parse().unwrap())
}

#[tokio::test]
async fn new_fingerprints_get_the_days_next_name_and_keep_it() {
    let h = harness();
    capture(&h, vec![event(1, "production", "aaaa1111", None)]).await;
    capture(
        &h,
        vec![
            event(2, "production", "bbbb2222", None),
            event(3, "production", "aaaa1111", None),
            event(4, "staging", "aaaa1111", None),
            // Not a fingerprint: no release is made up.
            event(5, "production", "../etc", None),
        ],
    )
    .await;
    let r = releases(&h).await;
    let a = r[&1].clone().unwrap();
    let b = r[&2].clone().unwrap();
    assert_eq!(r[&3].as_deref(), Some(a.as_str()), "same build, same name");
    let ((day_a, n_a), (day_b, n_b)) = (split(&a), split(&b));
    assert_eq!(day_a, day_b);
    assert!(n_b > n_a, "{a} then {b}");
    // Each environment counts its own builds.
    assert_eq!(split(r[&4].as_deref().unwrap()).1, n_a);
    assert_eq!(r[&5], None);
}

#[tokio::test]
async fn an_explicit_release_wins_and_names_the_fingerprint_from_then_on() {
    let h = harness();
    capture(&h, vec![event(1, "production", "aaaa1111", None)]).await;
    capture(&h, vec![event(2, "production", "aaaa1111", Some("v1.4.0"))]).await;
    capture(&h, vec![event(3, "production", "aaaa1111", None)]).await;
    // A fingerprint first seen with an explicit release is named by it.
    capture(
        &h,
        vec![
            event(4, "production", "cccc3333", Some("v2.0.0")),
            event(5, "production", "cccc3333", None),
        ],
    )
    .await;
    let r = releases(&h).await;
    let auto = r[&1].clone().unwrap();
    split(&auto);
    assert_eq!(r[&2].as_deref(), Some("v1.4.0"), "explicit wins");
    assert_eq!(
        r[&3].as_deref(),
        Some("v1.4.0"),
        "the alias applies to later events"
    );
    assert_eq!(r[&5].as_deref(), Some("v2.0.0"));
    // Stored events are not rewritten (D-024): the first keeps its name.
    assert_eq!(r[&1].as_deref(), Some(auto.as_str()));
    // The alias is recorded, not only cached.
    assert_eq!(
        h.state
            .meta
            .build_release("default", "production", "aaaa1111")
            .unwrap()
            .as_deref(),
        Some("v1.4.0")
    );
}
