//! The host allow-list (#485, D-043): a pending list drops nothing, an
//! active one drops other hosts and counts them, events without a page
//! address always pass, and a list is saved after its preview.

use super::environments::{api, capture, page};
use super::*;

const URI: &str = "/api/projects/default/environments/allow";

/// An event with no page address, as a server SDK sends it.
fn hostless(uuid: u32) -> serde_json::Value {
    serde_json::json!({
        "event": "signup",
        "distinct_id": "u1",
        "uuid": format!("0191f0c0-0000-7000-8000-{uuid:012}"),
    })
}

async fn allow(h: &Harness, hosts: &[&str], confirm: bool) -> (StatusCode, serde_json::Value) {
    openlens_server::flush_now(&h.state).unwrap();
    let body = serde_json::json!({ "hosts": hosts, "confirm": confirm });
    api(h, "POST", URI, Some(body)).await
}

async fn status(h: &Harness) -> serde_json::Value {
    let (s, v) = api(h, "GET", "/api/projects/default/environments", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["allow"].clone()
}

#[tokio::test]
async fn a_pending_list_drops_nothing() {
    let h = harness();
    capture(&h, vec![page("app.example.com", 1, None)]).await;
    // A typo: no event has ever come from the host on the list.
    let (s, v) = allow(&h, &["app.exmaple.com"], true).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["preview"]["status"], "pending", "{v}");
    assert_eq!(v["saved"], true);
    let v = capture(
        &h,
        vec![
            page("app.example.com", 2, None),
            page("other.example.org", 3, None),
        ],
    )
    .await;
    assert_eq!(v["accepted"], 2, "{v}");
    let a = status(&h).await;
    assert_eq!(a["status"], "pending", "{a}");
    assert_eq!(a["hosts"], serde_json::json!(["app.exmaple.com"]));
    assert!(a["dropped"].as_array().unwrap().is_empty(), "{a}");
}

#[tokio::test]
async fn an_active_list_drops_other_hosts_counts_them_and_keeps_hostless_events() {
    let h = harness();
    capture(&h, vec![page("app.example.com", 1, None)]).await;
    let (s, v) = allow(&h, &["app.example.com"], true).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["preview"]["status"], "active", "{v}");
    let v = capture(
        &h,
        vec![
            page("app.example.com", 2, None),
            page("copycat.example.org", 3, None),
            page("copycat.example.org", 4, None),
            page("scraper.example.net", 5, None),
            hostless(6),
        ],
    )
    .await;
    assert_eq!(
        v["accepted"], 2,
        "the listed host and the hostless event: {v}"
    );
    let a = status(&h).await;
    assert_eq!(a["status"], "active", "{a}");
    assert_eq!(
        a["dropped"][0],
        serde_json::json!({ "host": "copycat.example.org", "events": 2 }),
        "{a}"
    );
    assert!(
        a["dropped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["host"] == "scraper.example.net"),
        "{a}"
    );
    let log = h.state.meta.audit_log(10).unwrap();
    assert!(
        log.iter()
            .any(|e| e.3 == "environments.allow" && e.4.contains("app.example.com")),
        "{log:?}"
    );

    // An empty list turns it off: every host is accepted again.
    let (s, _) = allow(&h, &[], true).await;
    assert_eq!(s, StatusCode::OK);
    let v = capture(&h, vec![page("copycat.example.org", 7, None)]).await;
    assert_eq!(v["accepted"], 1, "{v}");
    assert_eq!(status(&h).await["status"], "off");
}

#[tokio::test]
async fn the_preview_counts_the_last_day_and_saves_nothing() {
    let h = harness();
    capture(
        &h,
        vec![
            page("app.example.com", 1, None),
            page("app.example.com", 2, None),
            page("pr-1.preview.example.com", 3, None),
            page("copycat.example.org", 4, None),
            page("copycat.example.org", 5, None),
            page("scraper.example.net", 6, None),
            hostless(7),
        ],
    )
    .await;
    let (s, p) = allow(&h, &["app.example.com", "*.preview.example.com"], false).await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert_eq!(p["saved"], false);
    let p = &p["preview"];
    assert_eq!(p["status"], "active", "{p}");
    assert_eq!(
        p["kept_events"], 4,
        "listed hosts and the hostless event: {p}"
    );
    assert_eq!(p["dropped_events"], 3, "{p}");
    assert_eq!(
        p["dropped"],
        serde_json::json!([
            { "host": "copycat.example.org", "events": 2 },
            { "host": "scraper.example.net", "events": 1 },
        ])
    );
    assert_eq!(status(&h).await["status"], "off", "a preview saves nothing");

    let (s, p) = allow(&h, &["app.exmaple.com"], false).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(p["preview"]["status"], "pending", "{p}");
    let (s, _) = allow(&h, &["bad host"], false).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

/// The settings tab previews the list (a GET), then saves it.
#[tokio::test]
async fn the_settings_tab_previews_then_saves_the_list() {
    let h = harness();
    capture(
        &h,
        vec![
            page("app.example.com", 1, None),
            page("copycat.example.org", 2, None),
        ],
    )
    .await;
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let uri = "/p/default/settings/environments";
    let html = body_text(
        get(
            &h,
            &format!("{uri}?allow=app.example.com"),
            Some(&owner),
            false,
        )
        .await,
    )
    .await;
    assert!(html.contains("Save the list"), "{html}");
    assert!(html.contains("copycat.example.org"), "{html}");
    assert_eq!(status(&h).await["status"], "off");
    let code = post_form(&h, uri, &owner, "action=allow&hosts=app.example.com").await;
    assert_eq!(code, StatusCode::SEE_OTHER);
    let a = status(&h).await;
    assert_eq!(a["hosts"], serde_json::json!(["app.example.com"]), "{a}");
    assert_eq!(a["status"], "active", "{a}");
}
