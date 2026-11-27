//! The product and instance screens (#551): the map health findings, the
//! flags' rollout bars, the forms' fields and the Storage page's paths.

#![allow(clippy::unwrap_used)]
use crate::*;
use std::collections::BTreeSet;

fn p() -> ProjectInfo<'static> {
    ProjectInfo {
        id: "d",
        name: "D",
        key: "k",
    }
}

/// No inline style or script (the CSP).
fn csp_clean(html: &str) {
    assert!(!html.contains(" style=") && !html.contains("<script"));
}

/// The field names a form posts: the `name=` of every control between
/// the form's opening tag (found by `open`) and its end.
fn names(html: &str, open: &str) -> BTreeSet<String> {
    let at = html.find(open).unwrap();
    let form = &html[at..at + html[at..].find("</form>").unwrap()];
    form.split(" name=\"")
        .skip(1)
        .map(|r| r[..r.find('"').unwrap()].to_owned())
        .collect()
}

fn set(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

fn finding(message: &str, fix: Option<&str>) -> openlens_engine::maphealth::Finding {
    openlens_engine::maphealth::Finding {
        kind: "dead",
        level: "warning",
        feature: Some("export".into()),
        message: message.into(),
        evidence: "0 uses in 30 days".into(),
        fix: fix.map(Into::into),
        impact: 1,
    }
}

#[test]
fn map_health_findings_keep_their_order_and_their_suggested_change() {
    let h = openlens_engine::MapHealth {
        score: 62,
        findings: vec![
            finding("Zeta is never used", Some("- key: zeta")),
            finding("Alpha overlaps billing", None),
            finding("Mid has no success", Some("+ success: mid_done")),
        ],
        ..openlens_engine::MapHealth::default()
    };
    let html = map_health("d", &h).into_string();
    csp_clean(&html);
    // In the order given (the engine ranks them), not sorted.
    let at = |s: &str| html.find(s).unwrap();
    let (z, a, m) = (
        at("Zeta is never used"),
        at("Alpha overlaps billing"),
        at("Mid has no success"),
    );
    assert!(z < a && a < m, "findings reordered");
    // Each suggested change sits folded in a details, inside its own finding.
    assert_eq!(html.matches("<details").count(), 2);
    let fix_z = at("- key: zeta");
    let fix_m = at("+ success: mid_done");
    assert!(z < fix_z && fix_z < a && m < fix_m);
    for fix in [fix_z, fix_m] {
        let open = html[..fix].rfind("<details").unwrap();
        assert!(!html[open..fix].contains("</details>"));
        assert!(html[open..fix].contains("Suggested change"));
    }
    // Every finding keeps its ticket.
    assert_eq!(
        html.matches("/tickets/new?kind=map&amp;subject=dead%3Aexport")
            .count(),
        3
    );
}

fn flag(key: &str, rollout: u8) -> openlens_flags::Flag {
    openlens_flags::Flag {
        key: key.into(),
        description: String::new(),
        enabled: true,
        rollout,
        roles: Vec::new(),
        accounts: Vec::new(),
        variants: Vec::new(),
        rules: Vec::new(),
        users: Vec::new(),
        holdout: 0,
    }
}

#[test]
fn a_flag_with_a_higher_rollout_has_a_wider_bar() {
    let html = flags(&p(), &[flag("narrow", 10), flag("broad", 80)]).into_string();
    csp_clean(&html);
    let width = |key: &str| -> f64 {
        let row = html
            .split("<tr>")
            .find(|r| r.contains(&format!("flags-edit?key={key}\"")))
            .unwrap();
        let w = row.split("class=\"fg\" width=\"").nth(1).unwrap();
        w[..w.find('"').unwrap()].parse().unwrap()
    };
    assert!(width("broad") > width("narrow"));
    // Each row keeps its two forms on the flag's endpoint.
    assert_eq!(html.matches("action=\"/p/d/flags/broad\"").count(), 2);
}

#[test]
fn the_survey_editor_posts_the_same_fields_with_one_answer_type_checked() {
    let html = survey_edit(&p(), None, None).into_string();
    csp_clean(&html);
    assert_eq!(
        names(&html, "action=\"/p/d/surveys-edit\""),
        set(&[
            "original", "key", "question", "kind", "choices", "trigger", "sample", "roles",
            "enabled"
        ])
    );
    let radios: Vec<&str> = html
        .split("<input ")
        .skip(1)
        .filter(|t| t.contains("name=\"kind\""))
        .collect();
    assert_eq!(radios.len(), 4);
    assert!(radios.iter().all(|t| t.contains("type=\"radio\"")));
    assert_eq!(
        radios
            .iter()
            .filter(|t| t[..t.find('>').unwrap()].contains(" checked"))
            .count(),
        1
    );
}

#[test]
fn the_instance_forms_post_the_same_fields() {
    let team = users(&[], &["d".to_owned()], None, "", None).into_string();
    csp_clean(&team);
    assert_eq!(
        names(&team, "method=\"post\" action=\"/users\""),
        set(&["email", "name", "role", "projects"])
    );
    let tokens = tokens(&TokensView {
        tokens: Vec::new(),
        projects: vec![("d".into(), "D".into())],
        created: None,
        notice: None,
        error: None,
        confirm: None,
    })
    .into_string();
    csp_clean(&tokens);
    assert_eq!(
        names(&tokens, "action=\"/settings/tokens\""),
        set(&["label", "scope", "project"])
    );
    let storage = storage(&storage_view("/srv/openlens/data")).into_string();
    assert_eq!(
        names(&storage, "action=\"/settings/storage/s3\""),
        set(&[
            "bucket",
            "region",
            "endpoint",
            "prefix",
            "access_key",
            "secret",
            "action"
        ])
    );
}

fn storage_view(data_dir: &str) -> StorageView {
    StorageView {
        from_env: false,
        bucket: Some("my-bucket".into()),
        region: None,
        endpoint: None,
        prefix: None,
        access_key: Some("AKIA".into()),
        secret_set: true,
        secret_ok: true,
        last_ok: None,
        last_objects: None,
        last_error: None,
        plan: BackupPlan {
            schedule: "daily".into(),
            hour: 3,
            weekday: 1,
            keep: 7,
            dir: String::new(),
            folder: format!("{data_dir}/backups"),
            to_s3: false,
        },
        history: vec![BackupRun {
            at: "2026-11-26T03:00:00Z".into(),
            ok: true,
            files: 12,
            bytes: 4096,
            secs: 1.5,
            dest: format!("{data_dir}/backups/2026-11-26T0300"),
            s3: None,
            error: None,
            how: "schedule".into(),
        }],
        running: false,
        has_latest: true,
        data_dir: data_dir.into(),
        cold_on: false,
        cold_from_env: false,
        cold_blocked: None,
        cold_days: 0,
        notice: None,
        error: None,
    }
}

#[test]
fn the_storage_page_never_prints_the_full_data_path() {
    let dir = "/home/alice/openlens-data";
    let html = storage(&storage_view(dir)).into_string();
    csp_clean(&html);
    assert!(
        !html.contains(dir) && !html.contains("/home/alice"),
        "{html}"
    );
    // The directory is still named, by its last component.
    assert!(html.contains("openlens-data"));
    // A path elsewhere shows by its last component only.
    assert_eq!(
        crate::settings::shown_path("/home/alice/backups", dir),
        "…/backups"
    );
}

#[test]
fn a_message_page_is_one_empty_state_card() {
    let html = message("Not available", "Ask is turned off.").into_string();
    csp_clean(&html);
    assert_eq!(html.matches("class=\"card empty message\"").count(), 1);
    assert!(html.contains("Ask is turned off.") && html.contains("href=\"/\""));
}

#[test]
fn the_feature_map_holds_who_uses_what_as_a_table() {
    // #620: the full roles × features matrix, gone from the board with
    // #618, is on the Feature map: one row per feature of the map, one
    // column per role, the share printed in every cell and shaded by a
    // class (no style), the raw roles no role matches below it.
    let yaml = "version: 1\nroles:\n  - key: admin\n    match: [\"admin\"]\n  - key: member\n    match: [\"member\"]\nfeatures:\n  - key: reports\n    name: Reports\n    entry:\n      routes: [\"/reports\"]\n  - key: export\n    name: Export\n    entry:\n      routes: [\"/export\"]\n";
    let m = openlens_featuremap::parse(yaml).map.unwrap();
    let role = |r: &str, users| openlens_engine::RoleUsers {
        role: Some(r.into()),
        users,
    };
    let x = openlens_engine::RoleMatrix {
        roles: vec![role("member", 100), role("admin", 10)],
        cells: vec![
            ("member".into(), "reports".into(), 90),
            ("admin".into(), "reports".into(), 1),
        ],
        gaps: vec![("Admin ".into(), 3)],
    };
    let html = crate::features::role_matrix("d", &m, &x).into_string();
    csp_clean(&html);
    let table = &html[html.find("<table class=\"heat\">").unwrap()..];
    assert!(table.contains("<caption"), "{table}");
    let head = &table[..table.find("</thead>").unwrap()];
    assert!(head.find(">member<") < head.find(">admin<"), "{head}");
    assert!(
        head.contains("100 users") && head.contains("10 users"),
        "{head}"
    );
    // Rows in the map's order, each a link to its page.
    let row = |k: &str| table.find(&format!("href=\"/p/d/features/{k}\"")).unwrap();
    assert!(row("reports") < row("export"));
    // More of a role's users, a darker cell; nobody, a dash.
    let reports = &table[row("reports")..row("export")];
    let shade = |s: &str| -> u8 {
        let at = reports.find(&format!(">{s}<")).unwrap();
        let cls = &reports[..at];
        let p = &cls[cls.rfind("hc p").unwrap() + 4..];
        p[..p.find('"').unwrap()].parse().unwrap()
    };
    assert!(shade("90.0%") > shade("10.0%"), "{reports}");
    let export = &table[row("export")..];
    assert_eq!(export.matches(">—<").count(), 2, "{export}");
    assert!(html.contains("<code>Admin </code> 3 users"), "{html}");
    // No role at all: the card says where roles come from.
    let none = crate::features::role_matrix("d", &m, &openlens_engine::RoleMatrix::default())
        .into_string();
    assert!(none.contains("No identified user") && !none.contains("<table"));
}
