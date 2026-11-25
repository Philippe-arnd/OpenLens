//! A snapshot's batches, at a size that does not depend on the plan (#444).

use crate::QueryError;

/// `batches` as batches of about `size` rows: small ones merged, large
/// ones sliced (no copy). A snapshot's shape then does not depend on the
/// plan that made it: one 966,000-row batch from an anti join ran every
/// board query on one thread over million-row arrays (#444), and thousands
/// of tiny ones pay a per-batch cost on every query.
pub(crate) fn rebatch(
    schema: &arrow::datatypes::SchemaRef,
    batches: Vec<arrow::array::RecordBatch>,
    size: usize,
) -> Result<Vec<arrow::array::RecordBatch>, QueryError> {
    let size = size.max(1);
    let mut out = Vec::new();
    let mut pending = Vec::new();
    let mut pending_rows = 0;
    let flush = |pending: &mut Vec<arrow::array::RecordBatch>,
                 out: &mut Vec<arrow::array::RecordBatch>|
     -> Result<(), QueryError> {
        match pending.len() {
            0 => {}
            1 => out.append(pending),
            _ => {
                out.push(
                    arrow::compute::concat_batches(schema, pending.iter())
                        .map_err(|e| QueryError::Storage(e.to_string()))?,
                );
                pending.clear();
            }
        }
        Ok(())
    };
    for b in batches {
        let n = b.num_rows();
        if n == 0 {
            continue;
        }
        if n >= size {
            flush(&mut pending, &mut out)?;
            pending_rows = 0;
            let mut at = 0;
            while at < n {
                let len = size.min(n - at);
                out.push(b.slice(at, len));
                at += len;
            }
            continue;
        }
        if pending_rows + n > size {
            flush(&mut pending, &mut out)?;
            pending_rows = 0;
        }
        pending_rows += n;
        pending.push(b);
    }
    flush(&mut pending, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use arrow::array::{Int32Array, RecordBatch};
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    #[test]
    fn snapshots_hold_batches_of_a_bounded_size() {
        let schema = Arc::new(Schema::new(vec![Field::new("n", DataType::Int32, false)]));
        let batch = |rows: i32| {
            RecordBatch::try_new(
                schema.clone(),
                vec![Arc::new(Int32Array::from_iter_values(0..rows))],
            )
            .unwrap()
        };
        let mut input = vec![batch(250)];
        input.extend((0..40).map(|_| batch(3)));
        let total: usize = input.iter().map(RecordBatch::num_rows).sum();
        let out = rebatch(&schema, input, 100).unwrap();
        assert_eq!(out.iter().map(RecordBatch::num_rows).sum::<usize>(), total);
        assert!(out.iter().all(|b| b.num_rows() <= 100 && b.num_rows() > 0));
        // A giant batch is cut, tiny ones are merged.
        assert!(out.len() > 2 && out.len() < 41);
    }
}
