//! SQL functions written in Rust by higher layers (#446).
//!
//! The engine turns a feature map into functions (`feature_of_<hash>`,
//! `role_of_<hash>`) instead of a `CASE` evaluated with regular
//! expressions on every row. The name carries a hash of the rules, so one
//! name always means the same rules, and the SQL text that uses it can be
//! built anywhere without a handle on the engine. Every query context
//! registers the functions defined so far.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use arrow::array::{Array, ArrayRef, StringArray, StringBuilder};
use arrow::datatypes::DataType;
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{ColumnarValue, ScalarUDF, Volatility, create_udf};
use datafusion::prelude::SessionContext;

/// A function of text arguments: writes its result into the buffer and
/// returns `true`, or returns `false` for NULL.
pub type TextFn = Arc<dyn Fn(&[Option<&str>], &mut String) -> bool + Send + Sync>;

/// Functions kept at once. Each map version in use defines two; beyond
/// this, the oldest go (a plan that already holds one keeps it).
const KEEP: usize = 64;

static DEFINED: Mutex<VecDeque<(String, ScalarUDF)>> = Mutex::new(VecDeque::new());

/// Whether `name` is defined.
pub fn is_defined(name: &str) -> bool {
    DEFINED
        .lock()
        .is_ok_and(|d| d.iter().any(|(n, _)| n == name))
}

/// Define `name(arg1, …)` over `arity` text arguments, for every query
/// context made from now on. Defining a name again keeps the first one.
pub fn define(name: &str, arity: usize, f: TextFn) {
    let Ok(mut d) = DEFINED.lock() else {
        return;
    };
    if d.iter().any(|(n, _)| n == name) {
        return;
    }
    if d.len() >= KEEP {
        d.pop_front();
    }
    d.push_back((name.to_owned(), udf(name, arity, f)));
}

/// Add every defined function to `ctx`.
pub(crate) fn register(ctx: &SessionContext) {
    let Ok(d) = DEFINED.lock() else {
        return;
    };
    for (_, f) in d.iter() {
        ctx.register_udf(f.clone());
    }
}

fn udf(name: &str, arity: usize, f: TextFn) -> ScalarUDF {
    create_udf(
        name,
        vec![DataType::Utf8; arity],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(move |args: &[ColumnarValue]| {
            if args.len() != arity {
                return Err(DataFusionError::Plan(format!("{arity} arguments expected")));
            }
            let rows = args
                .iter()
                .find_map(|a| match a {
                    ColumnarValue::Array(a) => Some(a.len()),
                    ColumnarValue::Scalar(_) => None,
                })
                .unwrap_or(1);
            let mut cols: Vec<ArrayRef> = Vec::with_capacity(arity);
            for a in args {
                let a = match a {
                    ColumnarValue::Array(a) => a.clone(),
                    ColumnarValue::Scalar(s) => s.to_array_of_size(rows)?,
                };
                cols.push(arrow::compute::cast(&a, &DataType::Utf8)?);
            }
            let cols: Vec<&StringArray> = cols
                .iter()
                .map(|c| {
                    c.as_any()
                        .downcast_ref::<StringArray>()
                        .ok_or_else(|| DataFusionError::Plan("text expected".into()))
                })
                .collect::<Result<_, _>>()?;
            let mut out = StringBuilder::with_capacity(rows, rows * 8);
            let mut buf = String::new();
            let mut row: Vec<Option<&str>> = vec![None; arity];
            for i in 0..rows {
                for (slot, c) in row.iter_mut().zip(&cols) {
                    *slot = (!c.is_null(i)).then(|| c.value(i));
                }
                buf.clear();
                if f(&row, &mut buf) {
                    out.append_value(&buf);
                } else {
                    out.append_null();
                }
            }
            Ok(ColumnarValue::Array(Arc::new(out.finish()) as ArrayRef))
        }),
    )
}
