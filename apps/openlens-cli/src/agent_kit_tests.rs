//! The agent kit (#524) on a fake API: the verify checklist, init's steps,
//! the rules block and the MCP tools, with no server.

use std::cell::RefCell;

use clap::CommandFactory;
use openlens_mcp::Tools;
use serde_json::{Value, json};

use crate::agent_kit::{judge, keep_it_running, proposal, release_layers, tool};
use crate::mcp::OpenLensTools;
use crate::{Api, Body, Cli, Client, RULES_SNIPPET, verify_report};

/// Canned answers by method and path prefix (the first that matches), SQL
/// answered from its text, and every call kept.
struct Fake {
    routes: Vec<(&'static str, &'static str, u16, Value)>,
    sql: fn(&str) -> Vec<Value>,
    calls: RefCell<Vec<(String, String, Option<String>)>>,
}

impl Fake {
    fn new(
        routes: Vec<(&'static str, &'static str, u16, Value)>,
        sql: fn(&str) -> Vec<Value>,
    ) -> Self {
        Self {
            routes,
            sql,
            calls: RefCell::new(Vec::new()),
        }
    }

    fn sent(&self, method: &str, prefix: &str) -> Option<(String, Option<String>)> {
        self.calls
            .borrow()
            .iter()
            .find(|(m, p, _)| m == method && p.starts_with(prefix))
            .map(|(_, p, b)| (p.clone(), b.clone()))
    }
}

impl Api for Fake {
    fn call(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Value), String> {
        let text = match &body {
            Some(Body::Json(v)) => Some(v.to_string()),
            Some(Body::Text(t) | Body::Raw(t)) => Some(t.clone()),
            None => None,
        };
        self.calls
            .borrow_mut()
            .push((method.to_owned(), path.to_owned(), text));
        if path.ends_with("/sql") {
            let q = match &body {
                Some(Body::Json(v)) => v["sql"].as_str().unwrap_or_default().to_owned(),
                _ => String::new(),
            };
            return Ok((200, json!({ "rows": (self.sql)(&q) })));
        }
        for (m, p, status, v) in &self.routes {
            if *m == method && path.starts_with(p) {
                return Ok((*status, v.clone()));
            }
        }
        Ok((
            404,
            json!({ "code": "not_found", "message": "unknown project" }),
        ))
    }
}

fn n(v: i64) -> Vec<Value> {
    vec![json!({ "n": v })]
}

/// An app wired with every release layer.
fn wired() -> Fake {
    Fake::new(
        vec![
            (
                "GET",
                "/api/projects/app/feature-map",
                200,
                json!({ "version": 3, "yaml": "features:\n  - key: write-entry\n    name: Write\n" }),
            ),
            (
                "GET",
                "/api/projects/app/environments",
                200,
                json!({ "unclassified": [] }),
            ),
            (
                "GET",
                "/api/projects/app/releases",
                200,
                json!({
                    "releases": [
                        { "release": "v2", "environment": "production", "verdict": "healthy" },
                        { "release": "v1", "environment": "production", "verdict": null },
                    ],
                    "deploys": [{ "status": "succeeded" }, { "status": "started" }],
                    "deploy_hook": "2026-11-20 10:00",
                }),
            ),
        ],
        |q| {
            if q.contains("$build") {
                n(12)
            } else if q.contains("'bot'") {
                n(0)
            } else if q.contains("GROUP BY") {
                Vec::new()
            } else {
                n(5)
            }
        },
    )
}

/// The checklist without the fixes, which are text.
fn layers_only(v: &Value) -> Value {
    json!(
        v.as_array()
            .unwrap()
            .iter()
            .map(|l| json!({ "layer": l["layer"], "active": l["active"], "detail": l["detail"] }))
            .collect::<Vec<_>>()
    )
}

#[test]
fn verify_lists_every_release_layer_as_active_on_a_wired_app() {
    let report = verify_report(&wired(), "app").unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(
        layers_only(&report["release_layers"]),
        json!([
            { "layer": "build fingerprint seen", "active": true, "detail": 12 },
            { "layer": "deploy hook configured", "active": true, "detail": "2026-11-20 10:00" },
            { "layer": "releases recorded", "active": true, "detail": 2 },
            { "layer": "verdicts present", "active": true, "detail": 1 },
        ])
    );
}

