//! Vectorized batch processing for columnar query execution
//!
//! Implements DuckDB-inspired selection vector pattern with late materialization:
//! 1. Evaluate predicates on entire Arrow arrays using compute kernels
//! 2. Combine selection bitmaps with AND/OR
//! 3. Materialize only selected rows (late materialization)

use anyhow::Result;
use arrow::array::{
    BooleanArray, Float32Array, Float64Array, Int32Array, Int64Array, Scalar, StringArray,
};
use arrow::compute;
use arrow::compute::kernels::cmp;
use arrow::record_batch::RecordBatch;
use tracing::trace;

use crate::storage::engines::core::formats::columnar::FilterCondition;

/// Fixed-size data chunk for vectorized processing
pub struct DataChunk {
    /// Underlying Arrow data
    batch: RecordBatch,
    /// Selection bitmap (None = all selected)
    selection: Option<BooleanArray>,
    /// Active row count after filtering
    active_count: usize,
}

impl DataChunk {
    /// Create a new DataChunk from a RecordBatch (all rows selected)
    pub fn new(batch: RecordBatch) -> Self {
        let active_count = batch.num_rows();
        Self {
            batch,
            selection: None,
            active_count,
        }
    }

    /// Get the number of active (selected) rows
    pub fn active_count(&self) -> usize {
        self.active_count
    }

    /// Get the underlying batch
    pub fn batch(&self) -> &RecordBatch {
        &self.batch
    }

    /// Check if all rows have been filtered out
    pub fn is_empty(&self) -> bool {
        self.active_count == 0
    }

    /// Apply a selection mask (AND with existing selection)
    pub fn apply_selection(&mut self, mask: &BooleanArray) {
        self.selection = match &self.selection {
            None => Some(mask.clone()),
            Some(existing) => {
                match compute::and(existing, mask) {
                    Ok(combined) => Some(combined),
                    Err(_) => Some(mask.clone()), // fallback
                }
            }
        };
        self.active_count = self
            .selection
            .as_ref()
            .map_or(self.batch.num_rows(), |s| s.true_count());
    }

    /// Materialize only selected rows (late materialization)
    pub fn materialize(&self) -> Result<RecordBatch> {
        match &self.selection {
            None => Ok(self.batch.clone()),
            Some(selection) => compute::filter_record_batch(&self.batch, selection)
                .map_err(|e| anyhow::anyhow!("Failed to materialize filtered batch: {}", e)),
        }
    }
}

/// Evaluate a filter condition against an entire Arrow array, returning a boolean selection mask
pub fn evaluate_predicate_vectorized(
    batch: &RecordBatch,
    condition: &FilterCondition,
) -> Result<BooleanArray> {
    match condition {
        FilterCondition::Equals(col_name, value) => evaluate_equals(batch, col_name, value),
        FilterCondition::Range(col_name, min_val, max_val) => {
            evaluate_range(batch, col_name, min_val, max_val)
        }
        FilterCondition::In(col_name, values) => evaluate_in(batch, col_name, values),
        FilterCondition::IsNull(col_name) => evaluate_is_null(batch, col_name),
        FilterCondition::IsNotNull(col_name) => evaluate_is_not_null(batch, col_name),
    }
}

/// Create a boolean array of all-true values for pass-through
fn all_true(num_rows: usize) -> BooleanArray {
    BooleanArray::from(vec![true; num_rows])
}

/// Create a boolean array of all-false values
fn all_false(num_rows: usize) -> BooleanArray {
    BooleanArray::from(vec![false; num_rows])
}

