//! Stored totals against the queries (#447), split out of main.rs (#307).

use super::*;
use openlens_query::QueryResult;

#[tokio::test]
async fn stored_totals_match_the_queries_and_follow_new_segments() {
    let (dir, q) = dataset();
    let queried = |sql: &'static str| {
        let q = q.clone();
        async move { q.sql("p", sql).await.unwrap() }
    };
    let check = |n: i64, envs: Vec<String>, rows: &QueryResult, names: &QueryResult| {
        assert_eq!(rows.rows[0]["n"].as_i64(), Some(n));
        let want: Vec<_> = names
            .rows
            .iter()
            .filter_map(|r| r["environment"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(envs, want);
    };
    let sum = q.raw_summary("p").await.unwrap();
    let rows = queried("SELECT count(*) AS n FROM raw_events").await;
    let names = queried("SELECT DISTINCT environment FROM raw_events ORDER BY environment").await;
    check(sum.rows as i64, sum.environments.clone(), &rows, &names);

    // A segment with a new environment is seen by the next call.
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let mut ev = mk(
        &E {
            user: "z",
            role: "admin",
            session: "s9",
            event: "$pageview",
            mins_ago: 1,
            feature: None,
            release: "v2",
            props: "{}",
        },
        99,
    );
    ev.environment = "staging".into();
    w.append(&[ev]).unwrap();
    w.flush().unwrap();
    let after = q.raw_summary("p").await.unwrap();
    assert!(after.rows > sum.rows);
    assert!(after.environments.contains(&"staging".to_owned()));
    let rows = queried("SELECT count(*) AS n FROM raw_events").await;
    let names = queried("SELECT DISTINCT environment FROM raw_events ORDER BY environment").await;
    check(after.rows as i64, after.environments, &rows, &names);
}
