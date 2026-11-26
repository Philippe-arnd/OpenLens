//! Typed columns derived from `properties` at write time (#449, D-037):
//! the Web Vitals facts of `$vital` events, so readers stop parsing the
//! JSON text with a regular expression per row.
//!
//! They are added at the end of [`crate::event_schema`] and are nullable.
//! Files written before them have no such columns: the query layer reads
//! them as NULL and falls back to the text, and every rewrite (compaction,
//! erasure, click trimming) fills them in from `properties` on the way.

use std::sync::Arc;

use arrow::array::{Array, ArrayRef, Float64Builder, RecordBatch, StringArray, StringBuilder};
use arrow::datatypes::{DataType, Field};
use openlens_event::{Vital, vital::PARTS};

use crate::{Result, event_schema};

/// Reads one text fact.
type Getter = fn(&Vital) -> Option<&str>;

/// The text columns, in schema order, and how each is read from a fact.
const TEXT: [(&str, Getter); 5] = [
    ("vital_name", |v| Some(&v.name)),
    ("vital_device", |v| v.device.as_deref()),
    ("vital_el", |v| v.el.as_deref()),
    ("vital_kind", |v| v.kind.as_deref()),
    ("vital_res", |v| v.res.as_deref()),
];

/// The typed columns' names, in schema order.
pub fn names() -> Vec<String> {
    let mut v: Vec<String> = TEXT.iter().map(|(n, _)| (*n).to_owned()).collect();
    v.push("vital_value".to_owned());
    v.extend(PARTS.iter().map(|p| format!("vital_{p}")));
    v
}

/// The fields appended to the event schema.
pub(crate) fn fields() -> Vec<Field> {
    names()
        .into_iter()
        .map(|n| {
            let t = if TEXT.iter().any(|(t, _)| *t == n) {
                DataType::Utf8
            } else {
                DataType::Float64
            };
            Field::new(n, t, true)
        })
        .collect()
}

/// The typed columns, in schema order, for rows given as
/// `(event, properties)`.
pub(crate) fn columns<'a>(
    rows: impl ExactSizeIterator<Item = (&'a str, &'a str)>,
) -> Vec<ArrayRef> {
    let facts: Vec<Option<Vital>> = rows.map(|(e, p)| openlens_event::vital(e, p)).collect();
    let n = facts.len();
    let mut out: Vec<ArrayRef> = Vec::with_capacity(TEXT.len() + 1 + PARTS.len());
    for (_, get) in TEXT {
        let mut b = StringBuilder::with_capacity(n, 0);
        for f in &facts {
            b.append_option(f.as_ref().and_then(get));
        }
        out.push(Arc::new(b.finish()));
    }
    let number = |get: &dyn Fn(&Vital) -> Option<f64>| -> ArrayRef {
        let mut b = Float64Builder::with_capacity(n);
        for f in &facts {
            b.append_option(f.as_ref().and_then(get));
        }
        Arc::new(b.finish())
    };
    out.push(number(&|v| v.value));
    for i in 0..PARTS.len() {
        out.push(number(&|v| v.parts.get(i).copied().flatten()));
    }
    out
}

/// `batch` with exactly the columns of [`event_schema`], matched by name:
/// a segment written before the typed columns gets them computed from its
/// `event` and `properties` columns; any other missing nullable column is
/// NULL.
pub(crate) fn conform(batch: &RecordBatch) -> Result<RecordBatch> {
    let schema = event_schema();
    if batch.schema().fields().len() == schema.fields().len()
        && batch
            .schema()
            .fields()
            .iter()
            .zip(schema.fields())
            .all(|(a, b)| a.name() == b.name())
    {
        return Ok(RecordBatch::try_new(schema, batch.columns().to_vec())?);
    }
    let text = |name: &str| {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
    };
    let typed = match (text("event"), text("properties")) {
        (Some(e), Some(p)) => {
            Some(columns((0..batch.num_rows()).map(|i| {
                (e.value(i), if p.is_null(i) { "" } else { p.value(i) })
            })))
        }
        _ => None,
    };
    let names = names();
    let mut cols = Vec::with_capacity(schema.fields().len());
    for field in schema.fields() {
        let col = match batch.column_by_name(field.name()) {
            Some(c) => c.clone(),
            None => {
                let derived = names
                    .iter()
                    .position(|n| n == field.name())
                    .and_then(|i| typed.as_ref().and_then(|t| t.get(i)).cloned());
                derived.unwrap_or_else(|| {
                    arrow::array::new_null_array(field.data_type(), batch.num_rows())
                })
            }
        };
        cols.push(col);
    }
    Ok(RecordBatch::try_new(schema, cols)?)
}