/// Evaluate an Equals filter condition against an Arrow batch column
fn evaluate_equals(
    batch: &RecordBatch,
    col_name: &str,
    value: &serde_json::Value,
) -> Result<BooleanArray> {
    let col = match batch.column_by_name(col_name) {
        Some(c) => c,
        None => return Ok(all_true(batch.num_rows())), // no column = no filter
    };

    match col.data_type() {
        arrow::datatypes::DataType::Utf8 => {
            if let Some(s) = value.as_str() {
                let array = col
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to StringArray"))?;
                let scalar = Scalar::new(StringArray::from(vec![s]));
                let result = cmp::eq(array, &scalar)
                    .map_err(|e| anyhow::anyhow!("Utf8 eq failed: {}", e))?;
                Ok(result)
            } else {
                Ok(all_true(batch.num_rows()))
            }
        }
        arrow::datatypes::DataType::Int32 => {
            if let Some(n) = value.as_i64() {
                let array = col
                    .as_any()
                    .downcast_ref::<Int32Array>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Int32Array"))?;
                let scalar = Scalar::new(Int32Array::from(vec![n as i32]));
                let result = cmp::eq(array, &scalar)
                    .map_err(|e| anyhow::anyhow!("Int32 eq failed: {}", e))?;
                Ok(result)
            } else {
                Ok(all_true(batch.num_rows()))
            }
        }
        arrow::datatypes::DataType::Int64 => {
            if let Some(n) = value.as_i64() {
                let array = col
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Int64Array"))?;
                let scalar = Scalar::new(Int64Array::from(vec![n]));
                let result = cmp::eq(array, &scalar)
                    .map_err(|e| anyhow::anyhow!("Int64 eq failed: {}", e))?;
                Ok(result)
            } else {
                Ok(all_true(batch.num_rows()))
            }
        }
        arrow::datatypes::DataType::Float32 => {
            if let Some(n) = value.as_f64() {
                let array = col
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Float32Array"))?;
                let scalar = Scalar::new(Float32Array::from(vec![n as f32]));
                let result = cmp::eq(array, &scalar)
                    .map_err(|e| anyhow::anyhow!("Float32 eq failed: {}", e))?;
                Ok(result)
            } else {
                Ok(all_true(batch.num_rows()))
            }
        }
        arrow::datatypes::DataType::Float64 => {
            if let Some(n) = value.as_f64() {
                let array = col
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Float64Array"))?;
                let scalar = Scalar::new(Float64Array::from(vec![n]));
                let result = cmp::eq(array, &scalar)
                    .map_err(|e| anyhow::anyhow!("Float64 eq failed: {}", e))?;
                Ok(result)
            } else {
                Ok(all_true(batch.num_rows()))
            }
        }
        arrow::datatypes::DataType::Boolean => {
            if let Some(b) = value.as_bool() {
                let array = col
                    .as_any()
                    .downcast_ref::<BooleanArray>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to BooleanArray"))?;
                let scalar = Scalar::new(BooleanArray::from(vec![b]));
                let result = cmp::eq(array, &scalar)
                    .map_err(|e| anyhow::anyhow!("Boolean eq failed: {}", e))?;
                Ok(result)
            } else {
                Ok(all_true(batch.num_rows()))
            }
        }
        _ => Ok(all_true(batch.num_rows())), // unsupported type = pass through
    }
}

