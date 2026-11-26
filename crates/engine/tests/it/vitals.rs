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

/// Rewrite every stored file of `dir` through `f`.
fn rewrite(
    dir: &std::path::Path,
    f: impl Fn(&arrow::array::RecordBatch) -> arrow::array::RecordBatch,
) {
    use parquet::arrow::{ArrowWriter, arrow_reader::ParquetRecordBatchReaderBuilder};
    let mut todo = vec![dir.join("events")];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                todo.push(p);
            } else if p.extension().is_some_and(|x| x == "parquet") {
                let r = ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&p).unwrap())
                    .unwrap()
                    .build()
                    .unwrap();
                let out: Vec<_> = r.map(|b| f(&b.unwrap())).collect();
                let mut w =
                    ArrowWriter::try_new(std::fs::File::create(&p).unwrap(), out[0].schema(), None)
                        .unwrap();
                for b in &out {
                    w.write(b).unwrap();
                }
                w.close().unwrap();
            }
        }
    }
}

/// What the board and the Web Vitals page say about the vitals, as JSON.
async fn told(q: &QueryEngine) -> String {
    let b = board(q, "p", &Filters::default(), None).await.unwrap();
    let page = openlens_engine::vitals(q, "p", &Filters::default(), None)
        .await
        .unwrap();
    serde_json::to_string(&(&b.vitals, &b.vitals_why, &page)).unwrap()
}

/// Files written before the typed columns (#449) have none: the board and
/// the Web Vitals page say the same on them as on new files. And on new
/// files they read only the typed columns: emptying `properties` changes
/// nothing.
#[tokio::test]
async fn old_files_without_typed_columns_and_new_ones_say_the_same() {
    let hero = r#","$el":"main#hero > img.cover","$el_type":"image","$res":"/img/hero.jpg","$ttfb":300,"$load_delay":3600,"$load_time":300,"$render_delay":300,"$device":"phone""#;
    let tap = r#","$el":"button.buy","$input":"pointer","$input_delay":40,"$processing":400,"$presentation":60,"$device":"tablet""#;
    let (d, q) = seed(&[
        (
            "/pricing",
            &[
                ("LCP", 4500.0, hero),
                ("INP", 520.0, tap),
                ("TTFB", 300.0, ""),
            ],
        ),
        (
            "/docs",
            &[("LCP", 1200.0, ""), ("CLS", 0.3, r#","$el":"div.ad""#)],
        ),
    ]);
    let new = told(&q).await;
    assert!(
        new.contains("img.cover") && new.contains("button.buy"),
        "{new}"
    );

    let keep = |b: &arrow::array::RecordBatch, drop: &dyn Fn(&str) -> bool| {
        let cols: Vec<usize> = (0..b.num_columns())
            .filter(|&i| !drop(b.schema().field(i).name()))
            .collect();
        b.project(&cols).unwrap()
    };
    // New files whose text says nothing: the typed columns alone.
    let typed = tempfile::tempdir().unwrap();
    copy(d.path(), typed.path());
    rewrite(typed.path(), |b| {
        let i = b.schema().index_of("properties").unwrap();
        let mut cols = b.columns().to_vec();
        cols[i] = std::sync::Arc::new(arrow::array::StringArray::from(vec!["{}"; b.num_rows()]));
        arrow::array::RecordBatch::try_new(b.schema(), cols).unwrap()
    });
    let q = QueryEngine::new(typed.path(), Limits::default());
    assert_eq!(told(&q).await, new, "the typed columns alone");

    // Old files: the text alone.
    rewrite(d.path(), |b| keep(b, &|n| n.starts_with("vital_")));
    let q = QueryEngine::new(d.path(), Limits::default());
    assert_eq!(told(&q).await, new, "the text alone");
}

fn copy(from: &std::path::Path, to: &std::path::Path) {
    for e in std::fs::read_dir(from).unwrap() {
        let p = e.unwrap().path();
        let dst = to.join(p.file_name().unwrap());
        if p.is_dir() {
            std::fs::create_dir_all(&dst).unwrap();
            copy(&p, &dst);
        } else {
            std::fs::copy(&p, &dst).unwrap();
        }
    }
}
