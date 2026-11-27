//! The agent kit for releases (#524, D-069): the steps `init` prints to
//! keep a new project running, the release layers `verify` reports, and the
//! MCP tools `suggested_features`, `set_feature` and `stale_flags`.
//!
//! Everything goes through the HTTP API (the [`Api`] trait), like the rest
//! of the CLI.

use openlens_featuremap::{Feature, FeatureMap, valid_key};
use openlens_mcp::ToolSpec;
use serde_json::{Value, json};

use crate::{Api, Body, api_error, query_value};

/// Share of the production page views that must carry a flag on for
/// everyone: a first visit is sent before the SDK knows its flags.
const EVERYONE_SHARE: f64 = 0.95;
/// Fewest production page views to judge a flag on for everyone.
const MIN_VIEWS: i64 = 100;
/// Days a flag must have been settled (off, or on for everyone).
const SETTLED_DAYS: i64 = 14;
/// Days read to tell a flag that was used from one never used.
const SEEN_DAYS: i64 = 90;
/// Flags judged per SQL query.
const FLAGS_PER_QUERY: usize = 25;

fn ok_or(status: u16, v: Value) -> Result<Value, String> {
    if (200..300).contains(&status) {
        Ok(v)
    } else {
        Err(api_error(status, &v))
    }
}

fn first_int(rows: &[Value], col: &str) -> i64 {
    rows.first().and_then(|r| r[col].as_i64()).unwrap_or(0)
}

/// The steps that keep a project measured release after release, added to
/// `init`'s output: the deploy hook URL (made now, shown once), the release
/// step, the rules and the check. A hook that cannot be made says how to
/// make it later instead.
pub(crate) fn keep_it_running<A: Api + ?Sized>(c: &A, created: &Value) -> Value {
    let id = created["id"].as_str().unwrap_or_default();
    let url = if valid_key(id) {
        c.call("POST", &format!("/api/projects/{id}/deploy-hook"), None)
            .ok()
            .filter(|(status, v)| *status == 200 && v["url"].is_string())
            .map(|(_, v)| v["url"].clone())
    } else {
        None
    };
    let hook = match url {
        Some(url) => json!({
            "step": "deploy hook",
            "url": url,
            "do": "Shown only now. Store this URL as a secret of the platform or the CI, never in the repository, and paste it where the platform sends deploy notifications (/agents.md, step 5c). The MCP tool deploy_hook makes a new one; the old one then stops working.",
        }),
        None => json!({
            "step": "deploy hook",
            "url": null,
            "do": format!("Not made: make it with the MCP tool deploy_hook (or POST /api/projects/{id}/deploy-hook), then paste it where the platform sends deploy notifications (/agents.md, step 5c)."),
        }),
    };
    json!([
        hook,
        {
            "step": "release step",
            "do": "Add `openlens-cli release` (or the `openlens-release` GitHub Action) to the deploy step, after the build, with the release name the SDK sends.",
        },
        {
            "step": "rules",
            "do": "Add the block printed by `openlens-cli rules` to the app's AGENTS.md (or CLAUDE.md): its agents then keep the feature map current, read `release_report` after each deploy and remove the flags `stale_flags` reports.",
        },
        {
            "step": "check",
            "do": format!("Run `openlens-cli verify --project {id}` after the first deploy: `release_layers` says which layers are active and how to turn on the others."),
        },
    ])
}

/// Which release layers are active, as a checklist: a build fingerprint
/// seen (7 days), the deploy hook made, deploys recorded and releases
/// judged (90 days). Reported by `verify`, never required for its `ok`.
pub(crate) fn release_layers<A: Api + ?Sized>(c: &A, project: &str) -> Result<Value, String> {
    let builds = first_int(
        &c.sql(
            project,
            "SELECT count(*) AS n FROM events WHERE timestamp >= now() - INTERVAL '7 days' \
             AND strpos(properties, '\"$build\"') > 0",
        )?,
        "n",
    );
    let (status, r) = c.call("GET", &format!("/api/projects/{project}/releases"), None)?;
    let unreadable = (status != 200).then(|| api_error(status, &r));
    let releases = r["releases"].as_array().cloned().unwrap_or_default();
    let deploys = r["deploys"].as_array().map_or(0, Vec::len);
    let hook = r["deploy_hook"].as_str();
    let judged = releases
        .iter()
        .filter(|x| x.get("verdict").is_some_and(|v| !v.is_null()))
        .count();
    let or_error = |ok: Value| unreadable.as_ref().map_or(ok, |e| json!(e));
    let verdicts = or_error(if judged > 0 {
        json!(judged)
    } else if releases.is_empty() {
        json!("no release in the last 90 days")
    } else if releases.iter().all(|x| x.get("verdict").is_none()) {
        json!("this server does not judge releases yet")
    } else {
        json!("no release judged yet")
    });
    Ok(json!([
        {
            "layer": "build fingerprint seen",
            "active": builds > 0,
            "detail": builds,
            "fix": "use the browser SDK at its current version (script tag or @openlens-obs/web): it sends a fingerprint of the page's own scripts with each event",
        },
        {
            "layer": "deploy hook configured",
            "active": hook.is_some(),
            "detail": or_error(json!(hook)),
            "fix": "make the URL with the MCP tool deploy_hook and paste it in the platform's deploy notifications (/agents.md, step 5c)",
        },
        {
            "layer": "releases recorded",
            "active": deploys > 0,
            "detail": or_error(json!(deploys)),
            "fix": "run `openlens-cli release` (or the `openlens-release` GitHub Action) in the deploy step, or let the deploy hook report each deploy",
        },
        {
            "layer": "verdicts present",
            "active": judged > 0,
            "detail": verdicts,
            "fix": "keep releases recorded: each release is judged once enough users have used it; read it with `release_report` through MCP",
        },
    ]))
}

