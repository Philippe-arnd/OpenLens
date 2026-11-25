//! The map's rules in Rust against the reference SQL (#446).

use super::*;

/// #446: the map's rules applied in Rust give, row by row, what the
/// reference `CASE` expressions give, for several map versions.
#[tokio::test]
async fn map_lookups_equal_the_reference_case() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let paths = [
        Some("/"),
        Some("/docs/42"),
        Some("/docs/42/"),
        Some("/docs"),
        Some("/docs/42/edit"),
        Some("/admin"),
        Some("/admin/users/7"),
        Some("/reports.csv"),
        Some("/reports_csv"),
        Some("/é/1"),
        Some(""),
        None,
    ];
    let roles = [
        Some("Admin"),
        Some(" admin "),
        Some("OWNER"),
        Some("billing-admin"),
        Some("Billing-Team"),
        Some("billing"),
        Some("member"),
        Some("team_a"),
        Some("teamXa"),
        Some("guest"),
        Some("Ünïcode"),
        Some(""),
        None,
    ];
    let features = [None, Some("export"), Some("old-search"), Some("")];
    let events = ["$pageview", "$click", "doc_saved", "report_sent"];
    let mut rows = Vec::new();
    let mut n = 0;
    for (pi, path) in paths.iter().enumerate() {
        for (ri, role) in roles.iter().enumerate() {
            let feature = features[(pi + ri) % features.len()];
            let event = events[(pi * 3 + ri) % events.len()];
            n += 1;
            let ts = Utc::now() - Duration::minutes(n);
            rows.push(Event {
                uuid: format!("u{n}"),
                project: "p".into(),
                event: event.into(),
                distinct_id: format!("d{n}"),
                session_id: None,
                timestamp: ts,
                received_at: ts,
                url: None,
                path: path.map(Into::into),
                feature: feature.map(Into::into),
                role: role.map(Into::into),
                account: None,
                plan: None,
                environment: "production".into(),
                release: None,
                flags: None,
                properties: "{}".into(),
            });
        }
    }
    w.append(&rows).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());

    let maps = [
        // Overlapping routes (first wins), `*`, a rename and success events.
        "version: 1\nroles:\n  - key: admin\n    match: [Admin, owner, 'billing-*']\n  \
         - key: member\n    match: [member, 'team_*', '*a*z']\nfeatures:\n  \
         - key: docs\n    name: Docs\n    purpose: Docs.\n    entry:\n      routes: ['/docs/:id', '/docs/*']\n    success: doc_saved\n  \
         - key: search\n    name: Search\n    purpose: Search.\n    renamed_from: [old-search]\n    entry:\n      routes: ['/docs/:id/edit', /reports.csv]\n  \
         - key: admin-area\n    name: Admin\n    purpose: Admin.\n    entry:\n      routes: ['/admin/*', /]\n    success: report_sent\n",
        // A later version: fewer rules, different order.
        "version: 1\nroles:\n  - key: staff\n    match: ['*']\nfeatures:\n  \
         - key: admin-area\n    name: Admin\n    purpose: Admin.\n    entry:\n      routes: ['/admin/users/:id']\n",
        // No roles: the raw value is kept.
        "version: 1\nfeatures:\n  - key: docs\n    name: Docs\n    purpose: Docs.\n    entry:\n      routes: ['/docs/:id']\n",
    ];
    for yaml in maps {
        let m = parse(yaml).map.expect("valid map");
        let src = openlens_engine::source(Some(&m));
        assert!(src.contains("feature_of_"), "{src}");
        let reference = format!(
            "SELECT uuid, {} AS role, {} AS feature FROM events ORDER BY uuid",
            m.role_sql(),
            m.feature_sql()
        );
        let lookups = format!("SELECT uuid, role, feature FROM {src} ORDER BY uuid");
        let a = q.sql("p", &reference).await.unwrap().rows;
        let b = q.sql("p", &lookups).await.unwrap().rows;
        assert_eq!(a.len(), rows.len());
        assert_eq!(a, b, "map:\n{yaml}");
    }
}
