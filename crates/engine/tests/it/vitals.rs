//! Web Vitals explained (#408): worst pages, dominant cause and actions
//! over seeded `$vital` events.

use super::*;

/// `/slow` has a poor LCP mostly spent waiting for the server; `/fast`
/// is fine. Ten visits each, so both pages have enough measurements.
fn vitals_dataset() -> (tempfile::TempDir, QueryEngine) {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut evs = Vec::new();
    for i in 0..10 {
        for (path, lcp, ttfb) in [("/slow", 7000, 5000), ("/fast", 1200, 200)] {
            for (name, v) in [("LCP", lcp), ("TTFB", ttfb)] {
                let user = format!("u{i}");
                let props = format!(r#"{{"$name":"{name}","$value":{v}}}"#);
                let mut e = mk(
                    &E {
                        user: &user,
                        role: "member",
                        session: &user,
                        event: "$vital",
                        mins_ago: 10 + i,
                        feature: None,
                        release: "v1",
                        props: &props,
                    },
                    evs.len(),
                );
                e.path = Some(path.into());
                evs.push(e);
            }
        }
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    (dir, q)
}

#[tokio::test]
async fn vitals_rank_the_worst_page_and_lead_with_its_cause() {
    let (_d, q) = vitals_dataset();
    let v = openlens_engine::vitals(&q, "p", &Filters::default(), None)
        .await
        .unwrap();
    let lcp = v.iter().find(|r| r.name == "LCP").unwrap();
    assert_ne!(lcp.rating, "good");
    assert_eq!(lcp.worst_pages[0].path, "/slow");
    assert!(lcp.worst_pages[0].p75 > lcp.worst_pages.last().unwrap().p75);
    assert_eq!(lcp.dominant_cause.as_ref().unwrap().cause, "server");
    // FCP is not measured: still explained, without pages.
    let fcp = v.iter().find(|r| r.name == "FCP").unwrap();
    assert!(fcp.p75.is_none() && fcp.worst_pages.is_empty() && !fcp.actions.is_empty());

    // The board explains the same vital, and the card names its first action.
    let b = board(&q, "p", &Filters::default(), None).await.unwrap();
    let why = b.vitals_why.iter().find(|r| r.name == "LCP").unwrap();
    assert_eq!(why.first_action(), lcp.first_action());
    assert!(b.vitals_why.iter().all(|r| r.rating != "good"));
    let card = b.insights.iter().find(|i| i.kind == "slow_pages").unwrap();
    assert!(card.next_step.contains(lcp.first_action().unwrap()));
}
