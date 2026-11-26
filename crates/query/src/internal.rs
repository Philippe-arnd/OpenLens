//! Whether an id is one of the team's own people, without a join (#444).
//!
//! Board filters left out internal people with `distinct_id NOT IN (SELECT
//! person FROM persons WHERE internal)`. DataFusion planned it as an anti
//! join that built its hash table on the events, then emitted every kept
//! event at once, as one batch: at 1 M events the board snapshot was a
//! single 966,000-row batch, every board query ran on one thread over
//! million-row arrays, and the board took three times longer and twice the
//! memory once the identity join (#445) stopped hiding it. The internal
//! people are already in memory (`persons`, a few rows), so each id is
//! looked up instead.

use std::collections::HashSet;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, BooleanBuilder, StringArray};
use arrow::datatypes::DataType;
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{ColumnarValue, ScalarUDF, Volatility, create_udf};

/// The SQL name of the test.
pub(crate) const IS_INTERNAL: &str = "is_internal";

/// Ids marked internal.
pub(crate) type Internal = Arc<HashSet<String>>;

/// `is_internal(id)`: true for an internal person, false otherwise (NULL
/// for a NULL id, as `NOT IN` would leave it).
pub(crate) fn is_internal_udf(internal: Internal) -> ScalarUDF {
    create_udf(
        IS_INTERNAL,
        vec![DataType::Utf8],
        DataType::Boolean,
        Volatility::Immutable,
        Arc::new(move |args: &[ColumnarValue]| {
            let arg = args
                .first()
                .ok_or_else(|| DataFusionError::Plan("one argument expected".into()))?;
            let arr = match arg {
                ColumnarValue::Array(a) => a.clone(),
                ColumnarValue::Scalar(s) => s.to_array()?,
            };
            let arr = arrow::compute::cast(&arr, &DataType::Utf8)?;
            let ids = arr
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| DataFusionError::Plan("text expected".into()))?;
            let mut out = BooleanBuilder::with_capacity(ids.len());
            for v in ids.iter() {
                out.append_option(v.map(|v| !internal.is_empty() && internal.contains(v)));
            }
            Ok(ColumnarValue::Array(Arc::new(out.finish()) as ArrayRef))
        }),
    )
}
