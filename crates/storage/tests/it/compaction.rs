//! Compaction as a streaming merge (#418, part 3): the same day as before,
//! whatever its inputs look like.

use super::*;
use std::collections::BTreeSet;

use arrow::array::{Array, RecordBatch, StringArray, TimestampMicrosecondArray, UInt32Array};
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

fn read_day(day: &std::path::Path) -> Vec<RecordBatch> {
    let mut out = Vec::new();
    for f in std::fs::read_dir(day).unwrap() {
        let p = f.unwrap().path();
        if p.extension().is_some_and(|x| x == "parquet") {
            let r = ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&p).unwrap())
                .unwrap()
                .build()
                .unwrap();
            out.extend(r.map(Result::unwrap));
        }
    }
    out
}

fn uuids(batches: &[RecordBatch]) -> Vec<String> {
    batches
        .iter()
        .flat_map(|b| {
            let c = b.column_by_name("uuid").unwrap();
            let c = c.as_any().downcast_ref::<StringArray>().unwrap();
            (0..c.len())
                .map(|i| c.value(i).to_owned())
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn a_day_of_mixed_segments_compacts_sorted_once_per_uuid() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let day0 = Utc.with_ymd_and_hms(2026, 11, 20, 0, 0, 0).unwrap();
    let names = ["$pageview", "$click", "$error", "signup", "$click"];
    let mut n = 0;
    let mut batch = |w: &mut Writer, count: usize| {
        let evs: Vec<Event> = (0..count)
            .map(|_| {
                n += 1;
                // Timestamps out of order within a flush.
                let mut e = ev("p", day0 + Duration::seconds(((n * 7919) % 50_000) as i64));
                e.event = names[n % names.len()].into();
                e
            })
            .collect();
        w.append(&evs).unwrap();
        w.flush().unwrap();
        evs
    };
    // An already compacted file, then new segments on the same day.
    batch(&mut w, 300);
    batch(&mut w, 300);
    let day = dir.path().join("events/project=p/date=2026-11-20");
    assert_eq!(compact(dir.path(), "p", Utc::now()).unwrap(), 1);
    let first = batch(&mut w, 250);
    batch(&mut w, 400);

    // A segment from before segments were sorted: one of the new
    // segments' rows, reversed, plus a resent copy of events stored above.
    let mut rows = read_day(&day);
    let last = rows.pop().unwrap();
    let rev: UInt32Array = (0..last.num_rows() as u32).rev().collect();
    let legacy = arrow::compute::take_record_batch(&last, &rev).unwrap();
    let path = day.join("00000000-legacy.parquet");
    let mut wr =
        ArrowWriter::try_new(std::fs::File::create(&path).unwrap(), legacy.schema(), None).unwrap();
    wr.write(&legacy).unwrap();
    wr.close().unwrap();

    let before = read_day(&day);
    let expected: BTreeSet<String> = uuids(&before).into_iter().collect();
    assert!(
        uuids(&before).len() > expected.len(),
        "the day must hold duplicates"
    );
    assert!(first.iter().all(|e| expected.contains(&e.uuid)));

    assert_eq!(compact(dir.path(), "p", Utc::now()).unwrap(), 1);
    let after = read_day(&day);
    let files = std::fs::read_dir(&day)
        .unwrap()
        .filter(|f| {
            f.as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|x| x == "parquet")
        })
        .count();
    assert_eq!(files, 1);
    let got = uuids(&after);
    assert_eq!(got.len(), expected.len(), "one row per uuid");
    assert_eq!(got.iter().cloned().collect::<BTreeSet<_>>(), expected);

    // Sorted by (event, timestamp), across batches.
    let mut keys = Vec::new();
    for b in &after {
        let e = b.column_by_name("event").unwrap();
        let e = e.as_any().downcast_ref::<StringArray>().unwrap();
        let t = b.column_by_name("timestamp").unwrap();
        let t = t
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .unwrap();
        keys.extend((0..b.num_rows()).map(|i| (e.value(i).to_owned(), t.value(i))));
    }
    assert!(keys.windows(2).all(|k| k[0] <= k[1]));
}

/// A segment written before the typed columns (#449, #482) has none: compaction
/// merges it with new segments and fills them in from `properties`, the
/// same values a new segment gets at write time.
#[test]
fn a_segment_without_typed_columns_merges_and_gets_them() {
    let dir = tempfile::tempdir().unwrap();
    let (mut w, _) = Writer::open(dir.path(), FlushPolicy::default()).unwrap();
    let t = Utc.with_ymd_and_hms(2026, 11, 20, 9, 0, 0).unwrap();
    let vital = |i: i64| {
        let mut e = ev("p", t + Duration::seconds(i));
        e.event = "$vital".into();
        e.properties =
            r#"{"$name":"LCP","$value":4200,"$el":"main > img","$el_type":"image","$ttfb":300,"$rating":"poor"}"#
                .into();
        e
    };
    w.append(&[vital(1), ev("p", t)]).unwrap();
    w.flush().unwrap();
    w.append(&[vital(2)]).unwrap();
    w.flush().unwrap();
    let day = dir.path().join("events/project=p/date=2026-11-20");

    // Rewrite one segment as an old one: without any typed column.
    let typed = openlens_storage::typed_columns();
    let seg = std::fs::read_dir(&day)
        .unwrap()
        .map(|f| f.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "parquet"))
        .unwrap();
    let r = ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&seg).unwrap())
        .unwrap()
        .build()
        .unwrap();
    let old: Vec<RecordBatch> = r
        .map(|b| {
            let b = b.unwrap();
            let keep: Vec<usize> = (0..b.num_columns())
                .filter(|&i| !typed.contains(b.schema().field(i).name()))
                .collect();
            b.project(&keep).unwrap()
        })
        .collect();
    std::fs::remove_file(&seg).unwrap();
    let path = day.join("00000000-old.parquet");
    let mut wr =
        ArrowWriter::try_new(std::fs::File::create(&path).unwrap(), old[0].schema(), None).unwrap();
    for b in &old {
        wr.write(b).unwrap();
    }
    wr.close().unwrap();
    assert!(old[0].column_by_name("vital_name").is_none());

    assert_eq!(compact(dir.path(), "p", Utc::now()).unwrap(), 1);
    let after = read_day(&day);
    let b = arrow::compute::concat_batches(&after[0].schema(), &after).unwrap();
    let col = |n: &str| {
        b.column_by_name(n)
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .clone()
    };
    let (event, name, el, kind, rating) = (
        col("event"),
        col("vital_name"),
        col("vital_el"),
        col("vital_kind"),
        col("vital_rating"),
    );
    let vitals: Vec<usize> = (0..b.num_rows())
        .filter(|&i| event.value(i) == "$vital")
        .collect();
    assert_eq!(vitals.len(), 2);
    for &i in &vitals {
        assert!(!name.is_null(i) && !el.is_null(i) && !kind.is_null(i) && !rating.is_null(i));
        assert_eq!(
            (name.value(i), el.value(i), kind.value(i), rating.value(i)),
            (
                name.value(vitals[0]),
                el.value(vitals[0]),
                kind.value(vitals[0]),
                rating.value(vitals[0])
            ),
            "an old row gets the values a new row has"
        );
    }
    let other = (0..b.num_rows())
        .find(|&i| event.value(i) != "$vital")
        .unwrap();
    assert!(name.is_null(other), "only a $vital has facts");
    // Every row now says it carries the facts of #482, the old ones too.
    assert_eq!(b.column_by_name("facts_version").unwrap().null_count(), 0);
}
