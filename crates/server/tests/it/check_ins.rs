//! Job check-ins (#609, D-086): the endpoint records a run with the key in
//! the path or in the header, and is bounded (bad slug, too fast, too many
//! jobs, wrong key, hostile query strings); the key is never echoed; the
//! Uptime page shows the jobs, or how to add one.

use openlens_engine::commands::jobs::MAX_JOBS;
use openlens_meta::jobs::JobRun;

use super::*;

async fn check_in(h: &Harness, uri: &str, key: Option<&str>) -> (StatusCode, String, bool) {
    let mut req = Request::post(uri);
    if let Some(k) = key {
        req = req.header("x-openlens-key", k);
    }
    let r = router(h.state.clone())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let s = r.status();
    let retry = r.headers().contains_key("retry-after");
    (s, body_text(r).await, retry)
}

async fn jobs(h: &Harness) -> serde_json::Value {
    let r = get(h, "/api/projects/default/jobs", None, true).await;
    assert_eq!(r.status(), StatusCode::OK);
    serde_json::from_str(&body_text(r).await).unwrap()
}

#[tokio::test]
async fn a_check_in_is_recorded_with_the_key_in_the_path_or_the_header() {
    let h = harness();
    let (s, body, _) = check_in(
        &h,
        &format!("/check-in/{}/nightly-export?status=error&ms=1200", h.key),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(!body.contains(&h.key), "the answer never names the key");
    let (s, _, _) = check_in(&h, "/check-in/queue-worker", Some(&h.key)).await;
    assert_eq!(s, StatusCode::OK);
    let v = jobs(&h).await;
    let slugs: Vec<&str> = v["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["slug"].as_str().unwrap())
        .collect();
    assert!(slugs.contains(&"nightly-export") && slugs.contains(&"queue-worker"));
    let nightly = v["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["slug"] == "nightly-export")
        .unwrap();
    assert_eq!(nightly["last_ok"], false);
    assert_eq!(nightly["state"], "failed");
}

#[tokio::test]
async fn a_wrong_or_missing_key_is_refused_and_never_echoed() {
    let h = harness();
    let wrong = "olk_not_a_real_key_42";
    let (s, body, _) = check_in(&h, &format!("/check-in/{wrong}/nightly"), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(!body.contains(wrong), "{body}");
    let (s, _, _) = check_in(&h, "/check-in/nightly", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, body, _) = check_in(&h, "/check-in/nightly", Some(wrong)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(!body.contains(wrong));
    assert!(jobs(&h).await["jobs"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn one_check_in_per_job_every_ten_seconds() {
    let h = harness();
    let uri = format!("/check-in/{}/backup", h.key);
    assert_eq!(check_in(&h, &uri, None).await.0, StatusCode::OK);
    let (s, _, retry) = check_in(&h, &uri, None).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert!(retry, "a 429 says when to retry");
    // Another job is not held back by the first.
    let other = format!("/check-in/{}/export", h.key);
    assert_eq!(check_in(&h, &other, None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn a_new_job_past_the_cap_is_refused_and_known_jobs_still_check_in() {
    let h = harness();
    let run = JobRun {
        at: chrono::Utc::now().timestamp(),
        ok: true,
        ms: None,
    };
    for i in 0..MAX_JOBS {
        h.state
            .meta
            .record_job_run("default", &format!("job-{i}"), &run, None, MAX_JOBS)
            .unwrap();
    }
    let (s, _, _) = check_in(&h, &format!("/check-in/{}/one-more", h.key), None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _, _) = check_in(&h, &format!("/check-in/{}/job-0", h.key), None).await;
    assert_eq!(s, StatusCode::OK);
    let n = jobs(&h).await["jobs"].as_array().unwrap().len();
    assert_eq!(i64::try_from(n).unwrap(), MAX_JOBS);
}

#[tokio::test]
async fn hostile_slugs_and_queries_are_refused_and_record_nothing() {
    let h = harness();
    for slug in openlens_testkit::hostile_job_slugs() {
        let (s, body, _) = check_in(
            &h,
            &format!("/check-in/{}/{}", h.key, url_encode(&slug)),
            None,
        )
        .await;
        assert!(s.is_client_error(), "{slug:?}: {s}");
        assert!(!body.contains(&h.key), "{slug:?}");
    }
    for (i, q) in openlens_testkit::hostile_check_in_queries()
        .iter()
        .enumerate()
    {
        let (s, body, _) = check_in(&h, &format!("/check-in/{}/q{i}?{q}", h.key), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{q}");
        assert!(!body.contains(&h.key), "{q}");
    }
    assert!(jobs(&h).await["jobs"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn the_uptime_page_explains_check_ins_then_lists_the_jobs() {
    let h = harness();
    let cookie = login_cookie(&h).await;
    let page = body_text(get(&h, "/p/default/uptime", Some(&cookie), false).await).await;
    assert!(page.contains("id=\"jobs\""));
    assert!(page.contains("/check-in/") && page.contains("checkIn("));
    let (s, _, _) = check_in(&h, &format!("/check-in/{}/nightly-export", h.key), None).await;
    assert_eq!(s, StatusCode::OK);
    let page = body_text(get(&h, "/p/default/uptime", Some(&cookie), false).await).await;
    assert!(page.contains("nightly-export"));
    assert!(page.contains("Learning"), "one check-in: still learning");
    // The owner can count jobs in availability and remove one.
    let s = post_form(&h, "/p/default/uptime/jobs/availability", &cookie, "on=on").await;
    assert!(s.is_redirection());
    assert_eq!(jobs(&h).await["in_availability"], true);
    let s = post_form(
        &h,
        "/p/default/uptime/jobs/nightly-export/remove",
        &cookie,
        "",
    )
    .await;
    assert!(s.is_redirection());
    assert!(jobs(&h).await["jobs"].as_array().unwrap().is_empty());
}
