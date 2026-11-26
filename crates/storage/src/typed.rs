//! Typed columns derived from `properties` at write time: the Web Vitals
//! facts of `$vital` events (#449, D-037), and the error, acquisition and
//! friction facts (#482, D-039), so readers stop parsing the JSON text
//! with a regular expression per row.
//!
//! They are added at the end of [`crate::event_schema`] and are nullable.
//! Files written before them have no such columns: the query layer reads
//! them as NULL and falls back to the text, and every rewrite (compaction,
//! erasure, click trimming) fills them in from `properties` on the way.

use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, BooleanBuilder, Float64Builder, Int32Builder, RecordBatch, StringArray,
    StringBuilder,
};
use arrow::datatypes::{DataType, Field};
use openlens_event::{Facts, facts::VERSION, vital::PARTS};

use crate::{Result, event_schema};

/// Reads one text fact.
type Getter = fn(&Facts) -> Option<&str>;

/// The Web Vitals text columns (#449), in schema order.
const VITAL_TEXT: [(&str, Getter); 5] = [
    ("vital_name", |f| f.vital.as_ref().map(|v| v.name.as_str())),
    ("vital_device", |f| {
        f.vital.as_ref().and_then(|v| v.device.as_deref())
    }),
    ("vital_el", |f| {
        f.vital.as_ref().and_then(|v| v.el.as_deref())
    }),
    ("vital_kind", |f| {
        f.vital.as_ref().and_then(|v| v.kind.as_deref())
    }),
    ("vital_res", |f| {
        f.vital.as_ref().and_then(|v| v.res.as_deref())
    }),
];

/// The text columns of #482, in schema order, after the Web Vitals ones.
const FACT_TEXT: [(&str, Getter); 8] = [
    ("vital_rating", |f| f.rating.as_deref()),
    ("error_message", |f| f.message.as_deref()),
    ("error_source", |f| f.source.as_deref()),
    ("error_stack", |f| f.stack.as_deref()),
    ("acq_channel", |f| f.acquisition[0].as_deref()),
    ("acq_utm_source", |f| f.acquisition[1].as_deref()),
    ("acq_utm_campaign", |f| f.acquisition[2].as_deref()),
    ("acq_ref_host", |f| f.acquisition[3].as_deref()),
];

/// Whether a `$click` was a rage click (#482).
const RAGE: &str = "click_rage";

/// The version of the facts, on every row written since #482.
const VERSION_COLUMN: &str = "facts_version";

/// The typed columns' names, in schema order.
pub fn names() -> Vec<String> {
    fields().into_iter().map(|f| f.name().clone()).collect()
}

/// The fields appended to the event schema.
pub(crate) fn fields() -> Vec<Field> {
    let text = |n: &str| Field::new(n, DataType::Utf8, true);
    let number = |n: &str| Field::new(n, DataType::Float64, true);
    let mut v: Vec<Field> = VITAL_TEXT.iter().map(|(n, _)| text(n)).collect();
    v.push(number("vital_value"));
    v.extend(PARTS.iter().map(|p| number(&format!("vital_{p}"))));
    v.extend(FACT_TEXT.iter().map(|(n, _)| text(n)));
    v.push(Field::new(RAGE, DataType::Boolean, true));
    v.push(Field::new(VERSION_COLUMN, DataType::Int32, true));
    v
}

/// The typed columns, in schema order, for rows given as
/// `(event, properties)`.
pub(crate) fn columns<'a>(
    rows: impl ExactSizeIterator<Item = (&'a str, &'a str)>,
) -> Vec<ArrayRef> {
    let facts: Vec<Facts> = rows.map(|(e, p)| openlens_event::facts(e, p)).collect();
    let n = facts.len();
    let mut out: Vec<ArrayRef> =
        Vec::with_capacity(VITAL_TEXT.len() + 1 + PARTS.len() + FACT_TEXT.len() + 2);
    let text = |get: Getter| -> ArrayRef {
        let mut b = StringBuilder::with_capacity(n, 0);
        for f in &facts {
            b.append_option(get(f));
        }
        Arc::new(b.finish())
    };
    let number = |get: &dyn Fn(&Facts) -> Option<f64>| -> ArrayRef {
        let mut b = Float64Builder::with_capacity(n);
        for f in &facts {
            b.append_option(get(f));
        }
        Arc::new(b.finish())
    };
    out.extend(VITAL_TEXT.iter().map(|(_, get)| text(*get)));
    out.push(number(&|f| f.vital.as_ref().and_then(|v| v.value)));
    for i in 0..PARTS.len() {
        out.push(number(&|f| {
            f.vital
                .as_ref()
                .and_then(|v| v.parts.get(i).copied().flatten())
        }));
    }
    out.extend(FACT_TEXT.iter().map(|(_, get)| text(*get)));
    let mut rage = BooleanBuilder::with_capacity(n);
    let mut version = Int32Builder::with_capacity(n);
    for f in &facts {
        rage.append_option(f.rage);
        version.append_value(VERSION);
    }
    out.push(Arc::new(rage.finish()));
    out.push(Arc::new(version.finish()));
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
