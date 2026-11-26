//! Status announcements and incident updates (#562): the checks on what a
//! person posts, the order of updates, the pinned notice, and what the
//! Uptime page proposes about incidents.

use openlens_engine::commands::announcements::{
    CreateInput, MAX_BODY, UpdateInput, after_update, changed_fields, check_body, check_components,
    check_title, listing, new_fields, pinned, public,
};
use openlens_meta::Meta;

const P: &str = "p";

fn input(title: &str) -> CreateInput {
    CreateInput {
        project: P.into(),
        title: title.into(),
        ..CreateInput::default()
    }
}

#[test]
fn posted_text_is_kept_as_typed_or_refused_never_rewritten() {
    for t in openlens_testkit::hostile_texts() {
        let trimmed = t.replace("\r\n", "\n");
        let trimmed = trimmed.trim();
        match check_body(&t) {
            // Markup is text: nothing is stripped or escaped in storage.
            Ok(kept) => assert_eq!(kept, trimmed, "{t:?} was rewritten"),
            Err(_) => assert!(
                t.chars().count() > MAX_BODY
                    || t.chars().any(|c| c.is_control() && c != '\n' && c != '\t'),
                "{t:?} was refused"
            ),
        }
        if let Ok(title) = check_title(&t) {
            assert_eq!(title, trimmed);
            assert!(!title.chars().any(char::is_control));
        }
    }
    assert!(check_body(&"é".repeat(MAX_BODY)).is_ok());
    assert!(check_body(&"é".repeat(MAX_BODY + 1)).is_err());
    assert!(check_title("two\nlines").is_err());
}

#[test]
fn components_are_environments_and_default_to_production() {
    assert_eq!(check_components(&[]).unwrap(), vec!["production"]);
    assert_eq!(
        check_components(&["Staging".into(), "staging".into()]).unwrap(),
        vec!["staging"]
    );
    assert!(check_components(&["ignore".into()]).is_err());
    assert!(check_components(&["<b>".into()]).is_err());
}

#[test]
fn updates_come_back_in_the_order_they_were_posted_and_resolved_ends_it() {
    let m = Meta::in_memory().unwrap();
    let now = chrono::Utc::now().timestamp();
    let mut i = input("Exports are slow");
    i.level = Some("degraded".into());
    i.state = Some("investigating".into());
    i.body = "We are looking into it.".into();
    let (f, first) = new_fields(&i, "UTC", now).unwrap();
    let a = m
        .create_announcement(
            P,
            &f,
            first.as_ref().map(|(s, b)| (s.as_str(), b.as_str())),
            now,
        )
        .unwrap();
    for (k, state) in ["identified", "monitoring", "resolved"].iter().enumerate() {
        let at = now + 60 * (k as i64 + 1);
        let fields = after_update(&a.fields, state, at);
        m.add_announcement_update(P, a.id, state, &format!("step {state}"), Some(&fields), at)
            .unwrap()
            .unwrap();
    }
    let listed = public(&m, P, now + 600).unwrap();
    let got = listed.iter().find(|x| x.id == a.id).unwrap();
    let states: Vec<&str> = got.updates.iter().map(|u| u.state.as_str()).collect();
    assert_eq!(
        states,
        ["investigating", "identified", "monitoring", "resolved"]
    );
    assert!(got.updates.windows(2).all(|w| w[0].at <= w[1].at));
    assert_eq!(got.fields.level, "resolved");
    assert!(got.fields.ends_at.is_some_and(|e| e <= now + 600));
}

#[test]
fn the_public_read_drops_what_ended_more_than_90_days_ago() {
    let m = Meta::in_memory().unwrap();
    let now = chrono::Utc::now().timestamp();
    let day = 86_400;
    let (mut old, _) = new_fields(&input("Old"), "UTC", now).unwrap();
    old.starts_at = now - 120 * day;
    old.ends_at = Some(now - 100 * day);
    let old = m.create_announcement(P, &old, None, now).unwrap();
    let recent = m
        .create_announcement(
            P,
            &new_fields(&input("Recent"), "UTC", now).unwrap().0,
            None,
            now,
        )
        .unwrap();
    let ids: Vec<i64> = public(&m, P, now).unwrap().iter().map(|a| a.id).collect();
    assert!(ids.contains(&recent.id) && !ids.contains(&old.id));
    assert!(public(&m, "other", now).unwrap().is_empty());
}

