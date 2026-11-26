//! Web Vitals explained (#408, #467): worst pages, dominant cause,
//! guilty elements and actions over seeded `$vital` events.

use super::*;

/// `(name, value, extra properties)` of one `$vital`.
type Sent<'a> = (&'a str, f64, &'a str);

/// Ten visits of each page; each visit sends every `$vital` of its page.
fn seed(pages: &[(&str, &[Sent<'_>])]) -> (tempfile::TempDir, QueryEngine) {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut evs = Vec::new();
    for i in 0..10 {
        for (path, vitals) in pages {
            for (name, v, extra) in *vitals {
                let user = format!("u{i}");
                let props = format!(r#"{{"$name":"{name}","$value":{v}{extra}}}"#);
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
                e.path = Some((*path).into());
                evs.push(e);
            }
        }
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    (dir, q)
}

/// An old SDK: `/slow` has a poor LCP mostly spent waiting for the server;
/// `/fast` is fine. No attribution at all.
#[tokio::test]
async fn vitals_rank_the_worst_page_and_lead_with_its_cause() {
    let (_d, q) = seed(&[
        ("/slow", &[("LCP", 7000.0, ""), ("TTFB", 5000.0, "")]),
        ("/fast", &[("LCP", 1200.0, ""), ("TTFB", 200.0, "")]),
    ]);
    let v = openlens_engine::vitals(&q, "p", &Filters::default(), None)
        .await
        .unwrap();
    let lcp = v.iter().find(|r| r.name == "LCP").unwrap();
    assert_ne!(lcp.rating, "good");
    assert_eq!(lcp.worst_pages[0].path, "/slow");
    assert!(lcp.worst_pages[0].p75 > lcp.worst_pages.last().unwrap().p75);
    assert_eq!(lcp.dominant_cause.as_ref().unwrap().cause, "server");
    // Nothing to attribute: no element, no device, the cause still named.
    assert!(lcp.elements.is_empty() && lcp.devices.is_empty());
    assert!(lcp.likely_cause.is_some());
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

/// A new SDK: `/pricing`'s hero image starts downloading late (the server
/// answers fast). The board names the page, the image and preloading it
/// first, even though the TTFB share alone would not single it out.
#[tokio::test]
async fn a_late_hero_image_is_named_with_preload_first() {
    let hero = r#","$el":"main#hero > img.cover","$el_type":"image","$res":"/img/hero.jpg","$ttfb":300,"$load_delay":3600,"$load_time":300,"$render_delay":300,"$device":"phone""#;
    let fine = r#","$el":"h1.title","$el_type":"text","$ttfb":200,"$load_delay":0,"$load_time":0,"$render_delay":1000,"$device":"desktop""#;
    let (_d, q) = seed(&[
        ("/pricing", &[("LCP", 4500.0, hero), ("TTFB", 300.0, "")]),
        ("/docs", &[("LCP", 1200.0, fine), ("TTFB", 200.0, "")]),
    ]);
    let b = board(&q, "p", &Filters::default(), None).await.unwrap();
    let lcp = b.vitals_why.iter().find(|r| r.name == "LCP").unwrap();
    assert_eq!(lcp.worst_pages[0].path, "/pricing");
    let top = &lcp.elements[0];
    assert_eq!(
        (top.element.as_str(), top.path.as_str()),
        ("main#hero > img.cover", "/pricing")
    );
    assert!(
        lcp.elements.iter().all(|c| c.path != "/docs"),
        "a good page's element is not blamed"
    );
    let first = lcp.first_action().unwrap();
    assert!(
        first.contains("Preload") && first.contains("fetchpriority"),
        "{first}"
    );
    let d = lcp.dominant_cause.as_ref().unwrap();
    assert_eq!(d.phase.as_ref().unwrap().key, "load_delay");
    let likely = lcp.likely_cause.as_deref().unwrap();
    assert!(
        likely.contains("img.cover") && likely.contains("/pricing"),
        "{likely}"
    );
    // Phones are slower than desktops here.
    assert_eq!(lcp.devices[0].device, "phone");
    assert!(lcp.devices[0].p75 > lcp.devices.last().unwrap().p75);
    // The card points at the element too.
    let card = b.insights.iter().find(|i| i.kind == "slow_pages").unwrap();
    assert!(card.next_step.contains("img.cover"), "{}", card.next_step);
}
