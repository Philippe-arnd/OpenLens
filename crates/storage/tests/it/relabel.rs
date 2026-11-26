//! Re-labelling past events (#484).

use super::*;
use std::collections::BTreeSet;

/// Every stored (day, url, environment) of project `p`.
fn stored(root: &std::path::Path) -> BTreeSet<(String, String, String)> {
    use arrow::array::{Array, StringArray};
    let mut out = BTreeSet::new();
    let pdir = openlens_storage::project_dir(root, "p").unwrap();
    for day in std::fs::read_dir(pdir).unwrap() {
        let day = day.unwrap().path();
        let name = day.file_name().unwrap().to_string_lossy().into_owned();
        for f in std::fs::read_dir(&day).unwrap() {
            let f = f.unwrap().path();
            if f.extension().is_none_or(|x| x != "parquet") {
                continue;
            }
            let r = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
                std::fs::File::open(&f).unwrap(),
            )
            .unwrap()
            .build()
            .unwrap();
            for b in r {
                let b = b.unwrap();
                let col = |n: &str| {
                    b.column_by_name(n)
                        .unwrap()
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .unwrap()
                        .clone()
                };
                let (url, env) = (col("url"), col("environment"));
                for i in 0..b.num_rows() {
                    out.insert((
                        name.clone(),
                        url.value(i).to_owned(),
                        env.value(i).to_owned(),
                    ));
                }
            }
        }
    }
    out
}

/// The dry run counts and writes nothing; the rewrite moves only the
/// chosen host's events inside the window, and drops the closed day's
/// rollup so the next pass computes it again.
#[test]
fn only_the_hosts_events_in_the_window_move_and_the_rollup_is_redone() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let now = Utc::now();
    let recent = now - Duration::days(3);
    let old = now - Duration::days(20);
    let at = |ts, url: &str| {
        let mut e = ev("p", ts);
        e.url = Some(url.into());
        e
    };
    let (mut w, _) = Writer::open(root, FlushPolicy::default()).unwrap();
    w.append(&[
        at(recent, "https://a.example.com/x"),
        at(recent, "https://b.example.com/x"),
        at(old, "https://a.example.com/old"),
    ])
    .unwrap();
    w.flush().unwrap();
    let day = recent.date_naive();
    let tables: Vec<_> = openlens_storage::rollup::tables()
        .into_iter()
        .map(|(n, s)| (n, arrow::array::RecordBatch::new_empty(s)))
        .collect();
    let g = openlens_storage::rollup::generation();
    assert!(openlens_storage::rollup::write(root, "p", day, g, &tables).unwrap());

    let target = |url: Option<&str>, _env: &str| {
        url.filter(|u| u.contains("://a.example.com/"))
            .map(|_| "staging".to_owned())
    };
    let (from, to) = ((now - Duration::days(7)).date_naive(), now.date_naive());
    let before = stored(root);
    let dry = openlens_storage::relabel::relabel(root, "p", from, to, &target, true).unwrap();
    let key = ("production".to_owned(), "staging".to_owned());
    assert_eq!(dry.get(&key), Some(&1), "{dry:?}");
    assert_eq!(stored(root), before, "a dry run writes nothing");
    assert!(openlens_storage::rollup::has(root, "p", day).unwrap());

    let done = openlens_storage::relabel::relabel(root, "p", from, to, &target, false).unwrap();
    assert_eq!(done, dry, "the rewrite moves what the dry run counted");
    let after = stored(root);
    let env_of = |url: &str| -> Vec<&str> {
        after
            .iter()
            .filter(|(_, u, _)| u == url)
            .map(|(_, _, e)| e.as_str())
            .collect()
    };
    assert_eq!(env_of("https://a.example.com/x"), ["staging"]);
    assert_eq!(env_of("https://b.example.com/x"), ["production"]);
    assert_eq!(env_of("https://a.example.com/old"), ["production"]);
    assert_eq!(count_rows(root, "p").unwrap(), 3);
    assert!(
        openlens_storage::rollup::missing(root, "p", now)
            .unwrap()
            .contains(&day),
        "the rewritten closed day is rolled up again"
    );
    let again = openlens_storage::relabel::relabel(root, "p", from, to, &target, false).unwrap();
    assert!(again.is_empty(), "{again:?}");
}
