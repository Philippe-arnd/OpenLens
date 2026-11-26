//! Who may reach which route; limits and sessions. Moved out of main.rs as is (#307).

use super::*;

#[tokio::test]
async fn the_board_is_computed_once_and_follows_new_data() {
    let h = harness();
    let batch = serde_json::json!({"key": h.key, "batch": [
        {"event": "$pageview", "distinct_id": "u1", "properties": {"$pathname": "/"}}
    ]});
    let (status, _) = post(&h, "/capture", batch.to_string().into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let boards = h.state.boards.clone();
    let before = boards.computed();
    let uri = "/api/projects/default/board";
    let (a, b, c, d) = tokio::join!(
        get(&h, uri, None, true),
        get(&h, uri, None, true),
        get(&h, uri, None, true),
        get(&h, uri, None, true)
    );
    for r in [a, b, c, d] {
        assert_eq!(r.status(), StatusCode::OK);
    }
    assert_eq!(
        boards.computed(),
        before + 1,
        "concurrent calls share one computation"
    );
    let again = get(&h, "/api/projects/default/board", None, true).await;
    assert_eq!(again.status(), StatusCode::OK);
    assert_eq!(
        boards.computed(),
        before + 1,
        "a second call is served from the cache"
    );
    // New data: the next call is still answered at once, and the board is
    // recomputed behind it.
    let (status, _) = post(&h, "/capture", batch.to_string().into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let r = get(&h, "/api/projects/default/board", None, true).await;
    assert_eq!(r.status(), StatusCode::OK);
    // Up to 20 s, as below: the recompute runs in the background.
    for _ in 0..400 {
        if boards.computed() > before + 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        boards.computed() > before + 1,
        "new data triggers a recomputation"
    );
}

/// The board page as the `scale` job loads it (D-071): `GET /p/default`,
/// whose window is the release window (D-055), twice with the same
/// session. The second load computes nothing; new data computes it again.
#[tokio::test]
async fn the_release_window_board_page_is_computed_once() {
    let h = harness();
    let owner = login_cookie(&h).await;
    let batch = serde_json::json!({"key": h.key, "batch": [
        {"event": "$pageview", "distinct_id": "u1", "properties": {"$pathname": "/", "$release": "1.0.0"}}
    ]});
    let (status, _) = post(&h, "/capture", batch.to_string().into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let boards = h.state.boards.clone();
    let before = boards.computed();
    let cold = get(&h, "/p/default", Some(&owner), false).await;
    assert_eq!(cold.status(), StatusCode::OK);
    let cold = body_text(cold).await;
    // "Can't show that": the page of a board that failed.
    assert!(!cold.contains("t show that"), "the board was computed");
    assert_eq!(boards.computed(), before + 1, "the cold page computes it");
    let warm = get(&h, "/p/default", Some(&owner), false).await;
    assert_eq!(warm.status(), StatusCode::OK);
    let _ = body_text(warm).await;
    assert_eq!(
        boards.computed(),
        before + 1,
        "the second page is served from the cache"
    );
    // A real data change still invalidates it (recomputed behind a stale
    // page, up to 20 s on a loaded runner).
    let (status, _) = post(&h, "/capture", batch.to_string().into_bytes(), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let _ = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    for _ in 0..400 {
        if boards.computed() > before + 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(boards.computed() > before + 1, "new data recomputes it");
}

/// A board page loaded twice with no new data is answered from the cache
/// as a whole (#480): the revenue on it used to be queried again on every
/// load, which made a cached page as slow as a cold one.
#[tokio::test]
async fn a_cached_board_page_runs_no_query_again() {
    let h = harness();
    let owner = login_cookie(&h).await;
    h.state
        .meta
        .save_feature_map(
            "default",
            "features:\n  - key: checkout\n    name: Checkout\n    success: order_paid\n    entry: { routes: ['/checkout'] }\n",
        )
        .unwrap();
    let paid = |who: &str| {
        serde_json::json!({"key": h.key, "batch": [
            {"event": "order_paid", "distinct_id": who, "properties": {"revenue": 10, "currency": "EUR"}}
        ]})
        .to_string()
        .into_bytes()
    };
    let (status, _) = post(&h, "/capture", paid("u1"), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let first = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    assert!(first.contains("10.00 EUR"), "one order of 10");
    let computed = h.state.boards.computed();

    // Rows written behind the cache's back (no generation bump): a page
    // that queries nothing again cannot see them.
    let (status, _) = post(&h, "/capture", paid("u2"), false).await;
    assert_eq!(status, StatusCode::OK);
    h.state.writer.lock().unwrap().flush().unwrap();
    let second = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    assert_eq!(h.state.boards.computed(), computed, "the board is cached");
    assert!(
        second.contains("10.00 EUR") && !second.contains("20.00 EUR"),
        "the revenue comes from the cache too"
    );

    // New data: board and revenue follow it together.
    h.state.boards.bump("default");
    let _stale = get(&h, "/p/default", Some(&owner), false).await;
    // Up to 20 s: the recompute runs in the background, slow on a loaded runner.
    for _ in 0..400 {
        if h.state.boards.computed() > computed {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let after = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    assert!(after.contains("20.00 EUR"), "the revenue follows new data");
}

// The route table: `access/table.rs` (split for the file limit).
mod table;
use table::{Access::*, ROUTES};

/// `(method, path)` of every `.route(…)` in the server's source.
pub(crate) fn routes_in_source() -> std::collections::BTreeSet<(String, String)> {
    let mut out = std::collections::BTreeSet::new();
    // Every source file, modules in subdirectories included (#307).
    let mut dirs = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    let mut files = Vec::new();
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                files.push(path);
            }
        }
    }
    for file in files {
        let src = std::fs::read_to_string(file).unwrap();
        let mut rest = src.as_str();
        while let Some(i) = rest.find(".route(") {
            rest = &rest[i + ".route(".len()..];
            let Some(open) = rest.find('"') else { break };
            let Some(len) = rest[open + 1..].find('"') else {
                break;
            };
            let path = &rest[open + 1..open + 1 + len];
            let (mut depth, mut end) = (1usize, 0usize);
            for (j, c) in rest.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    end = j;
                    break;
                }
            }
            let handlers = &rest[..end];
            for m in ["get", "post", "put", "delete", "patch"] {
                let call = format!("{m}(");
                if handlers.match_indices(&call).any(|(k, _)| {
                    k == 0
                        || !handlers[..k]
                            .chars()
                            .last()
                            .is_some_and(|c| c.is_alphanumeric() || c == '_')
                }) {
                    out.insert((m.to_uppercase(), path.to_owned()));
                }
            }
        }
    }
    out
}

#[test]
fn every_route_is_in_the_access_table() {
    let table: std::collections::BTreeSet<(String, String)> = ROUTES
        .iter()
        .map(|(m, p, _)| ((*m).to_owned(), (*p).to_owned()))
        .collect();
    let source = routes_in_source();
    let missing: Vec<_> = source.difference(&table).collect();
    let stale: Vec<_> = table.difference(&source).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "add to ROUTES: {missing:?}; remove from ROUTES: {stale:?}"
    );
}

fn sample(path: &str) -> String {
    path.split('/')
        .map(|s| match s {
            "{project}" => "default",
            "{mode}" => "dark",
            s if s.starts_with('{') => "x1",
            s => s,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether the response refuses: a 4xx, or a redirect to the login page.
async fn refused_as(
    h: &Harness,
    method: &str,
    path: &str,
    cookie: Option<&str>,
) -> Result<(), String> {
    let mut req = Request::builder()
        .method(method)
        .uri(sample(path))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin");
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    let r = router(h.state.clone())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let to = r
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let s = r.status();
    if s.is_client_error() || (s.is_redirection() && to.starts_with("/login")) {
        Ok(())
    } else {
        Err(format!("{s} {to}"))
    }
}

#[tokio::test]
async fn no_route_serves_someone_it_should_not() {
    let h = harness();
    let viewer = invite(
        &h,
        serde_json::json!({"email": "viewer@example.com", "role": "viewer", "projects": ["default"]}),
    )
    .await;
    let outsider = invite(
        &h,
        serde_json::json!({"email": "out@example.com", "role": "viewer", "projects": ["elsewhere"]}),
    )
    .await;
    let mut leaks = Vec::new();
    for (method, path, access) in ROUTES {
        let mut deny = Vec::new();
        if !matches!(access, Public) {
            deny.push(("anonymous", None));
        }
        if matches!(access, Read | Write | Shared) {
            deny.push(("viewer of another project", Some(outsider.as_str())));
        }
        if matches!(access, Admin | Write) {
            deny.push(("viewer", Some(viewer.as_str())));
        }
        for (who, cookie) in deny {
            if let Err(got) = refused_as(&h, method, path, cookie).await {
                leaks.push(format!("{method} {path} as {who}: {got}"));
            }
        }
    }
    assert!(leaks.is_empty(), "{leaks:#?}");
}

#[tokio::test]
async fn read_only_roles_post_on_shared_routes_only() {
    // Comments, timeline notes and Ask are open to every role that sees
    // the project (#339): no path rule says so, only the handler's
    // extractor (or its absence).
    let h = harness();
    let viewer = invite(
        &h,
        serde_json::json!({"email": "viewer@example.com", "role": "viewer", "projects": ["default"]}),
    )
    .await;
    let mut refused = Vec::new();
    for (method, path, access) in ROUTES {
        if *method == "GET" || !matches!(access, Shared) {
            continue;
        }
        if refused_as(&h, method, path, Some(&viewer)).await.is_ok() {
            let r = router(h.state.clone())
                .oneshot(
                    Request::builder()
                        .method(*method)
                        .uri(sample(path))
                        .header("cookie", &viewer)
                        .header("sec-fetch-site", "same-origin")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            // A 400 or 422 (an empty form) is fine; a 403 or the login is not.
            if r.status() == StatusCode::FORBIDDEN || r.status() == StatusCode::UNAUTHORIZED {
                refused.push(format!("{method} {path}: {}", r.status()));
            }
        }
    }
    assert!(refused.is_empty(), "{refused:#?}");
}

async fn burst_from(h: &Harness, uri: &str, body: &str, peer: &str, forged: bool) -> usize {
    let app = router(h.state.clone());
    let mut limited = 0;
    for i in 0..1000 {
        let mut req = Request::post(uri)
            .header("content-type", "application/json")
            .header("x-openlens-key", h.key.as_str());
        if forged {
            req = req.header("x-forwarded-for", format!("198.18.0.{}", i % 250));
        }
        let mut req = req.body(Body::from(body.to_owned())).unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo::<std::net::SocketAddr>(
                peer.parse().unwrap(),
            ));
        let r = app.clone().oneshot(req).await.unwrap();
        if r.status() == StatusCode::TOO_MANY_REQUESTS {
            limited += 1;
        }
    }
    limited
}

#[tokio::test]
async fn direct_clients_are_rate_limited_whatever_headers_they_send() {
    let h = harness();
    // The client is counted before the body is read: an invalid body
    // (answered at once) shows the limit without writing anything.
    let ev = "not json";
    assert!(burst_from(&h, "/capture", ev, "192.0.2.7:4000", false).await > 0);
    assert!(
        burst_from(&h, "/capture", ev, "192.0.2.8:4000", true).await > 0,
        "a forged X-Forwarded-For does not hide a direct client"
    );
    assert!(burst_from(&h, "/r", ev, "192.0.2.9:4000", false).await > 0);
    // Behind a private proxy, each forwarded client has its own budget.
    assert_eq!(
        burst_from(&h, "/capture", ev, "10.0.0.2:4000", true).await,
        0
    );
}

#[tokio::test]
async fn the_admin_token_cannot_forge_a_session() {
    use hmac::{Hmac, Mac};
    let h = harness();
    let exp = chrono::Utc::now().timestamp() + 3600;
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"adm_test").unwrap();
    mac.update(format!("session:{exp}:owner").as_bytes());
    let forged = format!(
        "ol_session={exp}.owner.{}",
        hex::encode(mac.finalize().into_bytes())
    );
    let r = get(&h, "/p/default", Some(&forged), false).await;
    assert!(
        r.status().is_redirection(),
        "a token copied from the store signs nothing"
    );
    // A real sign-in still works.
    let owner = login_cookie(&h).await;
    assert_eq!(
        get(&h, "/p/default", Some(&owner), false).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_nearly_full_disk_stops_writes_and_readiness() {
    let mut h = harness();
    let ready = |h: &Harness| {
        let state = h.state.clone();
        async move {
            router(state)
                .oneshot(Request::get("/readyz").body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status()
        }
    };
    assert_eq!(ready(&h).await, StatusCode::OK);
    assert_eq!(
        post(&h, "/capture", batch(&h.key), false).await.0,
        StatusCode::OK
    );

    // 1 byte free out of 100: under the fixed 5 % floor.
    h.state.disk_space = |_| Some((1, 100));
    assert_eq!(ready(&h).await, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        post(&h, "/capture", batch(&h.key), false).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let chunk = format!(
        r#"{{"key":"{}","session_id":"s-1","distinct_id":"u1","seq":0,"events":[{{"t":1}}]}}"#,
        h.key
    );
    assert_eq!(
        post(&h, "/r", chunk.into_bytes(), false).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn the_server_closes_connections_that_never_finish_their_headers() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let h = harness();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(openlens_server::serve(l, router(h.state.clone()), async {
        let _ = stopped.await;
    }));

    // A normal request is served, and handlers still see the client address.
    let mut ok = tokio::net::TcpStream::connect(addr).await.unwrap();
    ok.write_all(b"GET /healthz HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut reply = String::new();
    ok.read_to_string(&mut reply).await.unwrap();
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");

    // Half the headers, then nothing: cut off after the header timeout.
    let mut slow = tokio::net::TcpStream::connect(addr).await.unwrap();
    slow.write_all(b"GET /healthz HTTP/1.1\r\nhost: x\r\n")
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let mut buf = Vec::new();
    let read = tokio::time::timeout(
        openlens_server::HEADER_READ_TIMEOUT + std::time::Duration::from_secs(5),
        slow.read_to_end(&mut buf),
    )
    .await;
    assert!(read.is_ok(), "the connection was still open");
    assert!(
        started.elapsed()
            >= openlens_server::HEADER_READ_TIMEOUT - std::time::Duration::from_secs(1)
    );

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn api_tokens_are_made_in_settings_and_shown_once() {
    let h = harness();
    let owner = login_cookie(&h).await;
    let (s, page) = post_form_body(
        &h,
        "/settings/tokens",
        &owner,
        "label=ci&scope=read&project=default",
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let plain = page
        .split("<code>olt_")
        .nth(1)
        .and_then(|r| r.split("</code>").next())
        .map(|t| format!("olt_{t}"))
        .unwrap();
    assert!(plain.starts_with("olt_"), "{plain}");
    // The value works on the API, and the list never shows it again.
    let api = router(h.state.clone())
        .oneshot(
            Request::get("/api/projects")
                .header("authorization", format!("Bearer {plain}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(api.status(), StatusCode::OK);
    let list = body_text(get(&h, "/settings/tokens", Some(&owner), false).await).await;
    assert!(list.contains("ci") && !list.contains(&plain));
    let id = h.state.meta.api_tokens().unwrap()[0].id.clone();
    post_form(&h, &format!("/settings/tokens/{id}/revoke"), &owner, "").await;
    assert!(h.state.meta.api_tokens().unwrap().is_empty());
}

#[tokio::test]
async fn sign_out_everywhere_refuses_every_earlier_cookie() {
    let h = harness();
    let mine = login_cookie(&h).await;
    let other = login_cookie(&h).await;
    let r = router(h.state.clone())
        .oneshot(
            Request::post("/settings/sessions/revoke")
                .header("cookie", &mine)
                .header("sec-fetch-site", "same-origin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let renewed = r.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    for old in [&mine, &other] {
        let page = get(&h, "/projects", Some(old), false).await;
        assert!(page.status().is_redirection(), "an old cookie still works");
    }
    assert_eq!(
        get(&h, "/projects", Some(&renewed), false).await.status(),
        StatusCode::OK
    );
    // The generation is stored, so a restart keeps the old cookies out.
    assert_eq!(
        openlens_server::tokens::session_generation(&h.state.meta),
        1
    );
    // The admin token still works.
    assert!(login_cookie(&h).await.starts_with("ol_session="));
}

#[tokio::test]
async fn new_data_in_another_project_keeps_this_board_cached() {
    let h = harness();
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let uri = "/api/projects/default/board";
    assert_eq!(get(&h, uri, None, true).await.status(), StatusCode::OK);
    let computed = h.state.boards.computed();
    // Another project's flush, compaction or erasure (#420).
    h.state.boards.bump("other");
    assert_eq!(get(&h, uri, None, true).await.status(), StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(h.state.boards.computed(), computed, "no recomputation");
    h.state.boards.bump("default");
    assert_eq!(get(&h, uri, None, true).await.status(), StatusCode::OK);
    // The recomputation runs in the background; under the whole suite's
    // load it can take seconds, so wait up to 10 s for it.
    for _ in 0..500 {
        if h.state.boards.computed() > computed {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(h.state.boards.computed(), computed + 1, "its own data does");
}

#[tokio::test]
async fn instance_pages_keep_the_project_menu_and_a_breadcrumb() {
    let h = harness();
    let owner = login_cookie(&h).await;
    // Three scopes (#438): the instance's, mine, and the project's.
    for (page, scope) in [
        ("/settings/ai", "Instance settings"),
        ("/settings/storage", "Instance settings"),
        ("/settings/about", "Instance settings"),
        ("/users", "Instance settings"),
        ("/me", "My profile"),
        ("/me/sign-in", "My profile"),
        ("/me/appearance", "My profile"),
    ] {
        let html = body_text(get(&h, page, Some(&owner), false).await).await;
        // The only project's menu (#403), the scope's tabs and the trail.
        assert!(html.contains("class=\"side-nav\""), "{page}");
        assert!(html.contains("href=\"/p/default/events\""), "{page}");
        assert!(html.contains(&format!("aria-label=\"{scope}\"")), "{page}");
        assert!(html.contains("aria-label=\"Breadcrumb\""), "{page}");
        // The theme is chosen in My profile, not in the sidebar.
        assert!(!html.contains("href=\"/theme/dark\""), "{page}");
    }
    let settings = body_text(get(&h, "/p/default/settings", Some(&owner), false).await).await;
    assert!(settings.contains("list=\"tz-all\"") && settings.contains("Europe/Paris"));
    assert!(
        !settings.contains("name=\"quiet_start\""),
        "alerts have their own tab"
    );
    let alerts = body_text(get(&h, "/p/default/settings/alerts", Some(&owner), false).await).await;
    assert!(alerts.contains("name=\"quiet_start\""));
    for tab in ["install", "sharing", "data"] {
        let r = get(
            &h,
            &format!("/p/default/settings/{tab}"),
            Some(&owner),
            false,
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK, "{tab}");
    }
    assert_eq!(
        get(&h, "/p/default/settings/nope", Some(&owner), false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    // The setup progress (#403): required steps as a bar, the rest optional.
    assert!(settings.contains("aria-label=\"Setup\""), "{settings}");
    // The optional steps are folded under their count (#546).
    assert!(settings.contains("An alert channel") && settings.contains("optional steps"));
}

/// The form of `html` whose action is exactly `action`.
fn form_at<'a>(html: &'a str, action: &str) -> &'a str {
    let at = html
        .find(&format!("action=\"{action}\""))
        .unwrap_or_else(|| panic!("no form posts to {action}"));
    let start = html[..at].rfind("<form").unwrap();
    &html[start..at + html[at..].find("</form>").unwrap()]
}

/// The fields of a form, as (name, type) — `select` for a select.
fn form_fields(form: &str) -> Vec<(String, String)> {
    form.split('<')
        .filter(|t| {
            t.starts_with("input ") || t.starts_with("select ") || t.starts_with("textarea ")
        })
        .filter_map(|t| {
            let tag = &t[..t.find('>').unwrap_or(t.len())];
            let attr = |k: &str| {
                let v = tag.split(&format!(" {k}=\"")).nth(1)?;
                Some(v[..v.find('"')?].to_owned())
            };
            let kind = attr("type")
                .unwrap_or_else(|| tag[..tag.find(' ').unwrap_or(tag.len())].to_owned());
            Some((attr("name")?, kind))
        })
        .collect()
}

#[tokio::test]
async fn the_settings_forms_post_the_fields_their_handlers_read() {
    // #546 changed the markup of these forms only: each still posts to
    // the same action the fields its handler reads, and posting the form
    // back as rendered changes what the page then shows.
    let h = harness();
    let owner = login_cookie(&h).await;
    let names = |f: &[(String, String)]| {
        f.iter()
            .map(|x| x.0.clone())
            .collect::<std::collections::BTreeSet<_>>()
    };

    // General: `SaveForm` in server/src/settings.rs.
    let page = body_text(get(&h, "/p/default/settings", Some(&owner), false).await).await;
    let general = form_fields(form_at(&page, "/p/default/settings"));
    assert_eq!(
        names(&general),
        ["app_url", "name", "timezone"].map(String::from).into(),
        "{general:?}"
    );
    assert!(form_at(&page, "/p/default/settings").contains("method=\"post\""));
    let body = general
        .iter()
        .map(|(n, _)| match n.as_str() {
            "name" => "name=Renamed%20shop",
            "timezone" => "timezone=Pacific%2FAuckland",
            _ => "app_url=",
        })
        .collect::<Vec<_>>()
        .join("&");
    assert_eq!(
        post_form(&h, "/p/default/settings", &owner, &body).await,
        StatusCode::SEE_OTHER
    );
    let after = body_text(get(&h, "/p/default/settings", Some(&owner), false).await).await;
    let form = form_at(&after, "/p/default/settings");
    assert!(
        form.contains("name=\"name\" value=\"Renamed shop\""),
        "{form}"
    );
    assert!(
        form.contains("name=\"timezone\" value=\"Pacific/Auckland\""),
        "{form}"
    );

    // Alerts: `AlertsForm`. Every switch on, every threshold changed.
    let uri = "/p/default/settings/alerts";
    let page = body_text(get(&h, uri, Some(&owner), false).await).await;
    let alerts = form_fields(form_at(&page, uri));
    assert_eq!(
        names(&alerts),
        [
            "budget_burn",
            "budget_spent",
            "cert",
            "down",
            "down_wait",
            "email",
            "failing",
            "health_path",
            "ntfy",
            "quiet_end",
            "quiet_start",
            "regressions",
            "traffic",
            "webhook",
        ]
        .map(String::from)
        .into(),
        "{alerts:?}"
    );
    let value = |n: &str| match n {
        "down_wait" => "7",
        "health_path" => "%2Freadyz",
        "quiet_start" => "22",
        "quiet_end" => "6",
        "ntfy" => "ops-alerts",
        _ => "",
    };
    let body = alerts
        .iter()
        .map(|(n, kind)| {
            if kind == "checkbox" {
                format!("{n}=on")
            } else {
                format!("{n}={}", value(n))
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    assert_eq!(
        post_form(&h, uri, &owner, &body).await,
        StatusCode::SEE_OTHER,
        "{body}"
    );
    let after = body_text(get(&h, uri, Some(&owner), false).await).await;
    let form = form_at(&after, uri);
    for tag in form
        .split("<input ")
        .skip(1)
        .filter(|t| t.contains("type=\"checkbox\""))
    {
        let tag = &tag[..tag.find('>').unwrap()];
        assert!(
            tag.contains("role=\"switch\"") && tag.contains(" checked"),
            "{tag}"
        );
    }
    for shown in [
        "name=\"down_wait\" min=\"1\" max=\"60\" value=\"7\"",
        "name=\"health_path\" value=\"/readyz\"",
        "name=\"ntfy\" value=\"ops-alerts\"",
        "value=\"22\" selected",
        "value=\"6\" selected",
    ] {
        assert!(form.contains(shown), "{shown}: {form}");
    }
    for page in [&page, &after] {
        assert!(!page.contains(" style=") && !page.contains("<script"));
    }
}

#[tokio::test]
async fn the_board_tells_admins_what_setup_is_left() {
    let h = harness();
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    let board = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    assert!(board.contains("Finish in Settings"), "no alert channel yet");
}

#[tokio::test]
async fn errors_pages_and_events_pages_do_not_compute_the_board() {
    let h = harness();
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    for page in ["/p/default/errors", "/p/default/pages", "/p/default/events"] {
        let r = get(&h, page, Some(&owner), false).await;
        assert_eq!(r.status(), StatusCode::OK, "{page}");
    }
    // Only their filter bar's options were read (#329).
    assert_eq!(h.state.boards.computed(), 0);
    // The board page streams (#301): read it to the end first.
    let board = get(&h, "/p/default", Some(&owner), false).await;
    assert_eq!(board.status(), StatusCode::OK);
    body_text(board).await;
    assert_eq!(
        h.state.boards.computed(),
        1,
        "the board page still computes it"
    );
}

#[tokio::test]
async fn a_cold_board_streams_its_shell_first_and_a_warm_one_does_not() {
    let h = harness();
    let (status, _) = post(&h, "/capture", batch(&h.key), false).await;
    assert_eq!(status, StatusCode::OK);
    openlens_server::flush_now(&h.state).unwrap();
    let owner = login_cookie(&h).await;
    // Cold, never computed: the shell with its waiting panel (D-073), then
    // the board in the same page.
    let cold = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    let loading = cold.find("board-waiting").unwrap();
    let kpis = cold.find("Daily active users").unwrap();
    assert!(loading < kpis, "the waiting panel comes before the board");
    assert!(cold.contains("Computing the board…") && cold.contains("a few seconds"));
    assert!(cold.contains(r#"aria-live="polite""#));
    assert_eq!(cold.matches("<main>").count(), 1, "one page");
    assert!(cold.trim_end().ends_with("</html>"));
    // Warm: the cached board, with no waiting panel.
    let warm = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    assert!(warm.contains("Daily active users") && !warm.contains("board-waiting"));
    assert!(!warm.contains("Computing the board"));
}

/// Admin pages take the typed extractors (#339): the bearer token is the
/// owner there too, a viewer gets 403, and nobody signed in goes to login.
#[tokio::test]
async fn admin_pages_take_the_owner_token_and_refuse_viewers() {
    let h = harness();
    for uri in ["/settings/storage", "/settings/tokens", "/audit"] {
        assert_eq!(
            get(&h, uri, None, true).await.status(),
            StatusCode::OK,
            "{uri}"
        );
        assert!(
            get(&h, uri, None, false).await.status().is_redirection(),
            "{uri}"
        );
    }
    let owner = login_cookie(&h).await;
    let viewer = invite(
        &h,
        serde_json::json!({"email": "v@example.com", "role": "viewer"}),
    )
    .await;
    for uri in [
        "/settings/storage",
        "/settings/tokens",
        "/audit",
        "/p/default/sql",
    ] {
        assert_eq!(
            get(&h, uri, Some(&owner), false).await.status(),
            StatusCode::OK,
            "{uri}"
        );
        assert_eq!(
            get(&h, uri, Some(&viewer), false).await.status(),
            StatusCode::FORBIDDEN,
            "{uri}"
        );
    }
}

/// The OpenAPI document lists every `/api` route of the source and nothing
/// else, and `/api/v1` serves the same routes (#312).
#[tokio::test]
async fn the_api_contract_matches_the_routes_and_v1_serves_them() {
    let h = harness();
    let r = get(&h, "/api/v1/openapi.json", None, false).await;
    assert_eq!(r.status(), StatusCode::OK);
    let doc: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap())
            .unwrap();
    let mut documented = std::collections::BTreeSet::new();
    for (path, ops) in doc["paths"].as_object().unwrap() {
        for method in ops.as_object().unwrap().keys() {
            documented.insert((method.to_ascii_uppercase(), format!("/api{path}")));
        }
    }
    let in_source: std::collections::BTreeSet<(String, String)> = routes_in_source()
        .into_iter()
        .filter(|(_, p)| p.starts_with("/api/"))
        .collect();
    assert_eq!(
        documented, in_source,
        "openapi.rs ROUTES and the source differ"
    );
    // The versioned prefix answers like the alias.
    let a = get(&h, "/api/projects", None, true).await;
    let b = get(&h, "/api/v1/projects", None, true).await;
    assert_eq!((a.status(), b.status()), (StatusCode::OK, StatusCode::OK));
    let body = |r: axum::response::Response| async {
        axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap()
    };
    assert_eq!(body(a).await, body(b).await);
    assert_eq!(
        get(&h, "/api/v1/projects", None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
}
