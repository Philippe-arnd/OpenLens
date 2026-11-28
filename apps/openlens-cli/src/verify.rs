//! Checking an install, the agent rules snippet and day ranges. Moved out of main.rs as is (#307).

use super::*;

/// The wiring checklist an agent must get to all-green before handing over.
pub(super) fn verify<A: Api + ?Sized>(c: &A, project: &str) -> Result<bool, String> {
    let report = verify_report(c, project)?;
    let ok = report["ok"] == true;
    out(&report);
    Ok(ok)
}

/// The verify checklist as JSON (shared with the MCP tool).
pub(crate) fn verify_report<A: Api + ?Sized>(c: &A, project: &str) -> Result<Value, String> {
    let n = |rows: Vec<Value>| rows.first().and_then(|r| r["n"].as_i64()).unwrap_or(0);
    let recent = "timestamp >= now() - INTERVAL '1 day'";
    let events = n(c.sql(
        project,
        &format!("SELECT count(*) AS n FROM events WHERE {recent}"),
    )?);
    let identified = n(c.sql(
        project,
        &format!("SELECT count(*) AS n FROM events WHERE {recent} AND event = '$identify'"),
    )?);
    let with_role = n(c.sql(
        project,
        &format!("SELECT count(*) AS n FROM events WHERE {recent} AND role IS NOT NULL"),
    )?);
    let releases = n(c.sql(
        project,
        &format!("SELECT count(DISTINCT release) AS n FROM events WHERE {recent}"),
    )?);

    let bots = n(c.sql(
        project,
        &format!("SELECT count(*) AS n FROM raw_events WHERE {recent} AND environment = 'bot'"),
    )?);

    let (map_status, map) = c.call("GET", &format!("/api/projects/{project}/feature-map"), None)?;
    let mut features = Vec::new();
    if map_status == 200
        && let Some(m) = map["yaml"]
            .as_str()
            .and_then(|y| openlens_featuremap::parse(y).map)
    {
        // Raw events carry `feature` only for data-feature/feature() uses;
        // route entry points are resolved here with the map, like the board does.
        let rows = c.sql(
            project,
            &format!(
                "SELECT feature, path, count(*) AS n FROM events WHERE {recent} \
                 AND (feature IS NOT NULL OR path IS NOT NULL) GROUP BY feature, path"
            ),
        )?;
        let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for r in &rows {
            let n = r["n"].as_i64().unwrap_or(0);
            let key = r["feature"].as_str().map(str::to_owned).or_else(|| {
                r["path"]
                    .as_str()
                    .and_then(|p| m.feature_for_path(p))
                    .map(str::to_owned)
            });
            if let Some(k) = key {
                *counts.entry(k).or_default() += n;
            }
        }
        for f in &m.features {
            features.push(json!({ "key": f.key, "events_last_day": counts.get(&f.key).copied().unwrap_or(0) }));
        }
    }
    // Hosts stored as `unknown`: counted in no production number (D-034).
    let (env_status, envs) = c.call(
        "GET",
        &format!("/api/projects/{project}/environments"),
        None,
    )?;
    let unclassified = if env_status == 200 {
        envs["unclassified"].clone()
    } else {
        json!([])
    };
    let classified = unclassified.as_array().is_none_or(Vec::is_empty);
    let rules = if env_status == 200 {
        envs["rules"].clone()
    } else {
        json!([])
    };
    let linked = linkage(c, project, &rules, LINK_WAIT)?;
    let checks = json!([
        { "check": "events arrive", "ok": events > 0, "detail": events, "fix": "add the SDK snippet and load a page" },
        { "check": "users are identified", "ok": identified > 0, "detail": identified, "fix": "call openlens.identify(user.id, { role }) after login" },
        { "check": "roles are sent", "ok": with_role > 0, "detail": with_role, "fix": "pass the user's real role field to identify()" },
        { "check": "release is tagged", "ok": releases > 0, "detail": releases, "fix": "pass release (version or git SHA) to init or data-release" },
        { "check": "feature map pushed", "ok": map_status == 200, "detail": map["version"], "fix": "openlens-cli features push" },
        { "check": "hosts are classified", "ok": classified, "detail": unclassified, "fix": "openlens-cli env rule <host> <production|staging|preview|ignore>" },
        linked,
    ]);
    let ok = checks
        .as_array()
        .is_some_and(|a| a.iter().take(3).all(|c| c["ok"] == true))
        && map_status == 200;
    // The release layers (#524): reported, not required for `ok`.
    let layers = crate::agent_kit::release_layers(c, project)?;
    Ok(json!({ "ok": ok, "checks": checks, "features": features,
        "release_layers": layers,
        "robots_last_day": bots,
        "note": "features with 0 events have not been exercised yet; click through them once to confirm they fire" }))
}

/// The name of the linkage check in the verify report (#711).
pub(crate) const LINK_CHECK: &str = "visitor linked to the person";

/// How long the linkage check waits for the server to store and link its
/// test events (they are flushed every ~10 s, the links read every 30 s).
const LINK_WAIT: std::time::Duration = std::time::Duration::from_secs(45);