/// The MCP tools of the agent kit.
pub(crate) fn specs(proj: &Value) -> Vec<ToolSpec> {
    let with = |extra: Value| {
        let mut p = proj.clone();
        if let (Some(a), Some(b)) = (p.as_object_mut(), extra.as_object()) {
            a.extend(b.clone());
        }
        p
    };
    vec![
        ToolSpec {
            name: "suggested_features",
            description: "Pages and data-feature values used in production that no feature claims (30 days, from map_health's coverage, drift and undeclared findings), each as a proposed feature map entry ready for set_feature, with its evidence. Check each against the code and add its success event first. Nothing is applied.",
            input_schema: json!({"type":"object","properties":with(json!({"limit":{"type":"integer","minimum":1,"maximum":50}}))}),
        },
        ToolSpec {
            name: "set_feature",
            description: "Add or update one feature map entry (matched by key) and push the map as a new version. `feature` is the entry as in openlens.features.yaml: key, name, and optionally purpose, owner, entry {routes, selectors, calls}, steps, success, area, requires, renamed_from, server, paths. `paths` are the feature's code paths, globs from the repository root (`src/export/**`; `*`, `**`, `?`, at most 20 of 200 characters): keep them current, since a release's changed files are matched against them to say which features it touched. Returns the whole map as YAML: write it to openlens.features.yaml and commit it, since the file stays the source of truth (comments are not kept).",
            input_schema: json!({"type":"object","properties":with(json!({"feature":{"type":"object","properties":{"key":{"type":"string"},"name":{"type":"string"},"paths":{"type":"array","maxItems":20,"items":{"type":"string","maxLength":200},"description":"Code path globs from the repository root: src/export/**"}},"required":["key","name"]},"message":{"type":"string","description":"Why (shown in the map's history)"}})),"required":["feature"]}),
        },
        ToolSpec {
            name: "stale_flags",
            description: "Flags ready to remove from the code and from OpenLens: off, and seen on production page views in the last 90 days but none in the last 14 (remove the flag and its code); or on for everyone with no targeting, carried by 95% or more of at least 100 production page views of the last 14 days (remove the flag and keep its code path). Flags never seen in 90 days and flags not yet settled are listed apart.",
            input_schema: json!({"type":"object","properties":proj}),
        },
    ]
}

/// Run an agent kit tool.
pub(crate) fn tool<A: Api + ?Sized>(c: &A, name: &str, args: &Value) -> Result<Value, String> {
    let project = args
        .get("project")
        .and_then(Value::as_str)
        .unwrap_or("default");
    if !valid_key(project) {
        return Err(format!("`{project}` is not a project id"));
    }
    match name {
        "suggested_features" => {
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(10);
            suggested_features(
                c,
                project,
                usize::try_from(limit.clamp(1, 50)).unwrap_or(10),
            )
        }
        "set_feature" => set_feature(c, project, args),
        "stale_flags" => stale_flags(c, project),
        _ => Err(format!("unknown tool {name}")),
    }
}

fn suggested_features<A: Api + ?Sized>(
    c: &A,
    project: &str,
    limit: usize,
) -> Result<Value, String> {
    let (status, v) = c.call(
        "GET",
        &format!("/api/projects/{project}/feature-map/health?env=production"),
        None,
    )?;
    let v = ok_or(status, v)?;
    let picks: Vec<Value> = v["findings"]
        .as_array()
        .map(|a| a.iter().filter_map(proposal).take(limit).collect())
        .unwrap_or_default();
    Ok(json!({
        "suggestions": picks,
        "apply": "check each against the code, add its success event, then set_feature { feature } (or edit openlens.features.yaml and feature_map_push)",
    }))
}