#[test]
fn verify_says_what_is_missing_without_failing_the_wiring() {
    // A server that predates verdicts: no `verdict` on its releases.
    let older = Fake::new(
        vec![
            (
                "GET",
                "/api/projects/app/feature-map",
                200,
                json!({ "version": 1, "yaml": "features: []\n" }),
            ),
            (
                "GET",
                "/api/projects/app/environments",
                200,
                json!({ "unclassified": [] }),
            ),
            (
                "GET",
                "/api/projects/app/releases",
                200,
                json!({ "releases": [{ "release": "v1" }], "deploys": [], "deploy_hook": null }),
            ),
        ],
        |q| {
            if q.contains("$build") || q.contains("'bot'") {
                n(0)
            } else {
                n(5)
            }
        },
    );
    let report = verify_report(&older, "app").unwrap();
    assert_eq!(report["ok"], true, "release layers are not wiring");
    let layers = &report["release_layers"];
    assert_eq!(
        layers_only(layers),
        json!([
            { "layer": "build fingerprint seen", "active": false, "detail": 0 },
            { "layer": "deploy hook configured", "active": false, "detail": null },
            { "layer": "releases recorded", "active": false, "detail": 0 },
            { "layer": "verdicts present", "active": false, "detail": "this server does not judge releases yet" },
        ])
    );
    for l in layers.as_array().unwrap() {
        assert!(!l["fix"].as_str().unwrap_or_default().is_empty(), "{l}");
    }
}

#[test]
fn release_layers_report_an_unreadable_releases_list_as_inactive() {
    let down = Fake::new(Vec::new(), |_| n(3));
    let layers = release_layers(&down, "app").unwrap();
    let layers = layers.as_array().unwrap();
    assert_eq!(layers[0]["active"], true, "the fingerprint comes from SQL");
    for l in &layers[1..] {
        assert_eq!(l["active"], false);
        assert!(l["detail"].as_str().unwrap().contains("not found"), "{l}");
    }
}

#[test]
fn init_makes_the_deploy_hook_and_lists_the_steps_that_keep_it_running() {
    let url = "https://lens.example.com/hooks/deploy/dh_abc";
    let fake = Fake::new(
        vec![(
            "POST",
            "/api/projects/app/deploy-hook",
            200,
            json!({ "url": url, "note": "Shown once." }),
        )],
        |_| Vec::new(),
    );
    let steps = keep_it_running(&fake, &json!({ "id": "app", "key": "k" }));
    let names: Vec<&str> = steps
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["step"].as_str())
        .collect();
    assert_eq!(names, ["deploy hook", "release step", "rules", "check"]);
    assert_eq!(steps[0]["url"], url);
    assert!(
        steps[1]["do"]
            .as_str()
            .unwrap()
            .contains("openlens-release")
    );
    assert!(
        steps[2]["do"]
            .as_str()
            .unwrap()
            .contains("openlens-cli rules")
    );
    assert!(steps[3]["do"].as_str().unwrap().contains("--project app"));

    // A hook that cannot be made says how to make it, and shows no URL.
    let refused = Fake::new(Vec::new(), |_| Vec::new());
    let steps = keep_it_running(&refused, &json!({ "id": "app" }));
    assert!(steps[0]["url"].is_null());
    assert!(steps[0]["do"].as_str().unwrap().contains("deploy_hook"));
}