/// The linkage check (#711): an anonymous page view, then an `$identify`
/// that links its anonymous id, sent with the project key; the page view
/// must then belong to the person in `events`. The test person is marked
/// as the team's own (`$internal`) and sent in `development`, so it
/// counts in no production number. `rules` are the project's host rules,
/// to say when there is no second production host (no landing page).
pub(crate) fn linkage<A: Api + ?Sized>(
    c: &A,
    project: &str,
    rules: &Value,
    wait: std::time::Duration,
) -> Result<Value, String> {
    let fix = "check that the server is up to date; in the app, call openlens.identify(user.id, …) right after account creation and at login";
    let hosts: Vec<&str> = rules
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|r| r["env"] == "production")
                .filter_map(|r| r["host"].as_str())
                .filter(|h| !h.starts_with("*."))
                .collect()
        })
        .unwrap_or_default();
    let hosts_note = if hosts.len() < 2 {
        "the project has one production host: no landing page on another host to link".to_owned()
    } else {
        format!("production hosts linked by the SDK: {}", hosts.join(", "))
    };
    let check = |ok: bool, detail: String| json!({ "check": LINK_CHECK, "ok": ok, "detail": format!("{detail}; {hosts_note}"), "fix": fix });
    let (status, list) = c.call("GET", "/api/projects", None)?;
    let key = (status == 200)
        .then_some(&list)
        .and_then(|l| l["projects"].as_array())
        .and_then(|ps| ps.iter().find(|p| p["id"] == project))
        .and_then(|p| p["key"].as_str())
        .map(str::to_owned);
    let Some(key) = key else {
        return Ok(check(
            false,
            "not run: the project's capture key could not be read with this token".into(),
        ));
    };
    let stamp = chrono::Utc::now();
    let tag = stamp.timestamp_nanos_opt().unwrap_or_default();
    let (anon, person) = (format!("verify-anon-{tag}"), format!("verify-person-{tag}"));
    let at = |ms: i64| (stamp + chrono::Duration::milliseconds(ms)).to_rfc3339();
    let props = json!({ "$environment": "development", "$lib": "openlens-cli", "$verify": true });
    let mut page = props.clone();
    page["$pathname"] = json!("/openlens-verify");
    let mut identify = props;
    identify["$anon_distinct_id"] = json!(anon);
    identify["$internal"] = json!(true);
    let body = json!({
        "key": key,
        "batch": [
            { "event": "$pageview", "distinct_id": anon, "timestamp": at(0), "properties": page },
            { "event": "$identify", "distinct_id": person, "timestamp": at(1), "properties": identify },
        ],
    });
    let (status, v) = c.call("POST", "/capture", Some(Body::Json(body)))?;
    if status != 200 {
        return Ok(check(
            false,
            format!("the test events were refused: {}", api_error(status, &v)),
        ));
    }
    let n = |rows: Vec<Value>| rows.first().and_then(|r| r["n"].as_i64()).unwrap_or(0);
    let recent = "timestamp >= now() - INTERVAL '1 hour' AND event = '$pageview'";
    let started = std::time::Instant::now();
    loop {
        let linked = n(c.sql(
            project,
            &format!(
                "SELECT count(*) AS n FROM events WHERE {recent} AND distinct_id = '{person}'"
            ),
        )?);
        if linked > 0 {
            return Ok(check(
                true,
                format!(
                    "an anonymous page view was given to the person once identified ({} s)",
                    started.elapsed().as_secs()
                ),
            ));
        }
        if started.elapsed() >= wait {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
    let stored = n(c.sql(
        project,
        &format!("SELECT count(*) AS n FROM raw_events WHERE {recent} AND distinct_id = '{anon}'"),
    )?);
    let why = if stored > 0 {
        "the anonymous page view is stored but still not given to the person"
    } else {
        "the test page view was not stored yet"
    };
    Ok(check(false, format!("{why} after {} s", wait.as_secs())))
}

/// The block an app's own agents add to their `AGENTS.md`: keep the
/// feature map current, and keep releases measured (#524).
pub(super) const RULES_SNIPPET: &str = include_str!("rules.md");

/// Every date from `from` to `to`, inclusive, as YYYY-MM-DD (at most 366).
pub(super) fn days_between(
    from: (i32, u32, u32),
    to: (i32, u32, u32),
) -> Result<Vec<String>, String> {
    fn days_in(y: i32, m: u32) -> u32 {
        match m {
            2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }
    let (mut y, mut m, mut d) = from;
    let mut out = Vec::new();
    loop {
        if d > days_in(y, m) {
            return Err(format!("{y:04}-{m:02}-{d:02} does not exist"));
        }
        out.push(format!("{y:04}-{m:02}-{d:02}"));
        if (y, m, d) >= to {
            break;
        }
        if out.len() >= 366 {
            return Err("export at most 366 days at a time".into());
        }
        d += 1;
        if d > days_in(y, m) {
            d = 1;
            m += 1;
            if m > 12 {
                m = 1;
                y += 1;
            }
        }
    }
    Ok(out)
}