/// A map health finding as a proposed entry: a page no feature claims
/// (`coverage`, `drift`) or a `data-feature` value the map lacks
/// (`undeclared`). Other findings are not new features.
pub(crate) fn proposal(f: &Value) -> Option<Value> {
    let kind = f["kind"].as_str()?;
    let feature = match kind {
        "coverage" | "drift" => entry_of_fix(f["fix"].as_str()?)?,
        "undeclared" => {
            let key = f["feature"].as_str().filter(|k| valid_key(k))?;
            json!({ "key": key, "name": key.replace(['-', '_'], " ") })
        }
        _ => return None,
    };
    Some(json!({
        "kind": kind,
        "why": f["message"],
        "evidence": f["evidence"],
        "impact": f["impact"],
        "feature": feature,
    }))
}

/// The entry a coverage or drift fix declares (`- key:`, `name:`,
/// `routes: ["…"]`), as JSON for `set_feature`.
fn entry_of_fix(fix: &str) -> Option<Value> {
    let (mut key, mut name, mut route) = (None, None, None);
    for line in fix.lines().map(str::trim) {
        if let Some(k) = line.strip_prefix("- key:") {
            key = Some(k.trim().to_owned());
        } else if let Some(n) = line.strip_prefix("name:") {
            name = Some(n.trim().to_owned());
        } else if let Some(r) = line.strip_prefix("routes:") {
            let r = r.trim().trim_start_matches('[').trim_end_matches(']');
            route = Some(r.trim().trim_matches('"').to_owned());
        }
    }
    let key = key.filter(|k| valid_key(k))?;
    let route = route.filter(|r| r.starts_with('/'))?;
    let name = name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| key.replace('-', " "));
    Some(json!({ "key": key, "name": name, "entry": { "routes": [route] } }))
}

fn set_feature<A: Api + ?Sized>(c: &A, project: &str, args: &Value) -> Result<Value, String> {
    let raw = args.get("feature").cloned().ok_or("feature is required")?;
    let feature: Feature =
        serde_json::from_value(raw).map_err(|e| format!("not a feature map entry: {e}"))?;
    if !valid_key(&feature.key) {
        return Err(format!("`{}` is not a feature key", feature.key));
    }
    let (status, v) = c.call("GET", &format!("/api/projects/{project}/feature-map"), None)?;
    let mut map = match status {
        200 => v["yaml"]
            .as_str()
            .and_then(|y| openlens_featuremap::parse(y).map)
            .ok_or(
                "the stored map does not read back: push a valid openlens.features.yaml first",
            )?,
        404 => FeatureMap {
            version: 1,
            ..FeatureMap::default()
        },
        _ => return Err(api_error(status, &v)),
    };
    let action = match map.features.iter_mut().find(|f| f.key == feature.key) {
        Some(f) => {
            *f = feature.clone();
            "updated"
        }
        None => {
            map.features.push(feature.clone());
            "added"
        }
    };
    let yaml = openlens_featuremap::to_yaml(&map)?;
    let message = args
        .get("message")
        .and_then(Value::as_str)
        .map_or_else(|| format!("set_feature {}", feature.key), str::to_owned);
    let (status, pushed) = c.call(
        "PUT",
        &format!(
            "/api/projects/{project}/feature-map?message={}",
            query_value(&message)
        ),
        Some(Body::Text(yaml.clone())),
    )?;
    let pushed = ok_or(status, pushed)?;
    Ok(json!({
        "action": action,
        "key": feature.key,
        "pushed": pushed,
        "yaml": yaml,
        "next": "write `yaml` to openlens.features.yaml and commit it: a push of the older file would undo this change",
    }))
}

/// A flag that may be ready to remove: off, or on for everyone with no
/// targeting. Rollouts, targeted flags and experiments are not settled.
fn settled(f: &Value) -> Option<&'static str> {
    if f["enabled"] == false {
        return Some("off");
    }
    let empty = |k: &str| {
        f.get(k)
            .is_none_or(|v| v.is_null() || v.as_array().is_some_and(Vec::is_empty))
    };
    let plain = ["roles", "accounts", "rules", "users", "variants"]
        .iter()
        .all(|k| empty(k))
        && f["holdout"].as_u64().unwrap_or(0) == 0;
    (f["enabled"] == true && f["rollout"].as_u64() == Some(100) && plain)
        .then_some("on_for_everyone")
}

