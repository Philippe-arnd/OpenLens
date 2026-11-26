//! The board's release window (#323, D-055) on stored events: the board
//! pages read their period blocks since the current release and compare
//! them with the same length before; every other caller keeps the fixed
//! windows.

use super::*;
use openlens_engine::Window;

fn ev(n: usize, release: Option<&str>, user: usize, hours_ago: i64) -> Event {
    let ts = Utc::now() - Duration::hours(hours_ago);
    Event {
        uuid: format!("w-{n}"),
        project: "p".into(),
        event: "$pageview".into(),
        distinct_id: format!("u{user}"),
        session_id: Some(format!("s{user}-{hours_ago}")),
        timestamp: ts,
        received_at: ts,
        url: None,
        path: Some("/".into()),
        feature: None,
        role: None,
        account: None,
        plan: None,
        environment: "production".into(),
        release: release.map(Into::into),
        flags: None,
        properties: "{}".into(),
    }
}

fn store(rows: &[(Option<&str>, std::ops::Range<usize>, i64)]) -> (tempfile::TempDir, QueryEngine) {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut evs = Vec::new();
    for (release, users, hours_ago) in rows {
        for u in users.clone() {
            let n = evs.len();
            evs.push(ev(n, *release, u, *hours_ago));
        }
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    (dir, q)
}

fn release_window() -> Filters {
    Filters {
        window: Window::Release,
        ..Filters::default()
    }
}

#[tokio::test]
async fn the_window_starts_at_the_current_release_and_compares_the_same_length_before() {
    // v1 for weeks, then v2 about three days ago, which now has all the
    // traffic. Every event is at noon UTC, counted from yesterday's noon,
    // so none sits near a day boundary whatever the hour the test runs at
    // (it was flaky when events were whole days before `now`).
    let now = Utc::now();
    let noon = (now.date_naive() - Duration::days(1))
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_utc();
    let h = (now - noon).num_hours();
    let (_d, q) = store(&[
        (Some("v1"), 0..4, h + 19 * 24),
        (Some("v1"), 0..2, h + 4 * 24),
        (Some("v2"), 0..4, h + 2 * 24),
        (Some("v2"), 0..4, h),
    ]);
    let b = board(&q, "p", &release_window(), None).await.unwrap();
    let w = b.window.as_ref().unwrap();
    assert_eq!(w.release.as_deref(), Some("v2"));
    assert!((3..=4).contains(&w.days), "about three days: {}", w.days);
    assert!(w.label.starts_with("Since v2 ("), "{}", w.label);
    // The same length before it holds v1's last two users only.
    assert!(w.current.users > w.previous.users);
    assert!(w.previous.users > 0);
    assert!(w.current.sessions.count > w.previous.sessions.count);
    // The window narrows the period blocks: the fixed 7 days also hold
    // v1's sessions of five days ago.
    let fixed = board(&q, "p", &Filters::default(), None).await.unwrap();
    assert!(fixed.window.is_none(), "fixed windows by default");
    assert!(b.sessions.count < fixed.sessions.count);
    assert!(b.dau_series.len() < fixed.dau_series.len());
    // The KPIs with their own windows do not move.
    assert_eq!(b.users.mau, fixed.users.mau);
    assert_eq!(b.users.known, fixed.users.known);
    // A board filtered on one release already reads that release.
    let on_v1 = Filters {
        release: Some("v1".into()),
        ..release_window()
    };
    assert!(board(&q, "p", &on_v1, None).await.unwrap().window.is_none());
}

#[tokio::test]
async fn without_a_release_the_window_is_14_days_against_the_14_before() {
    let (_d, q) = store(&[(None, 0..3, 20 * 24), (None, 0..2, 2 * 24)]);
    let b = board(&q, "p", &release_window(), None).await.unwrap();
    let w = b.window.as_ref().unwrap();
    assert!(w.release.is_none());
    assert_eq!(w.days, 14);
    assert_eq!(w.label, "Last 14 days");
    assert!(w.previous.users > w.current.users, "three users then two");
    assert!(w.current.users > 0);
}

#[tokio::test]
async fn a_date_range_or_a_past_release_narrows_the_period_blocks() {
    // v1 three weeks ago, v2 for the last two days.
    let (_d, q) = store(&[(Some("v1"), 0..3, 20 * 24), (Some("v2"), 0..2, 2 * 24)]);
    let today = Utc::now().date_naive();
    let range = Filters {
        window: Window::Range {
            from: today - Duration::days(21),
            to: today - Duration::days(10),
        },
        ..Filters::default()
    };
    let b = board(&q, "p", &range, None).await.unwrap();
    let w = b.window.as_ref().unwrap();
    assert_eq!(w.days, 12);
    assert!(w.label.starts_with("From "), "{}", w.label);
    assert!(w.until.is_some(), "a range in the past has an end");
    assert!(
        w.current.users > w.previous.users,
        "v1's users, none before"
    );
    assert_eq!(b.sessions.count, w.current.sessions.count);
    let recent = Filters {
        window: Window::Range {
            from: today - Duration::days(3),
            to: today,
        },
        ..Filters::default()
    };
    let r = board(&q, "p", &recent, None).await.unwrap();
    assert!(
        r.sessions.count < b.sessions.count,
        "two sessions, not three"
    );
    let picker = &r.window.as_ref().unwrap().releases;
    assert!(picker.contains(&"v1".to_owned()), "{picker:?}");
    // A past release ends where the next one starts.
    let on_v1 = Filters {
        window: Window::Since {
            release: "v1".into(),
        },
        ..Filters::default()
    };
    let p = board(&q, "p", &on_v1, None).await.unwrap();
    let pw = p.window.as_ref().unwrap();
    assert!(pw.label.starts_with("During v1"), "{}", pw.label);
    assert!(pw.until.is_some());
    // A range in the future is refused.
    let ahead = Filters {
        window: Window::Range {
            from: today,
            to: today + Duration::days(2),
        },
        ..Filters::default()
    };
    assert!(board(&q, "p", &ahead, None).await.is_err());
}

/// A range before today handed its period blocks back reads them from
/// there, not from the days, and still reads the blocks that end now
/// (#516, D-064).
#[tokio::test]
async fn a_past_range_reuses_its_kept_period_blocks() {
    let (dir, q) = store(&[(None, 0..3, 10 * 24)]);
    let today = Utc::now().date_naive();
    let range = Filters {
        window: Window::Range {
            from: today - Duration::days(12),
            to: today - Duration::days(1),
        },
        ..Filters::default()
    };
    let first = board(&q, "p", &range, None).await.unwrap();
    let kept = openlens_engine::PeriodBlocks::of(&first);
    // Late events on a day of the range, from new people.
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let late: Vec<Event> = (10..14).map(|u| ev(1000 + u, None, u, 5 * 24)).collect();
    w.append(&late).unwrap();
    w.flush().unwrap();
    let fresh = board(&q, "p", &range, None).await.unwrap();
    assert!(fresh.sessions.count > first.sessions.count);
    let reused = openlens_engine::board_reusing(&q, "p", &range, None, Some(&kept))
        .await
        .unwrap();
    assert_eq!(reused.sessions.count, first.sessions.count, "kept");
    assert_eq!(reused.dau_series, first.dau_series);
    // MAU ends now: it is read again.
    assert_eq!(reused.users.mau, fresh.users.mau);
    assert!(reused.users.mau > first.users.mau);
}
