//! A feedback's text leaves the files after 90 days (#718, D-116): the
//! `$feedback` row stays, with its feature and rating, so the counts per
//! rating hold; the words a person wrote do not.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, RecordBatch, StringArray};
use chrono::{DateTime, Utc};
use openlens_event::feedback::{TEXT, TEXT_DAYS};
use openlens_event::names::FEEDBACK;

use crate::{Result, project_dir, rewrite_day};

/// The marker of a day already trimmed: its file listing then.
const TRIMMED: &str = ".feedback-trimmed";

/// Remove the text of every `$feedback` stored on a day older than
/// [`TEXT_DAYS`]. Returns the feedbacks trimmed. A day is read again only
/// when its files changed. Same preconditions as [`crate::erase`].
pub fn trim_feedback(root: &Path, project: &str, now: DateTime<Utc>) -> Result<usize> {
    let pdir = project_dir(root, project)?;
    if !pdir.is_dir() {
        return Ok(0);
    }
    let cutoff = (now - chrono::Duration::days(TEXT_DAYS)).date_naive();
    let mut total = 0;
    for entry in fs::read_dir(&pdir)? {
        let date = entry?.path();
        let Some(day) = date
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|n| n.strip_prefix("date="))
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        else {
            continue;
        };
        if !date.is_dir() || day >= cutoff {
            continue;
        }
        if fs::read_to_string(date.join(TRIMMED)).ok() == Some(listing(&date)?) {
            continue;
        }
        total += rewrite_day(&date, &mut without_text)?;
        fs::write(date.join(TRIMMED), listing(&date)?)?;
    }
    Ok(total)
}

/// The day's Parquet file names, sorted, one per line.
fn listing(date: &Path) -> Result<String> {
    let mut names: Vec<String> = fs::read_dir(date)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".parquet"))
        .collect();
    names.sort();
    Ok(names.join("\n"))
}

/// `b` with the text removed from `$feedback` properties.
fn without_text(b: &RecordBatch) -> Result<(RecordBatch, usize)> {
    let (Some(ev), Some((idx, props))) = (
        b.column_by_name("event")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>()),
        b.schema().index_of("properties").ok().and_then(|i| {
            b.column(i)
                .as_any()
                .downcast_ref::<StringArray>()
                .map(|c| (i, c.clone()))
        }),
    ) else {
        return Ok((b.clone(), 0));
    };
    let mut changed = 0;
    let out: StringArray = (0..b.num_rows())
        .map(|i| {
            if props.is_null(i) {
                return None;
            }
            let raw = props.value(i);
            if ev.is_null(i) || ev.value(i) != FEEDBACK {
                return Some(raw.to_owned());
            }
            let Ok(serde_json::Value::Object(mut m)) = serde_json::from_str(raw) else {
                return Some(raw.to_owned());
            };
            if m.remove(TEXT).is_none() {
                return Some(raw.to_owned());
            }
            changed += 1;
            Some(serde_json::Value::Object(m).to_string())
        })
        .collect();
    if changed == 0 {
        return Ok((b.clone(), 0));
    }
    let mut cols = b.columns().to_vec();
    cols[idx] = Arc::new(out) as ArrayRef;
    Ok((RecordBatch::try_new(b.schema(), cols)?, changed))
}