#[test]
fn the_rules_name_only_commands_and_tools_that_exist() {
    let cli = Cli::command();
    let words: Vec<&str> = RULES_SNIPPET
        .split("openlens-cli ")
        .skip(1)
        .filter_map(|rest| rest.split([' ', '`', ')']).next())
        .collect();
    assert!(words.len() >= 4, "{words:?}");
    for w in words {
        // `release` is the command of #521, built alongside.
        assert!(
            w == "release" || cli.find_subcommand(w).is_some(),
            "the rules name `openlens-cli {w}`, which is not a command"
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let client = Client::new(Some("http://localhost:1".into()), Some("t".into())).unwrap();
    let tools = OpenLensTools::new(client, dir.path(), dir.path()).unwrap();
    let listed: Vec<&str> = tools.list().iter().map(|t| t.name).collect();
    for t in [
        "suggested_features",
        "set_feature",
        "stale_flags",
        "release_report",
    ] {
        assert!(RULES_SNIPPET.contains(&format!("`{t}`")), "{t}");
        // `release_report` is the tool of #522, built alongside.
        assert!(
            t == "release_report" || listed.contains(&t),
            "{t} is not a tool"
        );
    }
}

#[test]
fn suggested_features_turns_unclaimed_production_usage_into_entries() {
    // Findings as map health writes them (crates/engine/src/maphealth.rs, drift.rs).
    let health = json!({ "score": 70, "findings": [
        { "kind": "coverage", "level": "error", "feature": null,
          "message": "`/billing/:id/invoices` is used but belongs to no feature",
          "evidence": "400 pages and clicks in 30 days (20% of all)",
          "fix": "# Add the route to the feature it belongs to, or declare it:\n- key: billing-invoices\n  name: billing invoices\n  entry:\n    routes: [\"/billing/:id/invoices\"]",
          "impact": 400 },
        { "kind": "overlap", "level": "warning", "feature": "b", "message": "m", "evidence": "e",
          "fix": "- key: a\n  entry:\n    routes: [<narrower routes>]", "impact": 50 },
        { "kind": "drift", "level": "warning", "feature": null,
          "message": "`/reports` is new in release `v9` and no feature claims it",
          "evidence": "30 pages and clicks in `v9` (3% of the release), none before it",
          "fix": "# A new feature of this release, or a new route of an existing one:\n- key: reports\n  name: reports\n  entry:\n    routes: [\"/reports\"]",
          "impact": 30 },
        { "kind": "undeclared", "level": "warning", "feature": "export_pdf",
          "message": "the app tags `export_pdf` (data-feature) but the map has no such feature",
          "evidence": "12 events in 30 days", "fix": "- key: export_pdf", "impact": 12 },
    ]});
    let fake = Fake::new(
        vec![("GET", "/api/projects/app/feature-map/health", 200, health)],
        |_| Vec::new(),
    );
    let v = tool(&fake, "suggested_features", &json!({ "project": "app" })).unwrap();
    let (path, _) = fake
        .sent("GET", "/api/projects/app/feature-map/health")
        .unwrap();
    assert!(path.contains("env=production"), "{path}");
    let picks = v["suggestions"].as_array().unwrap();
    let keys: Vec<&str> = picks
        .iter()
        .filter_map(|p| p["feature"]["key"].as_str())
        .collect();
    assert_eq!(keys, ["billing-invoices", "reports", "export_pdf"]);
    assert_eq!(
        picks[0]["feature"]["entry"]["routes"],
        json!(["/billing/:id/invoices"])
    );
    for p in picks {
        // Each proposal is what set_feature takes.
        serde_json::from_value::<openlens_featuremap::Feature>(p["feature"].clone()).unwrap();
    }
    assert!(
        proposal(
            &json!({ "kind": "coverage", "fix": "- key: ../x\n  entry:\n    routes: [\"/x\"]" })
        )
        .is_none()
    );
}

#[test]
fn set_feature_updates_one_entry_and_pushes_a_map_that_reads_back() {
    let stored = "version: 1\nroles:\n  - key: admin\n    match: [\"owner\"]\nfeatures:\n  - key: write-entry\n    name: Write\n    success: entry_saved\n  - key: insights\n    name: Insights\n";
    let fake = Fake::new(
        vec![
            (
                "GET",
                "/api/projects/app/feature-map",
                200,
                json!({ "version": 4, "yaml": stored }),
            ),
            (
                "PUT",
                "/api/projects/app/feature-map",
                200,
                json!({ "version": 5 }),
            ),
        ],
        |_| Vec::new(),
    );
    let args = json!({ "project": "app", "message": "route moved",
        "feature": { "key": "write-entry", "name": "Write", "entry": { "routes": ["/write"] }, "success": "entry_saved", "paths": ["src/editor/**"] } });
    let v = tool(&fake, "set_feature", &args).unwrap();
    assert_eq!(v["action"], "updated");
    let (path, body) = fake.sent("PUT", "/api/projects/app/feature-map").unwrap();
    assert!(path.contains("message=route%20moved"), "{path}");
    let pushed = openlens_featuremap::parse(&body.unwrap()).map.unwrap();
    assert_eq!(pushed.features.len(), 2, "the other entry is kept");
    assert_eq!(pushed.features[0].entry.routes, ["/write"]);
    assert_eq!(
        pushed.features[0].paths,
        ["src/editor/**"],
        "code paths are carried"
    );
    assert_eq!(pushed.roles.len(), 1);

    // No map yet: the entry starts one.
    let empty = Fake::new(
        vec![(
            "PUT",
            "/api/projects/app/feature-map",
            200,
            json!({ "version": 1 }),
        )],
        |_| Vec::new(),
    );
    let v = tool(&empty, "set_feature", &args).unwrap();
    assert_eq!(v["action"], "added");

    // Not an entry: refused before anything is sent.
    let bad = json!({ "project": "app", "feature": { "key": "x", "name": "X", "colour": "red" } });
    assert!(tool(&empty, "set_feature", &bad).is_err());
    let bad_key = json!({ "project": "app", "feature": { "key": "../x", "name": "X" } });
    assert!(tool(&empty, "set_feature", &bad_key).is_err());
}

#[test]
fn a_flag_is_ready_to_remove_only_once_settled() {
    let v = judge(
        &[
            ("gone", "off"),
            ("draft", "off"),
            ("lingering", "off"),
            ("shipped", "on_for_everyone"),
            ("half", "on_for_everyone"),
        ],
        &[(0, 40), (0, 0), (3, 40), (196, 4), (150, 0)],
        200,
        &["beta"],
    );
    let keys = |k: &str| -> Vec<String> {
        v[k].as_array()
            .unwrap()
            .iter()
            .filter_map(|f| f["key"].as_str().map(str::to_owned))
            .collect()
    };
    assert_eq!(keys("ready_to_remove"), ["gone", "shipped"]);
    assert_eq!(keys("never_seen"), ["draft"]);
    assert_eq!(keys("not_yet"), ["lingering", "half"]);
    assert_eq!(v["in_rollout"], json!(["beta"]));

    // Too few page views to judge a flag on for everyone.
    let small = judge(&[("shipped", "on_for_everyone")], &[(20, 0)], 20, &[]);
    assert_eq!(small["ready_to_remove"], json!([]));
}

#[test]
fn stale_flags_judges_only_settled_flags_from_production_page_views() {
    let fake = Fake::new(
        vec![(
            "GET",
            "/api/projects/app/flags",
            200,
            json!({ "flags": [
                { "key": "old-editor", "enabled": false, "rollout": 0 },
                { "key": "new-nav", "enabled": true, "rollout": 100 },
                { "key": "beta", "enabled": true, "rollout": 100, "roles": ["admin"] },
                { "key": "ramp", "enabled": true, "rollout": 30 },
            ]}),
        )],
        |q| {
            assert!(q.contains(",old-editor,") && q.contains(",new-nav,"), "{q}");
            assert!(q.contains("environment = 'production'") && q.contains("'$pageview'"));
            vec![json!({ "n": 300, "a0": 0, "b0": 25, "a1": 299, "b1": 10 })]
        },
    );
    let v = tool(&fake, "stale_flags", &json!({ "project": "app" })).unwrap();
    let ready: Vec<&str> = v["ready_to_remove"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["key"].as_str())
        .collect();
    assert_eq!(ready, ["old-editor", "new-nav"]);
    assert_eq!(v["in_rollout"], json!(["beta", "ramp"]));
}