/// Evaluate a Range filter condition against an Arrow batch column
fn evaluate_range(
    batch: &RecordBatch,
    col_name: &str,
    min_val: &serde_json::Value,
    max_val: &serde_json::Value,
) -> Result<BooleanArray> {
    let col = match batch.column_by_name(col_name) {
        Some(c) => c,
        None => return Ok(all_true(batch.num_rows())),
    };

    let min_f64 = min_val.as_f64();
    let max_f64 = max_val.as_f64();

    match col.data_type() {
        arrow::datatypes::DataType::Int32 => {
            let array = col
                .as_any()
                .downcast_ref::<Int32Array>()
                .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Int32Array"))?;
            let ge_min = if let Some(min) = min_f64 {
                let scalar = Scalar::new(Int32Array::from(vec![min as i32]));
                cmp::gt_eq(array, &scalar)
                    .map_err(|e| anyhow::anyhow!("Int32 gt_eq failed: {}", e))?
            } else {
                all_true(batch.num_rows())
            };
            let le_max = if let Some(max) = max_f64 {
                let scalar = Scalar::new(Int32Array::from(vec![max as i32]));
                cmp::lt_eq(array, &scalar)
                    .map_err(|e| anyhow::anyhow!("Int32 lt_eq failed: {}", e))?
            } else {
                all_true(batch.num_rows())
            };
            compute::and(&ge_min, &le_max).map_err(|e| anyhow::anyhow!("Range AND failed: {}", e))
        }
        arrow::datatypes::DataType::Int64 => {
            let array = col
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Int64Array"))?;
            let ge_min = if let Some(min) = min_f64 {
                let scalar = Scalar::new(Int64Array::from(vec![min as i64]));
                cmp::gt_eq(array, &scalar)
                    .map_err(|e| anyhow::anyhow!("Int64 gt_eq failed: {}", e))?
            } else {
                all_true(batch.num_rows())
            };
            let le_max = if let Some(max) = max_f64 {
                let scalar = Scalar::new(Int64Array::from(vec![max as i64]));
                cmp::lt_eq(array, &scalar)
                    .map_err(|e| anyhow::anyhow!("Int64 lt_eq failed: {}", e))?
            } else {
                all_true(batch.num_rows())
            };
            compute::and(&ge_min, &le_max).map_err(|e| anyhow::anyhow!("Range AND failed: {}", e))
        }
        arrow::datatypes::DataType::Float32 => {
            let array = col
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Float32Array"))?;
            let ge_min = if let Some(min) = min_f64 {
                let scalar = Scalar::new(Float32Array::from(vec![min as f32]));
                cmp::gt_eq(array, &scalar)
                    .map_err(|e| anyhow::anyhow!("Float32 gt_eq failed: {}", e))?
            } else {
                all_true(batch.num_rows())
            };
            let le_max = if let Some(max) = max_f64 {
                let scalar = Scalar::new(Float32Array::from(vec![max as f32]));
                cmp::lt_eq(array, &scalar)
                    .map_err(|e| anyhow::anyhow!("Float32 lt_eq failed: {}", e))?
            } else {
                all_true(batch.num_rows())
            };
            compute::and(&ge_min, &le_max).map_err(|e| anyhow::anyhow!("Range AND failed: {}", e))
        }
        arrow::datatypes::DataType::Float64 => {
            let array = col
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Float64Array"))?;
            let ge_min = if let Some(min) = min_f64 {
                let scalar = Scalar::new(Float64Array::from(vec![min]));
                cmp::gt_eq(array, &scalar)
                    .map_err(|e| anyhow::anyhow!("Float64 gt_eq failed: {}", e))?
            } else {
                all_true(batch.num_rows())
            };
            let le_max = if let Some(max) = max_f64 {
                let scalar = Scalar::new(Float64Array::from(vec![max]));
                cmp::lt_eq(array, &scalar)
                    .map_err(|e| anyhow::anyhow!("Float64 lt_eq failed: {}", e))?
            } else {
                all_true(batch.num_rows())
            };
            compute::and(&ge_min, &le_max).map_err(|e| anyhow::anyhow!("Range AND failed: {}", e))
        }
        _ => Ok(all_true(batch.num_rows())),
    }
}

/// Evaluate an In filter condition by OR-ing equality checks for each value
fn evaluate_in(
    batch: &RecordBatch,
    col_name: &str,
    values: &[serde_json::Value],
) -> Result<BooleanArray> {
    if batch.column_by_name(col_name).is_none() {
        return Ok(all_true(batch.num_rows()));
    }

    // Evaluate equals for each value, OR them together
    let mut result = all_false(batch.num_rows());
    for value in values {
        let eq_mask = evaluate_equals(batch, col_name, value)?;
        result =
            compute::or(&result, &eq_mask).map_err(|e| anyhow::anyhow!("IN OR failed: {}", e))?;
    }
    Ok(result)
}

/// Evaluate an IsNull filter condition
fn evaluate_is_null(batch: &RecordBatch, col_name: &str) -> Result<BooleanArray> {
    let col = match batch.column_by_name(col_name) {
        Some(c) => c,
        None => return Ok(all_true(batch.num_rows())),
    };
    compute::is_null(col.as_ref()).map_err(|e| anyhow::anyhow!("is_null failed: {}", e))
}