#[test]
fn one_information_notice_is_pinned_at_most() {
    let m = Meta::in_memory().unwrap();
    let now = chrono::Utc::now().timestamp();
    let mut a = input("Maintenance on Sunday");
    a.pinned = true;
    let a = m
        .create_announcement(P, &new_fields(&a, "UTC", now).unwrap().0, None, now)
        .unwrap();
    let mut b = input("New export format");
    b.pinned = true;
    let b = m
        .create_announcement(P, &new_fields(&b, "UTC", now).unwrap().0, None, now)
        .unwrap();
    let list = public(&m, P, now).unwrap();
    assert_eq!(list.iter().filter(|x| x.fields.pinned).count(), 1);
    assert_eq!(pinned(&list, now).map(|x| x.id), Some(b.id));
    assert_ne!(a.id, b.id);
    // Only information is pinned; an outage that was one loses its pin.
    let mut c = input("Down");
    c.pinned = true;
    c.level = Some("outage".into());
    assert!(new_fields(&c, "UTC", now).is_err());
    let change = UpdateInput {
        id: b.id,
        level: Some("outage".into()),
        ..UpdateInput::default()
    };
    assert!(
        !changed_fields(&b.fields, &change, "UTC", now)
            .unwrap()
            .pinned
    );
}

#[test]
fn incidents_are_offered_an_update_then_a_resolution() {
    let m = Meta::in_memory().unwrap();
    let now = chrono::Utc::now().timestamp();
    let inc = m.start_incident(P, "down", now - 300, "HTTP 502").unwrap();
    let offers = listing(&m, P, 90, now).unwrap().offers;
    assert!(
        offers
            .iter()
            .any(|o| o.kind == "write_update" && o.incident.id == inc)
    );
    let mut i = input("The app is down");
    i.level = Some("outage".into());
    i.incident = Some(inc);
    i.state = Some("investigating".into());
    let (f, first) = new_fields(&i, "UTC", now).unwrap();
    let a = m
        .create_announcement(
            P,
            &f,
            first.as_ref().map(|(s, b)| (s.as_str(), b.as_str())),
            now,
        )
        .unwrap();
    assert!(listing(&m, P, 90, now).unwrap().offers.is_empty());
    // The probe closes the incident: the announcement stays open, and the
    // page proposes to mark it resolved.
    m.end_incident(inc, now).unwrap();
    let l = listing(&m, P, 90, now).unwrap();
    assert!(
        l.announcements.iter().any(|x| x.announcement.id == a.id
            && x.announcement.fields.level == "outage"
            && x.active)
    );
    assert!(
        l.offers
            .iter()
            .any(|o| o.kind == "mark_resolved" && o.announcement == Some(a.id))
    );
    let f = after_update(&a.fields, "resolved", now + 1);
    m.add_announcement_update(P, a.id, "resolved", "Fixed.", Some(&f), now + 1)
        .unwrap();
    assert!(listing(&m, P, 90, now + 2).unwrap().offers.is_empty());
}

#[test]
fn times_are_read_in_the_project_zone_and_bounded() {
    use openlens_engine::commands::announcements::parse_time;
    let now = chrono::Utc::now().timestamp();
    let day = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let utc = parse_time(&format!("{day} 14:02"), "UTC", now)
        .unwrap()
        .unwrap();
    let paris = parse_time(&format!("{day}T14:02"), "Europe/Paris", now)
        .unwrap()
        .unwrap();
    assert!(paris < utc, "14:02 in Paris is before 14:02 UTC");
    assert_eq!(parse_time(" ", "UTC", now).unwrap(), None);
    assert!(parse_time("tomorrow", "UTC", now).is_err());
    assert!(parse_time("2099-01-01T00:00:00Z", "UTC", now).is_err());
    let mut i = input("Window");
    i.start = Some(format!("{day} 14:00"));
    i.end = Some(format!("{day} 13:00"));
    assert!(
        new_fields(&i, "UTC", now).is_err(),
        "an end before its start"
    );
}
