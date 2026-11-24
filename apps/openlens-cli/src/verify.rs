//! Checking an install, the agent rules snippet and day ranges. Moved out of main.rs as is (#307).

use super::*;

/// The wiring checklist an agent must get to all-green before handing over.
pub(super) fn verify(c: &Client, project: &str) -> Result<bool, String> {
    let report = verify_report(c, project)?;
    let ok = report["ok"] == true;
    out(&report);
    Ok(ok)
}

/// The verify checklist as JSON (shared with the MCP tool).
pub(crate) fn verify_report(c: &Client, project: &str) -> Result<Value, String> {
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
    let checks = json!([
        { "check": "events arrive", "ok": events > 0, "detail": events, "fix": "add the SDK snippet and load a page" },
        { "check": "users are identified", "ok": identified > 0, "detail": identified, "fix": "call openlens.identify(user.id, { role }) after login" },
        { "check": "roles are sent", "ok": with_role > 0, "detail": with_role, "fix": "pass the user's real role field to identify()" },
        { "check": "release is tagged", "ok": releases > 0, "detail": releases, "fix": "pass release (version or git SHA) to init or data-release" },
        { "check": "feature map pushed", "ok": map_status == 200, "detail": map["version"], "fix": "openlens-cli features push" },
    ]);
    let ok = checks
        .as_array()
        .is_some_and(|a| a.iter().take(3).all(|c| c["ok"] == true))
        && map_status == 200;
    Ok(json!({ "ok": ok, "checks": checks, "features": features,
        "robots_last_day": bots,
        "note": "features with 0 events have not been exercised yet; click through them once to confirm they fire" }))
}

/// The rule an app's own agents follow to keep the feature map current.
pub(super) const RULES_SNIPPET: &str = "## Product analytics (OpenLens)

- The app's features, success events and roles are declared in `openlens.features.yaml`.
- When you add, rename or remove a user-facing feature, update that file in the same change:
  its `entry` (routes, `data-feature` attribute) and its `success` event, and add
  `data-feature=\"<key>\"` to the feature's main control.
- When you add or rename a user role, map its raw value under `roles:`.
- Run `openlens-cli features lint --src .` before committing, then `openlens-cli features push` on deploy.
- Never send names, emails or content to OpenLens: ids, roles, plans and event names only.";

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