/// Evaluate an IsNotNull filter condition
fn evaluate_is_not_null(batch: &RecordBatch, col_name: &str) -> Result<BooleanArray> {
    let col = match batch.column_by_name(col_name) {
        Some(c) => c,
        None => return Ok(all_true(batch.num_rows())),
    };
    compute::is_not_null(col.as_ref()).map_err(|e| anyhow::anyhow!("is_not_null failed: {}", e))
}

/// Evaluate a single comparison operator against a column, returning a selection mask.
///
/// Unlike the flat [`FilterCondition`] path, this handles every
/// [`ComparisonOperator`] so the full [`FilterExpression`] tree (including OR/NOT
/// and boolean equality) can be evaluated vectorized without losing structure.
fn evaluate_comparison_mask(
    batch: &RecordBatch,
    field: &str,
    operator: &crate::core::search::ComparisonOperator,
    value: &serde_json::Value,
) -> Result<BooleanArray> {
    use crate::core::search::ComparisonOperator;

    match operator {
        ComparisonOperator::Equals => evaluate_equals(batch, field, value),
        ComparisonOperator::NotEquals => {
            let eq = evaluate_equals(batch, field, value)?;
            compute::not(&eq).map_err(|e| anyhow::anyhow!("NotEquals negation failed: {}", e))
        }
        ComparisonOperator::GreaterThan => {
            // (value, +inf): exclusive lower bound via a Range then drop equality.
            // Range is inclusive on both ends, so emulate strict > as Range(value,MAX) AND != value.
            let ge = evaluate_range(batch, field, value, &serde_json::json!(f64::MAX))?;
            let eq = evaluate_equals(batch, field, value)?;
            let ne = compute::not(&eq).map_err(|e| anyhow::anyhow!("gt negation failed: {}", e))?;
            compute::and(&ge, &ne).map_err(|e| anyhow::anyhow!("gt AND failed: {}", e))
        }
        ComparisonOperator::GreaterThanOrEqual => {
            evaluate_range(batch, field, value, &serde_json::json!(f64::MAX))
        }
        ComparisonOperator::LessThan => {
            let le = evaluate_range(batch, field, &serde_json::json!(f64::MIN), value)?;
            let eq = evaluate_equals(batch, field, value)?;
            let ne = compute::not(&eq).map_err(|e| anyhow::anyhow!("lt negation failed: {}", e))?;
            compute::and(&le, &ne).map_err(|e| anyhow::anyhow!("lt AND failed: {}", e))
        }
        ComparisonOperator::LessThanOrEqual => {
            evaluate_range(batch, field, &serde_json::json!(f64::MIN), value)
        }
        ComparisonOperator::In => {
            let values: Vec<serde_json::Value> = value
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![value.clone()]);
            evaluate_in(batch, field, &values)
        }
        ComparisonOperator::NotIn => {
            let values: Vec<serde_json::Value> = value
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![value.clone()]);
            let in_mask = evaluate_in(batch, field, &values)?;
            compute::not(&in_mask).map_err(|e| anyhow::anyhow!("NotIn negation failed: {}", e))
        }
        ComparisonOperator::IsNull => evaluate_is_null(batch, field),
        ComparisonOperator::IsNotNull => evaluate_is_not_null(batch, field),
        // Operators without a vectorized kernel (string Contains/StartsWith/EndsWith/Like/Between)
        // signal "can't vectorize" so the caller falls back to row-at-a-time evaluation.
        _ => Err(anyhow::anyhow!(
            "Operator {:?} has no vectorized kernel",
            operator
        )),
    }
}