/// Production page views of the last [`SEEN_DAYS`] days: `n` in the last
/// [`SETTLED_DAYS`], and per flag `a<i>` carrying it then and `b<i>`
/// carrying it before. Keys are checked by the caller ([`valid_key`]), so
/// they are safe inside the literals.
fn flags_sql(keys: &[&str]) -> String {
    let since = (chrono::Utc::now() - chrono::Duration::days(SEEN_DAYS)).format("%Y-%m-%d");
    let mut cols = vec!["sum(CASE WHEN recent THEN 1 ELSE 0 END) AS n".to_owned()];
    for (i, k) in keys.iter().enumerate() {
        let has = format!("strpos(concat(',', coalesce(flags, ''), ','), ',{k},') > 0");
        cols.push(format!(
            "sum(CASE WHEN recent AND {has} THEN 1 ELSE 0 END) AS a{i}"
        ));
        cols.push(format!(
            "sum(CASE WHEN NOT recent AND {has} THEN 1 ELSE 0 END) AS b{i}"
        ));
    }
    format!(
        "SELECT {} FROM (SELECT flags, timestamp >= now() - INTERVAL '{SETTLED_DAYS} days' AS recent \
         FROM events WHERE date >= '{since}' AND environment = 'production' AND event = '$pageview') e",
        cols.join(", ")
    )
}

fn stale_flags<A: Api + ?Sized>(c: &A, project: &str) -> Result<Value, String> {
    let (status, v) = c.call("GET", &format!("/api/projects/{project}/flags"), None)?;
    let v = ok_or(status, v)?;
    let flags = v["flags"].as_array().cloned().unwrap_or_default();
    let mut candidates = Vec::new();
    let mut unsettled = Vec::new();
    for f in &flags {
        let Some(key) = f["key"].as_str().filter(|k| valid_key(k)) else {
            continue;
        };
        match settled(f) {
            Some(state) => candidates.push((key, state)),
            None => unsettled.push(key),
        }
    }
    let mut seen = Vec::new();
    let mut views = 0;
    for chunk in candidates.chunks(FLAGS_PER_QUERY) {
        let keys: Vec<&str> = chunk.iter().map(|(k, _)| *k).collect();
        let rows = c.sql(project, &flags_sql(&keys))?;
        views = first_int(&rows, "n");
        for i in 0..chunk.len() {
            seen.push((
                first_int(&rows, &format!("a{i}")),
                first_int(&rows, &format!("b{i}")),
            ));
        }
    }
    Ok(judge(&candidates, &seen, views, &unsettled))
}

/// Which settled flags are ready to remove (pure): `seen` holds, per
/// candidate, the production page views carrying it in the last
/// [`SETTLED_DAYS`] and in the days before; `views` all of those of the
/// last [`SETTLED_DAYS`].
pub(crate) fn judge(
    candidates: &[(&str, &str)],
    seen: &[(i64, i64)],
    views: i64,
    unsettled: &[&str],
) -> Value {
    let (mut ready, mut never, mut not_yet) = (Vec::new(), Vec::new(), Vec::new());
    for (&(key, state), &(recent, before)) in candidates.iter().zip(seen) {
        if state == "off" {
            if recent == 0 && before == 0 {
                never.push(json!({ "key": key, "state": state,
                    "why": format!("off and never seen on a production page view in {SEEN_DAYS} days: remove it unless a change is about to use it") }));
            } else if recent == 0 {
                ready.push(json!({ "key": key, "state": state,
                    "evidence": format!("off; {before} production page views carried it before, none in the last {SETTLED_DAYS} days"),
                    "do": "delete the flag and the code behind isEnabled (nobody gets it)" }));
            } else {
                not_yet.push(json!({ "key": key, "state": state,
                    "why": format!("off, but {recent} production page views carried it in the last {SETTLED_DAYS} days") }));
            }
        } else if views >= MIN_VIEWS && recent as f64 >= EVERYONE_SHARE * views as f64 {
            ready.push(json!({ "key": key, "state": state,
                "evidence": format!("on for everyone; {recent} of {views} production page views carried it in the last {SETTLED_DAYS} days"),
                "do": "delete the flag and its isEnabled check, keeping the code path users get now" }));
        } else {
            not_yet.push(json!({ "key": key, "state": state,
                "why": format!("on for everyone, but {recent} of {views} production page views carried it in the last {SETTLED_DAYS} days (needs {:.0}% of at least {MIN_VIEWS})", EVERYONE_SHARE * 100.0) }));
        }
    }
    json!({
        "ready_to_remove": ready,
        "never_seen": never,
        "not_yet": not_yet,
        "in_rollout": unsettled,
        "window_days": SETTLED_DAYS,
    })
}
