//! Error, acquisition and friction facts as typed columns (#482): old
//! files without them and new files give the same answers.

use super::vitals::{copy, rewrite};
use super::*;

/// The columns added by #482.
const COLUMNS: [&str; 10] = [
    "vital_rating",
    "error_message",
    "error_source",
    "error_stack",
    "acq_channel",
    "acq_utm_source",
    "acq_utm_campaign",
    "acq_ref_host",
    "click_rage",
    "facts_version",
];

/// Ten users who left 20 days ago after rage clicks, slow pages and
/// errors, and ten still here; each landed from a campaign or not.
fn seed() -> (tempfile::TempDir, QueryEngine) {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut evs = Vec::new();
    for i in 0..20usize {
        let gone = i < 10;
        let user = format!("{}{i}", if gone { "c" } else { "r" });
        let ago = if gone { 20 * 24 * 60 } else { 24 * 60 } + i as i64;
        let landing = match i % 4 {
            0 => {
                serde_json::json!({"$channel": "paid", "$utm_source": "news\"letter", "$utm_campaign": "black-friday", "$ref_host": "mail.test"})
            }
            1 => serde_json::json!({"$channel": "search", "$ref_host": "search.test"}),
            2 => serde_json::json!({"$channel": "direct"}),
            _ => serde_json::json!({"$utm_source": "no channel"}),
        };
        // Every list ranks distinct counts: the order of ties is not part
        // of the answer. Two spellings of the first message, one group.
        let error = match i {
            0..=10 => {
                serde_json::json!({"$message": format!("Cannot read \"id\" of undefined (row {})", i % 2), "$source": "https://app.test/a.js?v=2:10:5", "$stack": "TypeError\n    at f (a.js:10:5)"})
            }
            11..=17 => {
                serde_json::json!({"$message": "Timeout\nafter 30s", "$source": "https://app.test/b\"c.js"})
            }
            _ => serde_json::json!({"$stack": "at g (b.js:1:1)"}),
        };
        let rage = gone || i == 10;
        let rating = if gone || i == 11 { "poor" } else { "good" };
        let rows = [
            ("$pageview", landing.to_string(), None),
            (
                "$click",
                serde_json::json!({ "$rage": rage, "$text": "Save" }).to_string(),
                Some("search"),
            ),
            (
                "$vital",
                serde_json::json!({"$name": "LCP", "$value": 3000, "$rating": rating}).to_string(),
                None,
            ),
            ("$error", error.to_string(), None),
        ];
        for (k, (event, props, feature)) in rows.iter().enumerate() {
            let e = E {
                user: &user,
                role: "member",
                session: &user,
                event,
                mins_ago: ago - k as i64,
                feature: *feature,
                release: "v1",
                props,
            };
            evs.push(mk(&e, evs.len()));
        }
    }
    w.append(&evs).unwrap();
    w.flush().unwrap();
    let q = QueryEngine::new(dir.path(), Limits::default());
    (dir, q)
}

/// What the errors, acquisition and churn pages say, as JSON.
async fn told(q: &QueryEngine) -> String {
    use openlens_engine::Touch;
    let f = Filters::default();
    let map = parse("features:\n  - key: search\n    name: Search\n    success: $click\n")
        .map
        .unwrap();
    let errors = openlens_engine::error_groups(q, "p", &openlens_engine::source(None), &f)
        .await
        .unwrap();
    let mut acquisition = Vec::new();
    for by in ["channel", "source", "campaign", "referrer"] {
        for touch in [Touch::First, Touch::Last] {
            acquisition.push(
                openlens_engine::acquisition(q, "p", Some(&map), &f, by, touch)
                    .await
                    .unwrap(),
            );
        }
    }
    let churn = openlens_engine::churn(q, "p", None, &f).await.unwrap();
    let feature = openlens_engine::feature_context(q, "p", &map, &[], &f, "search")
        .await
        .unwrap();
    serde_json::to_string(&(&errors, &acquisition, &churn, &feature)).unwrap()
}

/// Files written before the typed columns of #482 have none: errors,
/// acquisition and churn say the same on them as on new files. And on new
/// files they read only the typed columns: emptying `properties` changes
/// nothing.
#[tokio::test]
async fn old_files_without_fact_columns_and_new_ones_say_the_same() {
    let (d, q) = seed();
    let new = told(&q).await;
    // The facts are there to be found: escaped quotes and newlines in
    // messages, a campaign, rage clicks and poor vitals as friction.
    for seen in [r#"\\\"id\\\""#, "black-friday", "news\\\\", "Rage clicks"] {
        assert!(new.contains(seen), "{seen} in {new}");
    }

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

    // Old files (as written between #449 and #482): the text alone.
    rewrite(d.path(), |b| {
        let keep: Vec<usize> = (0..b.num_columns())
            .filter(|&i| !COLUMNS.contains(&b.schema().field(i).name().as_str()))
            .collect();
        b.project(&keep).unwrap()
    });
    let q = QueryEngine::new(d.path(), Limits::default());
    assert_eq!(told(&q).await, new, "the text alone");
}