/// Recursively evaluate a [`FilterExpression`] tree into a boolean selection mask,
/// preserving AND / OR / NOT structure.
///
/// Returns an error for any sub-expression that has no vectorized kernel so the
/// caller can fall back to the (correct, structure-preserving) row-at-a-time path.
fn evaluate_filter_expression_mask(
    batch: &RecordBatch,
    expr: &crate::core::search::FilterExpression,
) -> Result<BooleanArray> {
    use crate::core::search::FilterExpression;

    match expr {
        FilterExpression::Comparison {
            field,
            operator,
            value,
        } => evaluate_comparison_mask(batch, field, operator, value),
        FilterExpression::And(exprs) => {
            let mut acc = all_true(batch.num_rows());
            for e in exprs {
                let mask = evaluate_filter_expression_mask(batch, e)?;
                acc = compute::and(&acc, &mask)
                    .map_err(|err| anyhow::anyhow!("AND combine failed: {}", err))?;
            }
            Ok(acc)
        }
        FilterExpression::Or(exprs) => {
            let mut acc = all_false(batch.num_rows());
            for e in exprs {
                let mask = evaluate_filter_expression_mask(batch, e)?;
                acc = compute::or(&acc, &mask)
                    .map_err(|err| anyhow::anyhow!("OR combine failed: {}", err))?;
            }
            Ok(acc)
        }
        FilterExpression::Not(inner) => {
            let mask = evaluate_filter_expression_mask(batch, inner)?;
            compute::not(&mask).map_err(|err| anyhow::anyhow!("NOT negation failed: {}", err))
        }
    }
}

/// Process a RecordBatch through vectorized evaluation of a full [`FilterExpression`]
/// tree (AND/OR/NOT, boolean equality, and all numeric/string comparison operators
/// that have a vectorized kernel).
///
/// Returns `Ok(Some(batch))` with only matching rows when the entire expression could
/// be vectorized, or `Ok(None)` when any sub-expression lacks a vectorized kernel — in
/// which case the caller must fall back to row-at-a-time evaluation to stay correct.
pub fn vectorized_filter_batch_expr(
    batch: RecordBatch,
    expr: &crate::core::search::FilterExpression,
) -> Result<Option<RecordBatch>> {
    let mask = match evaluate_filter_expression_mask(&batch, expr) {
        Ok(mask) => mask,
        Err(_) => return Ok(None), // no vectorized kernel for some sub-expression
    };

    let filtered = compute::filter_record_batch(&batch, &mask)
        .map_err(|e| anyhow::anyhow!("Failed to materialize filtered batch: {}", e))?;

    trace!(
        "Vectorized expr filter: {} -> {} rows",
        batch.num_rows(),
        filtered.num_rows()
    );

    Ok(Some(filtered))
}

