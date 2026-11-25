//! The identity lookups of the query layer (#445): the alias table read
//! into memory, and the `person_of` function that applies it.

use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{RecordBatch, StringArray, cast::AsArray};
use arrow::datatypes::DataType;
use datafusion::common::DataFusionError;
use datafusion::logical_expr::{ColumnarValue, ScalarUDF, Volatility, create_udf};

/// The scalar function `person_of(distinct_id)`: the user an anonymous id
/// was identified as, or the id itself when it is not an alias.
pub(crate) fn person_of_udf(lookup: Arc<HashMap<String, String>>) -> ScalarUDF {
    create_udf(
        "person_of",
        vec![DataType::Utf8],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(move |args: &[ColumnarValue]| {
            let arrays = ColumnarValue::values_to_arrays(args)?;
            let ids = arrays
                .first()
                .map(|a| a.as_string::<i32>())
                .ok_or_else(|| DataFusionError::Internal("person_of takes one argument".into()))?;
            let out: StringArray = ids
                .iter()
                .map(|id| id.map(|id| lookup.get(id).map_or(id, String::as_str)))
                .collect();
            Ok(ColumnarValue::Array(Arc::new(out)))
        }),
    )
}

/// Read the `(anon, person)` rows of the alias table into a lookup.
pub(crate) fn alias_lookup(batches: &[RecordBatch]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for b in batches {
        let (Some(anon), Some(person)) = (b.column_by_name("anon"), b.column_by_name("person"))
        else {
            continue;
        };
        let (Some(anon), Some(person)) =
            (anon.as_string_opt::<i32>(), person.as_string_opt::<i32>())
        else {
            continue;
        };
        for (a, p) in anon.iter().zip(person.iter()) {
            if let (Some(a), Some(p)) = (a, p) {
                map.insert(a.to_owned(), p.to_owned());
            }
        }
    }
    map
}
