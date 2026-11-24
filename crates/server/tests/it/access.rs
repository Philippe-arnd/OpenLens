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
    for _ in 0..100 {
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

/// Who may reach a route (#294). The table must list every route of the
/// router: [`every_route_is_in_the_access_table`] fails otherwise.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Access {
    /// Anyone (ingestion, assets, login, share links).
    Public,
    /// The bearer API.
    Api,
    /// Any signed-in person.
    Signed,
    /// Admins only.
    Admin,
    /// A person who may see the project.
    Read,
    /// An admin who may see the project.
    Write,
    /// A person who may see the project, read-only roles included
    /// (comments, notes, Ask).
    Shared,
}
use Access::*;

const ROUTES: &[(&str, &str, Access)] = &[
    ("GET", "/api/projects", Api),
    ("DELETE", "/api/projects/{project}", Api),
    ("DELETE", "/api/projects/{project}/people/{who}/forget", Api),
    ("DELETE", "/api/users/{id}", Api),
    (
        "PUT",
        "/api/projects/{project}/sourcemaps/{release}/{bundle}",
        Api,
    ),
    ("POST", "/api/projects", Api),
    ("PATCH", "/api/projects/{project}", Api),
    ("POST", "/api/projects/{project}/sql", Api),
    ("GET", "/api/projects/{project}/board", Api),
    ("GET", "/api/projects/{project}/errors", Api),
    ("GET", "/api/projects/{project}/breakdown", Api),
    ("GET", "/api/projects/{project}/people/{distinct_id}", Api),
    ("GET", "/api/projects/{project}/feature-map", Api),
    ("PUT", "/api/projects/{project}/feature-map", Api),
    ("GET", "/api/projects/{project}/feature-map/health", Api),
    ("GET", "/api/projects/{project}/feature-map/versions", Api),
    (
        "GET",
        "/api/projects/{project}/feature-map/features/{key}",
        Api,
    ),
    ("GET", "/api/projects/{project}/tickets", Api),
    ("POST", "/api/projects/{project}/tickets", Api),
    ("POST", "/api/feature-map/validate", Api),
    ("POST", "/api/projects/{project}/import", Api),
    ("GET", "/api/projects/{project}/export", Api),
    (
        "GET",
        "/api/projects/{project}/replays/{session}/comments",
        Api,
    ),
    (
        "POST",
        "/api/projects/{project}/replays/{session}/comments",
        Api,
    ),
    ("GET", "/api/projects/{project}/flows", Api),
    ("GET", "/api/projects/{project}/status", Api),
    ("GET", "/api/projects/{project}/churn", Api),
    ("GET", "/api/projects/{project}/goals", Api),
    ("POST", "/api/projects/{project}/goals", Api),
    ("GET", "/api/audit", Api),
    ("GET", "/api/tokens", Api),
    ("POST", "/api/tokens", Api),
    ("DELETE", "/api/tokens/{id}", Api),
    ("GET", "/api/projects/{project}/timeline", Api),
    ("POST", "/api/projects/{project}/timeline", Api),
    ("POST", "/api/login-links", Api),
    ("GET", "/api/users", Api),
    ("PATCH", "/api/users/{id}", Api),
    ("GET", "/api/projects/{project}/flags", Api),
    ("DELETE", "/api/projects/{project}/flags/{key}", Api),
    ("PUT", "/api/projects/{project}/flags/{key}", Api),
    ("GET", "/p/{project}/flags", Read),
    ("POST", "/p/{project}/flags/{key}", Write),
    ("POST", "/capture", Public),
    ("POST", "/e", Public),
    ("GET", "/flags", Public),
    ("POST", "/r", Public),
    ("GET", "/healthz", Public),
    ("GET", "/readyz", Public),
    ("GET", "/metrics", Api),
    ("GET", "/version", Public),
    ("GET", "/c.js", Public),
    ("GET", "/r.js", Public),
    ("GET", "/s.js", Public),
    ("GET", "/agents.md", Public),
    ("GET", "/api/projects/{project}/digest", Api),
    ("GET", "/oidc/start", Public),
    ("GET", "/oidc/callback", Public),
    ("GET", "/api/projects/{project}/replays", Api),
    ("GET", "/api/projects/{project}/replays/{session}", Api),
    ("GET", "/api/projects/{project}/surveys", Api),
    ("DELETE", "/api/projects/{project}/surveys/{key}", Api),
    ("PUT", "/api/projects/{project}/surveys/{key}", Api),
    ("GET", "/p/{project}/surveys", Read),
    ("POST", "/p/{project}/surveys/{key}", Write),
    ("GET", "/", Signed),
    ("GET", "/login", Public),
    ("POST", "/login", Public),
    ("POST", "/login/email", Public),
    ("GET", "/projects", Signed),
    ("POST", "/projects", Admin),
    ("GET", "/welcome", Signed),
    ("POST", "/welcome", Admin),
    ("GET", "/users", Admin),
    ("POST", "/users", Admin),
    ("GET", "/users/{id}", Admin),
    ("POST", "/users/{id}", Admin),
    ("POST", "/users/{id}/{action}", Admin),
    ("GET", "/me", Signed),
    ("POST", "/me", Signed),
    ("POST", "/logout", Signed),
    ("GET", "/l/{token}", Public),
    ("POST", "/l/{token}", Public),
    ("GET", "/p/{project}", Read),
    ("GET", "/p/{project}/events", Read),
    ("GET", "/p/{project}/errors", Read),
    ("GET", "/p/{project}/pages", Read),
    ("GET", "/p/{project}/replays", Read),
    ("GET", "/p/{project}/replays/{session}", Read),
    ("POST", "/p/{project}/replays/{session}/comments", Shared),
    (
        "POST",
        "/p/{project}/replays/{session}/comments/{id}/delete",
        Shared,
    ),
    ("GET", "/p/{project}/replays/{session}/timeline.json", Read),
    ("GET", "/p/{project}/replays/{session}/events.json", Read),
    ("GET", "/p/{project}/watch", Read),
    ("GET", "/p/{project}/heatmap", Read),
    ("GET", "/p/{project}/icon", Read),
    ("GET", "/p/{project}/timeline", Read),
    ("POST", "/p/{project}/timeline", Shared),
    ("POST", "/p/{project}/timeline/{nid}/delete", Shared),
    ("POST", "/p/{project}/settings/share", Write),
    ("POST", "/p/{project}/settings/share/{lid}/revoke", Write),
    ("GET", "/s/{token}", Public),
    ("POST", "/p/{project}/settings/alerts", Write),
    ("POST", "/p/{project}/settings/alerts/test", Write),
    ("GET", "/p/{project}/uptime", Read),
    ("GET", "/settings/ai", Admin),
    ("POST", "/settings/ai", Admin),
    ("GET", "/p/{project}/ask", Read),
    ("POST", "/p/{project}/ask", Shared),
    ("POST", "/p/{project}/ask/confirm", Write),
    ("POST", "/p/{project}/ask/clear", Shared),
    ("POST", "/p/{project}/ask/privacy", Write),
    ("GET", "/p/{project}/ask/sent", Read),
    ("GET", "/p/{project}/ask/export.md", Read),
    ("GET", "/p/{project}/tickets", Read),
    ("POST", "/p/{project}/tickets", Write),
    ("GET", "/p/{project}/tickets/new", Read),
    ("POST", "/p/{project}/tickets/target", Write),
    ("GET", "/settings/tokens", Admin),
    ("POST", "/settings/tokens", Admin),
    ("POST", "/settings/tokens/{id}/revoke", Admin),
    ("POST", "/settings/sessions/revoke", Admin),
    ("GET", "/settings/storage", Admin),
    ("POST", "/settings/storage/s3", Admin),
    ("POST", "/settings/storage/backups", Admin),
    ("POST", "/settings/storage/backups/run", Admin),
    ("GET", "/settings/storage/backups/latest.tar.gz", Admin),
    ("GET", "/p/{project}/features", Read),
    ("POST", "/p/{project}/features", Write),
    ("GET", "/p/{project}/features/{file}", Read),
    ("GET", "/p/{project}/people/{who}", Read),
    ("POST", "/p/{project}/people/{who}/forget", Write),
    ("GET", "/p/{project}/people/{who}/export.json", Read),
    ("GET", "/audit", Admin),
    ("GET", "/p/{project}/breakdown", Read),
    ("GET", "/p/{project}/audience", Read),
    ("GET", "/p/{project}/forms", Read),
    ("GET", "/p/{project}/acquisition", Read),
    ("GET", "/p/{project}/churn", Read),
    ("POST", "/p/{project}/churn/audience", Write),
    ("GET", "/p/{project}/churn/{file}", Read),
    ("GET", "/p/{project}/revenue", Read),
    ("GET", "/p/{project}/goals", Read),
    ("POST", "/p/{project}/goals", Write),
    ("POST", "/p/{project}/goals/{gid}/delete", Write),
    ("POST", "/p/{project}/objectives", Write),
    ("POST", "/p/{project}/objectives/{oid}/delete", Write),
    ("GET", "/p/{project}/flows", Read),
    ("GET", "/p/{project}/live", Read),
    ("GET", "/p/{project}/sql", Read),
    ("POST", "/p/{project}/sql", Write),
    ("GET", "/p/{project}/flags-edit", Read),
    ("POST", "/p/{project}/flags-edit", Write),
    ("POST", "/p/{project}/flags/{key}/delete", Write),
    ("GET", "/p/{project}/surveys-edit", Read),
    ("POST", "/p/{project}/surveys-edit", Write),
    ("POST", "/p/{project}/surveys/{key}/delete", Write),
    ("GET", "/p/{project}/settings", Read),
    ("POST", "/p/{project}/settings", Write),
    ("POST", "/p/{project}/settings/rotate-key", Write),
    ("POST", "/p/{project}/settings/delete", Write),
    ("GET", "/p/{project}/export", Read),
    ("GET", "/p/{project}/digest", Read),
    ("GET", "/p/{project}/heatmap.json", Read),
    ("GET", "/p/{project}/heatmap/overlay", Read),
    ("GET", "/assets/player.js", Public),
    ("GET", "/assets/player.css", Public),
    ("GET", "/assets/app.css", Public),
    ("GET", "/assets/favicon.svg", Public),
    ("GET", "/favicon.ico", Public),
    ("GET", "/theme/{mode}", Public),
    ("GET", "/nav/{mode}", Public),
];

/// `(method, path)` of every `.route(…)` in the server's source.
fn routes_in_source() -> std::collections::BTreeSet<(String, String)> {
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
    // Cold: the shell with its loading line, then the board in the same page.
    let cold = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    let loading = cold.find("page-loading").unwrap();
    let kpis = cold.find("Daily active users").unwrap();
    assert!(loading < kpis, "the loading line comes before the board");
    assert!(cold.trim_end().ends_with("</html>"));
    // Warm: the cached board, with no loading line.
    let warm = body_text(get(&h, "/p/default", Some(&owner), false).await).await;
    assert!(warm.contains("Daily active users") && !warm.contains("page-loading"));
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