/// Process a RecordBatch through vectorized filter evaluation.
/// Returns the filtered RecordBatch with only matching rows.
pub fn vectorized_filter_batch(
    batch: RecordBatch,
    conditions: &[FilterCondition],
) -> Result<RecordBatch> {
    if conditions.is_empty() {
        return Ok(batch);
    }

    let mut chunk = DataChunk::new(batch);

    for condition in conditions {
        if chunk.is_empty() {
            break; // early exit if all rows eliminated
        }
        let mask = evaluate_predicate_vectorized(chunk.batch(), condition)?;
        chunk.apply_selection(&mask);
    }

    trace!(
        "Vectorized filter: {} -> {} rows",
        chunk.batch().num_rows(),
        chunk.active_count()
    );

    chunk.materialize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Float64Array, Int32Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    fn make_test_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("score", DataType::Float64, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5])),
                Arc::new(StringArray::from(vec![
                    "alice", "bob", "carol", "dave", "eve",
                ])),
                Arc::new(Float64Array::from(vec![0.5, 0.8, 0.3, 0.9, 0.1])),
            ],
        )
        .unwrap()
    }

    #[test]
    fn test_vectorized_equals_string() {
        let batch = make_test_batch();
        let condition = FilterCondition::Equals("name".to_string(), serde_json::json!("bob"));
        let result = vectorized_filter_batch(batch, &[condition]).unwrap();
        assert_eq!(result.num_rows(), 1);
        let names = result
            .column_by_name("name")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(names.value(0), "bob");
    }

    #[test]
    fn test_vectorized_equals_numeric() {
        let batch = make_test_batch();
        let condition = FilterCondition::Equals("id".to_string(), serde_json::json!(3));
        let result = vectorized_filter_batch(batch, &[condition]).unwrap();
        assert_eq!(result.num_rows(), 1);
        let ids = result
            .column_by_name("id")
            .unwrap()
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap();
        assert_eq!(ids.value(0), 3);
    }

    #[test]
    fn test_vectorized_range() {
        let batch = make_test_batch();
        let condition = FilterCondition::Range(
            "score".to_string(),
            serde_json::json!(0.3),
            serde_json::json!(0.8),
        );
        let result = vectorized_filter_batch(batch, &[condition]).unwrap();
        // Rows with score 0.3, 0.5, 0.8 match (inclusive range)
        assert_eq!(result.num_rows(), 3);
    }

    #[test]
    fn test_vectorized_in() {
        let batch = make_test_batch();
        let condition = FilterCondition::In(
            "name".to_string(),
            vec![serde_json::json!("alice"), serde_json::json!("eve")],
        );
        let result = vectorized_filter_batch(batch, &[condition]).unwrap();
        assert_eq!(result.num_rows(), 2);
    }

    #[test]
    fn test_vectorized_is_null() {
        // Create batch with nullable column
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("value", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec![Some("a"), None, Some("c")])),
            ],
        )
        .unwrap();

        let condition = FilterCondition::IsNull("value".to_string());
        let result = vectorized_filter_batch(batch, &[condition]).unwrap();
        assert_eq!(result.num_rows(), 1);
        let ids = result
            .column_by_name("id")
            .unwrap()
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap();
        assert_eq!(ids.value(0), 2);
    }

    #[test]
    fn test_data_chunk_materialize() {
        let batch = make_test_batch();
        let mut chunk = DataChunk::new(batch);
        assert_eq!(chunk.active_count(), 5);
        assert!(!chunk.is_empty());

        // Apply a mask that selects rows 0, 2, 4
        let mask = BooleanArray::from(vec![true, false, true, false, true]);
        chunk.apply_selection(&mask);
        assert_eq!(chunk.active_count(), 3);

        let materialized = chunk.materialize().unwrap();
        assert_eq!(materialized.num_rows(), 3);
        let ids = materialized
            .column_by_name("id")
            .unwrap()
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap();
        assert_eq!(ids.value(0), 1);
        assert_eq!(ids.value(1), 3);
        assert_eq!(ids.value(2), 5);
    }

    #[test]
    fn test_vectorized_filter_batch_multiple_conditions() {
        let batch = make_test_batch();
        let conditions = vec![
            FilterCondition::Range(
                "score".to_string(),
                serde_json::json!(0.1),
                serde_json::json!(0.8),
            ),
            FilterCondition::IsNotNull("name".to_string()),
        ];
        let result = vectorized_filter_batch(batch, &conditions).unwrap();
        // All rows have non-null names, score range [0.1, 0.8] matches: 0.5, 0.8, 0.3, 0.1
        assert_eq!(result.num_rows(), 4);
    }

    #[test]
    fn test_empty_conditions_returns_all() {
        let batch = make_test_batch();
        let result = vectorized_filter_batch(batch, &[]).unwrap();
        assert_eq!(result.num_rows(), 5);
    }

    #[test]
    fn test_missing_column_passes_through() {
        let batch = make_test_batch();
        let condition =
            FilterCondition::Equals("nonexistent".to_string(), serde_json::json!("foo"));
        let result = vectorized_filter_batch(batch, &[condition]).unwrap();
        // Missing column = no filter applied = all rows pass
        assert_eq!(result.num_rows(), 5);
    }

    #[test]
    fn test_early_exit_on_empty() {
        let batch = make_test_batch();
        // First condition eliminates all rows, second should be skipped (early exit)
        let conditions = vec![
            FilterCondition::Equals("id".to_string(), serde_json::json!(999)),
            FilterCondition::Equals("name".to_string(), serde_json::json!("alice")),
        ];
        let result = vectorized_filter_batch(batch, &conditions).unwrap();
        assert_eq!(result.num_rows(), 0);
    }
}
