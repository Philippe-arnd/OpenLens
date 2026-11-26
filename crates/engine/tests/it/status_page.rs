//! The public status page (#561): what it shows comes only from public
//! fields, an off or unknown page is not found, slugs are checked and
//! unique across projects.

use openlens_engine::commands::status_page::{
    self, Component, GetPublic, PublicInput, Set, SetInput, check_components, check_slug,
    valid_slug,
};
use openlens_engine::commands::{Caller, Command, Error, Host};

use super::maintenance::TestHost;

const ENVS: &str = r#"{"rules":[],"monitors":[{"env":"staging","url":"https://staging.internal.example:8443","level":"log"}]}"#;

fn comp(env: &str, name: &str, response_time: bool) -> Component {
    Component {
        environment: env.into(),
        name: name.into(),
        response_time,
    }
}

/// A project whose id differs from its name, with a monitored staging,
/// an address, a health path, failed checks with error texts and
/// incidents with causes: everything the page must keep inside.
fn seeded(h: &TestHost) -> String {
    let m = h.meta();
    let p = m.create_project("zq internal 7731", None).unwrap();
    m.rename_project(&p.id, "Acme").unwrap();
    m.set_project_setting(&p.id, "environments", ENVS).unwrap();
    m.set_project_setting(&p.id, "app_url", "https://app.internal.example")
        .unwrap();
    m.set_project_setting(&p.id, "alerts", r#"{"health_path":"/secret-health"}"#)
        .unwrap();
    let now = chrono::Utc::now().timestamp();
    for k in 0..30 {
        let ok = k % 10 != 3;
        let err = (!ok).then_some("connect to 10.1.2.3:8080 refused");
        m.record_check(
            &p.id,
            now - 60 * k,
            ok,
            ok.then_some(200),
            Some(120 + k),
            err,
        )
        .unwrap();
    }
    let probe = openlens_meta::Probe {
        ok: false,
        status: Some(502),
        ms: Some(30),
        error: Some("TLS handshake with 10.9.9.9 failed"),
    };
    m.record_check_in(&p.id, "staging", now - 30, &probe)
        .unwrap();
    let inc = m
        .start_incident(
            &p.id,
            "down",
            now - 7200,
            "HTTP 502 from https://app.internal.example/secret-health",
        )
        .unwrap();
    m.end_incident(inc, now - 6000).unwrap();
    m.start_incident(
        &p.id,
        "env:staging:down",
        now - 300,
        "staging.internal.example: connection refused",
    )
    .unwrap();
    p.id
}

async fn turn_on(h: &TestHost, project: &str, slug: &str) -> Result<status_page::View, Error> {
    let input = SetInput {
        project: project.into(),
        enabled: Some(true),
        slug: Some(slug.into()),
        components: Some(vec![
            comp("production", "Web app", true),
            comp("staging", "Preview", false),
        ]),
    };
    Set::run(h, &Caller::default(), input).await
}

async fn public(h: &TestHost, slug: &str) -> Result<status_page::Public, Error> {
    let input = PublicInput { slug: slug.into() };
    GetPublic::run(h, &Caller::default(), input).await
}

#[tokio::test]
async fn the_page_shows_incidents_and_bars_and_nothing_from_inside() {
    let (h, _) = TestHost::new();
    let id = seeded(&h);
    turn_on(&h, &id, "acme-status").await.unwrap();
    let v = public(&h, "acme-status").await.unwrap();
    assert_eq!(v.name, "Acme");
    assert_eq!(v.components.len(), 2);
    for c in &v.components {
        assert_eq!(c.days.len(), 90, "{}: 90 daily bars", c.name);
    }
    assert!(
        v.incidents
            .iter()
            .any(|i| i.component == "Web app" && i.ended_at.is_some()),
        "the closed production incident is listed: {:?}",
        v.incidents
    );
    assert_eq!(v.components[1].state, "down");
    assert_ne!(v.state, "operational", "a component is down");
    assert!(v.components[0].response_p95_ms.is_some(), "opted in");
    assert!(v.components[1].response_p95_ms.is_none(), "not opted in");
    let json = serde_json::to_string(&v).unwrap();
    for inside in [
        id.as_str(),
        "internal",
        "10.1.2.3",
        "10.9.9.9",
        "HTTP 502",
        "secret-health",
        "refused",
        "staging",
    ] {
        assert!(!json.contains(inside), "{inside:?} leaked: {json}");
    }
}

#[tokio::test]
async fn an_off_or_unknown_page_is_not_found_whatever_the_slug() {
    let (h, _) = TestHost::new();
    let id = seeded(&h);
    turn_on(&h, &id, "acme-status").await.unwrap();
    let off = SetInput {
        project: id.clone(),
        enabled: Some(false),
        ..SetInput::default()
    };
    let view = Set::run(&h, &Caller::default(), off).await.unwrap();
    assert_eq!(
        view.settings.slug.as_deref(),
        Some("acme-status"),
        "kept while off"
    );
    assert_eq!(public(&h, "acme-status").await, Err(Error::NotFound));
    assert_eq!(public(&h, "nothing-here").await, Err(Error::NotFound));
    for s in openlens_testkit::hostile_slugs() {
        assert_eq!(public(&h, &s).await, Err(Error::NotFound), "{s:?}");
    }
}

#[tokio::test]
async fn a_slug_belongs_to_one_project() {
    let (h, first) = TestHost::new();
    let id = seeded(&h);
    turn_on(&h, &id, "acme-status").await.unwrap();
    let take = || SetInput {
        project: first.id.clone(),
        enabled: Some(true),
        slug: Some("acme-status".into()),
        components: None,
    };
    let err = Set::run(&h, &Caller::default(), take()).await;
    assert!(matches!(err, Err(Error::Invalid(_))), "{err:?}");
    // Even off, the page keeps its slug.
    let off = SetInput {
        project: id.clone(),
        enabled: Some(false),
        ..SetInput::default()
    };
    Set::run(&h, &Caller::default(), off).await.unwrap();
    assert!(Set::run(&h, &Caller::default(), take()).await.is_err());
    let free = SetInput {
        slug: Some("shop-status".into()),
        ..take()
    };
    assert!(Set::run(&h, &Caller::default(), free).await.is_ok());
    // Turning a page on needs a slug.
    let no_slug = SetInput {
        project: first.id.clone(),
        enabled: Some(true),
        ..SetInput::default()
    };
    assert!(Set::run(&h, &Caller::default(), no_slug).await.is_err());
}

#[test]
fn hostile_slugs_are_refused_or_become_a_valid_slug() {
    for raw in openlens_testkit::hostile_slugs() {
        assert!(!valid_slug(&raw), "{raw:?} passed as typed");
        if let Ok(s) = check_slug(&raw) {
            assert!(valid_slug(&s), "{raw:?} stored as {s:?}");
            assert_eq!(s, raw.trim().to_ascii_lowercase());
        }
    }
    assert!(check_slug("Acme-Status").is_ok_and(|s| s == "acme-status"));
}

#[test]
fn components_are_production_then_monitored_environments() {
    let monitored = vec!["staging".to_owned()];
    let c = check_components(&[comp("staging", "API", false)], &monitored).unwrap();
    assert_eq!(
        c.first().map(|c| c.environment.as_str()),
        Some("production")
    );
    assert_eq!(c.len(), 2);
    assert!(check_components(&[comp("preview", "x", false)], &monitored).is_err());
    assert!(
        check_components(
            &[comp("staging", "a", false), comp("staging", "b", false)],
            &monitored
        )
        .is_err()
    );
    for t in openlens_testkit::hostile_texts() {
        if let Ok(c) = check_components(&[comp("production", &t, false)], &monitored) {
            assert!(!c[0].name.chars().any(char::is_control), "{t:?}");
        }
    }
}
