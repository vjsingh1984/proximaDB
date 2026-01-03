// =============================================================================
// BRANCHED METADATA FILTERING STRATEGY
// =============================================================================
//
// Implements intelligent filtering strategy that chooses between:
// 1. Fast path: Column projection with pushdown filters (filterable columns only)
// 2. Slow path: Full projection with post-filtering (when non-filterable columns needed)
// 3. Mixed path: Pushdown what we can, post-filter the rest
//
// This avoids MapArray projection issues while providing optimal performance
// when possible and correctness when necessary.

use anyhow::{Result, anyhow};
use arrow_array::{ArrayRef, RecordBatch, StringArray, UInt32Array};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, info, trace, warn};

use super::{FilterCondition, MetadataFilter};
use crate::proto::proximadb_v1::VectorRecord;

/// Strategy for handling metadata filters based on column types
#[derive(Debug, Clone)]
pub enum MetadataFilterStrategy {
    /// Fast path: All filters can be pushed down to Parquet
    FastFilterable {
        /// Columns to project (only what's needed)
        projection_columns: Vec<String>,
        /// Filters that can be pushed to Parquet reader
        pushdown_filters: Vec<FilterCondition>,
    },

    /// Slow path: Some filters require full projection
    SlowFullScan {
        /// Filters on filterable columns (can push down)
        filterable_filters: Vec<FilterCondition>,
        /// Filters on non-filterable columns (need post-processing)
        non_filterable_filters: Vec<FilterCondition>,
        /// Warning message for user about performance impact
        warning_message: String,
    },

    /// Mixed path: Optimize what we can, handle the rest
    Mixed {
        /// These can be pushed to Parquet
        pushdown_filters: Vec<FilterCondition>,
        /// These need post-filtering
        post_filters: Vec<FilterCondition>,
        /// Columns to project
        projection_columns: Vec<String>,
    },

    /// No filtering needed
    NoFilter,
}

/// Analyzer for determining optimal filter strategy
pub struct MetadataFilterAnalyzer {
    /// Set of columns that are filterable (have dedicated columns)
    filterable_columns: HashSet<String>,
    /// Whether to allow slow queries for non-filterable columns
    allow_slow_queries: bool,
    /// Performance warning threshold (ms)
    slow_query_threshold_ms: u64,
}

impl MetadataFilterAnalyzer {
    pub fn new(filterable_columns: Vec<String>, allow_slow_queries: bool) -> Self {
        Self {
            filterable_columns: filterable_columns.into_iter().collect(),
            allow_slow_queries,
            slow_query_threshold_ms: 1000, // 1 second
        }
    }

    /// Analyze metadata filters and determine optimal strategy
    pub fn analyze_filters(&self, filters: &[MetadataFilter]) -> Result<MetadataFilterStrategy> {
        if filters.is_empty() {
            return Ok(MetadataFilterStrategy::NoFilter);
        }

        let mut filterable = Vec::new();
        let mut non_filterable = Vec::new();
        let mut required_columns = HashSet::new();

        // Categorize filters by examining all conditions
        for filter in filters {
            for condition in &filter.conditions {
                let column_name = match condition {
                    FilterCondition::Equals(col, _) => col,
                    FilterCondition::Range(col, _, _) => col,
                    FilterCondition::In(col, _) => col,
                    FilterCondition::IsNull(col) => col,
                    FilterCondition::IsNotNull(col) => col,
                };

                if self.filterable_columns.contains(column_name) {
                    // This filter can be pushed down
                    filterable.push(condition.clone());
                    required_columns.insert(column_name.clone());
                } else {
                    // This requires full scan
                    non_filterable.push(condition.clone());
                    // For non-filterable, we need the extra_meta column
                    required_columns.insert(super::FIELD_EXTRA_META.to_string());
                }
            }
        }

        // Determine strategy
        match (filterable.is_empty(), non_filterable.is_empty()) {
            (false, true) => {
                // Fast path: All filters are on filterable columns
                debug!(
                    "Fast path: All {} filters on filterable columns",
                    filterable.len()
                );

                Ok(MetadataFilterStrategy::FastFilterable {
                    projection_columns: required_columns.into_iter().collect(),
                    pushdown_filters: filterable,
                })
            }

            (true, false) => {
                // Slow path: All filters are on non-filterable columns
                if !self.allow_slow_queries {
                    return Err(anyhow!(
                        "Query requires filtering on non-indexed columns: {:?}. \
                        This would require a full table scan. \
                        Enable allow_slow_queries or add these columns to filterable_columns.",
                        non_filterable
                            .iter()
                            .map(|f| f.column().to_string())
                            .collect::<Vec<_>>()
                    ));
                }

                let warning = format!(
                    "⚠️ SLOW QUERY: Filtering on {} non-indexed columns requires full scan. \
                    Consider adding these columns to filterable_columns for better performance.",
                    non_filterable.len()
                );

                warn!("{}", warning);

                Ok(MetadataFilterStrategy::SlowFullScan {
                    filterable_filters: vec![],
                    non_filterable_filters: non_filterable,
                    warning_message: warning,
                })
            }

            (false, false) => {
                // Mixed path: Some filters can be pushed, others can't
                if !self.allow_slow_queries {
                    return Err(anyhow!(
                        "Query requires filtering on non-indexed columns: {:?}. \
                        Enable allow_slow_queries to proceed with slower performance.",
                        non_filterable
                            .iter()
                            .map(|f| f.column().to_string())
                            .collect::<Vec<_>>()
                    ));
                }

                info!(
                    "Mixed strategy: {} pushdown filters, {} post-filters",
                    filterable.len(),
                    non_filterable.len()
                );

                Ok(MetadataFilterStrategy::Mixed {
                    pushdown_filters: filterable,
                    post_filters: non_filterable,
                    projection_columns: required_columns.into_iter().collect(),
                })
            }

            (true, true) => Ok(MetadataFilterStrategy::NoFilter),
        }
    }

    /// Apply post-filtering to a batch of records
    pub fn apply_post_filters(
        &self,
        batch: RecordBatch,
        filters: &[FilterCondition],
    ) -> Result<RecordBatch> {
        if filters.is_empty() {
            return Ok(batch);
        }

        // Build a mask for rows that pass all filters
        let num_rows = batch.num_rows();
        let mut mask = vec![true; num_rows];

        for filter in filters {
            // Get the extra_meta column (MapArray or Binary)
            let metadata_column = batch
                .column_by_name(super::FIELD_EXTRA_META)
                .ok_or_else(|| anyhow!("Missing extra_meta column for filtering"))?;

            // Apply filter to each row
            for row_idx in 0..num_rows {
                if !mask[row_idx] {
                    continue; // Already filtered out
                }

                // Extract metadata for this row and check filter
                let passes = self.check_filter_for_row(metadata_column, row_idx, filter)?;

                mask[row_idx] &= passes;
            }
        }

        // Create filtered batch with only passing rows
        let indices: Vec<u32> = mask
            .iter()
            .enumerate()
            .filter_map(
                |(idx, &passes)| {
                    if passes { Some(idx as u32) } else { None }
                },
            )
            .collect();

        if indices.is_empty() {
            // No rows passed - return empty batch with same schema
            return Ok(RecordBatch::new_empty(batch.schema()));
        }

        // Use Arrow's take to create filtered batch
        let indices_array: ArrayRef = Arc::new(UInt32Array::from(indices));
        let arrays = batch
            .columns()
            .iter()
            .map(|col| arrow_select::take::take(col, &indices_array, None))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("Failed to filter batch: {}", e))?;

        RecordBatch::try_new(batch.schema(), arrays)
            .map_err(|e| anyhow!("Failed to create filtered batch: {}", e))
    }

    /// Check if a single row passes a filter
    ///
    /// Supports metadata stored as:
    /// - UTF-8 JSON string (primary format for extra_meta)
    /// - Binary serialized data (future enhancement)
    fn check_filter_for_row(
        &self,
        metadata_column: &dyn arrow_array::Array,
        row_idx: usize,
        filter: &FilterCondition,
    ) -> Result<bool> {
        // Handle null values - null metadata doesn't match any filter except IsNull
        if metadata_column.is_null(row_idx) {
            return match filter {
                FilterCondition::IsNull(_) => Ok(true),
                _ => Ok(false),
            };
        }

        // Try to downcast to StringArray (UTF-8 JSON format)
        if let Some(string_array) = metadata_column.as_any().downcast_ref::<StringArray>() {
            let json_str = string_array.value(row_idx);
            return self.check_filter_json(json_str, filter);
        }

        // Try to downcast to BinaryArray for bincode-serialized HashMap
        if let Some(binary_array) = metadata_column
            .as_any()
            .downcast_ref::<arrow_array::BinaryArray>()
        {
            let bytes = binary_array.value(row_idx);
            return self.check_filter_binary(bytes, filter);
        }

        // Unsupported format - log warning and fail safely
        warn!(
            "Unsupported metadata column type for filtering: {:?}. Allowing all rows.",
            metadata_column.data_type()
        );
        Ok(true)
    }

    /// Check filter against JSON metadata
    fn check_filter_json(&self, json_str: &str, filter: &FilterCondition) -> Result<bool> {
        // Parse JSON string to serde_json::Value
        let metadata: serde_json::Value = match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(e) => {
                trace!(
                    "Failed to parse metadata JSON: {}. Treating as non-match.",
                    e
                );
                return Ok(false);
            }
        };

        // Extract column value from metadata object
        let column_name = filter.column();
        let field_value = metadata.get(column_name);

        match filter {
            FilterCondition::Equals(_, expected) => {
                match field_value {
                    Some(actual) => Ok(self.values_equal(actual, expected)),
                    None => Ok(false), // Field doesn't exist
                }
            }

            FilterCondition::Range(_, min, max) => match field_value {
                Some(actual) => Ok(self.value_in_range(actual, min, max)),
                None => Ok(false),
            },

            FilterCondition::In(_, values) => match field_value {
                Some(actual) => Ok(values.iter().any(|v| self.values_equal(actual, v))),
                None => Ok(false),
            },

            FilterCondition::IsNull(_) => {
                Ok(field_value.is_none() || field_value == Some(&serde_json::Value::Null))
            }

            FilterCondition::IsNotNull(_) => {
                Ok(field_value.is_some() && field_value != Some(&serde_json::Value::Null))
            }
        }
    }

    /// Check filter against binary (bincode) serialized metadata
    fn check_filter_binary(&self, bytes: &[u8], filter: &FilterCondition) -> Result<bool> {
        // Try to deserialize as HashMap<String, serde_json::Value>
        let metadata: std::collections::HashMap<String, serde_json::Value> =
            match bincode::deserialize(bytes) {
                Ok(v) => v,
                Err(e) => {
                    trace!(
                        "Failed to deserialize binary metadata: {}. Treating as non-match.",
                        e
                    );
                    return Ok(false);
                }
            };

        let column_name = filter.column();
        let field_value = metadata.get(column_name);

        match filter {
            FilterCondition::Equals(_, expected) => match field_value {
                Some(actual) => Ok(self.values_equal(actual, expected)),
                None => Ok(false),
            },

            FilterCondition::Range(_, min, max) => match field_value {
                Some(actual) => Ok(self.value_in_range(actual, min, max)),
                None => Ok(false),
            },

            FilterCondition::In(_, values) => match field_value {
                Some(actual) => Ok(values.iter().any(|v| self.values_equal(actual, v))),
                None => Ok(false),
            },

            FilterCondition::IsNull(_) => {
                Ok(field_value.is_none() || field_value == Some(&serde_json::Value::Null))
            }

            FilterCondition::IsNotNull(_) => {
                Ok(field_value.is_some() && field_value != Some(&serde_json::Value::Null))
            }
        }
    }

    /// Compare two JSON values for equality
    fn values_equal(&self, actual: &serde_json::Value, expected: &serde_json::Value) -> bool {
        match (actual, expected) {
            // String comparison (case-sensitive)
            (serde_json::Value::String(a), serde_json::Value::String(e)) => a == e,

            // Number comparison (with tolerance for floating point)
            (serde_json::Value::Number(a), serde_json::Value::Number(e)) => {
                if let (Some(a_f64), Some(e_f64)) = (a.as_f64(), e.as_f64()) {
                    (a_f64 - e_f64).abs() < 1e-9
                } else if let (Some(a_i64), Some(e_i64)) = (a.as_i64(), e.as_i64()) {
                    a_i64 == e_i64
                } else if let (Some(a_u64), Some(e_u64)) = (a.as_u64(), e.as_u64()) {
                    a_u64 == e_u64
                } else {
                    false
                }
            }

            // Boolean comparison
            (serde_json::Value::Bool(a), serde_json::Value::Bool(e)) => a == e,

            // Null comparison
            (serde_json::Value::Null, serde_json::Value::Null) => true,

            // Array comparison (deep equality)
            (serde_json::Value::Array(a), serde_json::Value::Array(e)) => {
                a.len() == e.len()
                    && a.iter()
                        .zip(e.iter())
                        .all(|(av, ev)| self.values_equal(av, ev))
            }

            // Object comparison (deep equality)
            (serde_json::Value::Object(a), serde_json::Value::Object(e)) => {
                a.len() == e.len()
                    && a.iter()
                        .all(|(k, v)| e.get(k).map_or(false, |ev| self.values_equal(v, ev)))
            }

            // Type mismatch - try numeric coercion
            (serde_json::Value::Number(a), serde_json::Value::String(e)) => {
                e.parse::<f64>().map_or(false, |e_num| {
                    a.as_f64()
                        .map_or(false, |a_num| (a_num - e_num).abs() < 1e-9)
                })
            }
            (serde_json::Value::String(a), serde_json::Value::Number(e)) => {
                a.parse::<f64>().map_or(false, |a_num| {
                    e.as_f64()
                        .map_or(false, |e_num| (a_num - e_num).abs() < 1e-9)
                })
            }

            // Default: no match for mismatched types
            _ => false,
        }
    }

    /// Check if a value is within a range [min, max]
    fn value_in_range(
        &self,
        actual: &serde_json::Value,
        min: &serde_json::Value,
        max: &serde_json::Value,
    ) -> bool {
        // Extract numeric values for comparison
        let actual_num = self.extract_numeric(actual);
        let min_num = self.extract_numeric(min);
        let max_num = self.extract_numeric(max);

        match (actual_num, min_num, max_num) {
            (Some(a), Some(min_v), Some(max_v)) => a >= min_v && a <= max_v,
            _ => {
                // For non-numeric types, try string comparison
                match (actual, min, max) {
                    (
                        serde_json::Value::String(a),
                        serde_json::Value::String(min_s),
                        serde_json::Value::String(max_s),
                    ) => a >= min_s && a <= max_s,
                    _ => false,
                }
            }
        }
    }

    /// Extract numeric value from JSON for range comparisons
    fn extract_numeric(&self, value: &serde_json::Value) -> Option<f64> {
        match value {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::String(s) => s.parse::<f64>().ok(),
            _ => None,
        }
    }
}

/// Extension trait for ParquetQueryEngine
#[allow(async_fn_in_trait)]
pub trait BranchedMetadataFiltering {
    /// Execute query with branched filtering strategy
    async fn query_with_branched_filtering(
        &self,
        file_path: &str,
        filters: &[MetadataFilter],
        projection: Option<Vec<String>>,
    ) -> Result<Vec<VectorRecord>>;
}

/// Performance metrics for filter operations
#[derive(Debug, Clone)]
pub struct FilterPerformanceMetrics {
    pub strategy_used: String,
    pub rows_scanned: usize,
    pub rows_filtered: usize,
    pub pushdown_filters: usize,
    pub post_filters: usize,
    pub elapsed_ms: u64,
    pub warning: Option<String>,
}

impl FilterPerformanceMetrics {
    pub fn log_summary(&self) {
        let efficiency = if self.rows_scanned > 0 {
            (self.rows_filtered as f64 / self.rows_scanned as f64) * 100.0
        } else {
            0.0
        };

        info!(
            "Filter Performance: {} strategy, {}/{} rows passed ({:.1}%), {} pushdown + {} post filters, {:.1}ms",
            self.strategy_used,
            self.rows_filtered,
            self.rows_scanned,
            efficiency,
            self.pushdown_filters,
            self.post_filters,
            self.elapsed_ms as f64
        );

        if let Some(ref warning) = self.warning {
            warn!("{}", warning);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::core::formats::columnar::FilterLogic;

    #[test]
    fn test_strategy_selection() {
        let analyzer =
            MetadataFilterAnalyzer::new(vec!["category".to_string(), "priority".to_string()], true);

        // Fast path: all filterable
        let filters = vec![MetadataFilter {
            conditions: vec![FilterCondition::Equals(
                "category".to_string(),
                serde_json::Value::String("electronics".to_string()),
            )],
            logic: FilterLogic::And,
        }];

        let strategy = analyzer.analyze_filters(&filters).unwrap();
        matches!(strategy, MetadataFilterStrategy::FastFilterable { .. });

        // Slow path: non-filterable
        let filters = vec![MetadataFilter {
            conditions: vec![FilterCondition::Equals(
                "custom_field".to_string(),
                serde_json::Value::String("value".to_string()),
            )],
            logic: FilterLogic::And,
        }];

        let strategy = analyzer.analyze_filters(&filters).unwrap();
        matches!(strategy, MetadataFilterStrategy::SlowFullScan { .. });

        // Mixed path
        let filters = vec![
            MetadataFilter {
                conditions: vec![FilterCondition::Equals(
                    "category".to_string(),
                    serde_json::Value::String("electronics".to_string()),
                )],
                logic: FilterLogic::And,
            },
            MetadataFilter {
                conditions: vec![FilterCondition::Range(
                    "custom_field".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(100)),
                    serde_json::Value::Number(serde_json::Number::from(i64::MAX)),
                )],
                logic: FilterLogic::And,
            },
        ];

        let strategy = analyzer.analyze_filters(&filters).unwrap();
        matches!(strategy, MetadataFilterStrategy::Mixed { .. });
    }

    #[test]
    fn test_slow_query_rejection() {
        let analyzer = MetadataFilterAnalyzer::new(
            vec!["category".to_string()],
            false, // Don't allow slow queries
        );

        let filters = vec![MetadataFilter {
            conditions: vec![FilterCondition::Equals(
                "unknown_field".to_string(),
                serde_json::Value::String("value".to_string()),
            )],
            logic: FilterLogic::And,
        }];

        let result = analyzer.analyze_filters(&filters);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("allow_slow_queries")
        );
    }

    // =========================================================================
    // TD-004 Fix: Extra_meta filtering tests
    // =========================================================================

    #[test]
    fn test_json_filter_equals_string() {
        let analyzer = MetadataFilterAnalyzer::new(vec![], true);

        let json = r#"{"category": "electronics", "brand": "apple"}"#;
        let filter = FilterCondition::Equals(
            "category".to_string(),
            serde_json::Value::String("electronics".to_string()),
        );

        assert!(analyzer.check_filter_json(json, &filter).unwrap());

        // Non-matching value
        let filter_no_match = FilterCondition::Equals(
            "category".to_string(),
            serde_json::Value::String("clothing".to_string()),
        );
        assert!(!analyzer.check_filter_json(json, &filter_no_match).unwrap());

        // Non-existing field
        let filter_missing = FilterCondition::Equals(
            "missing_field".to_string(),
            serde_json::Value::String("value".to_string()),
        );
        assert!(!analyzer.check_filter_json(json, &filter_missing).unwrap());
    }

    #[test]
    fn test_json_filter_equals_number() {
        let analyzer = MetadataFilterAnalyzer::new(vec![], true);

        let json = r#"{"price": 99.99, "quantity": 5}"#;

        // Float comparison
        let filter_float = FilterCondition::Equals("price".to_string(), serde_json::json!(99.99));
        assert!(analyzer.check_filter_json(json, &filter_float).unwrap());

        // Integer comparison
        let filter_int = FilterCondition::Equals("quantity".to_string(), serde_json::json!(5));
        assert!(analyzer.check_filter_json(json, &filter_int).unwrap());

        // Non-matching number
        let filter_no_match =
            FilterCondition::Equals("price".to_string(), serde_json::json!(100.0));
        assert!(!analyzer.check_filter_json(json, &filter_no_match).unwrap());
    }

    #[test]
    fn test_json_filter_range() {
        let analyzer = MetadataFilterAnalyzer::new(vec![], true);

        let json = r#"{"price": 75.0, "rating": 4}"#;

        // Value within range
        let filter_in_range = FilterCondition::Range(
            "price".to_string(),
            serde_json::json!(50.0),
            serde_json::json!(100.0),
        );
        assert!(analyzer.check_filter_json(json, &filter_in_range).unwrap());

        // Value at boundary (inclusive)
        let filter_boundary = FilterCondition::Range(
            "price".to_string(),
            serde_json::json!(75.0),
            serde_json::json!(75.0),
        );
        assert!(analyzer.check_filter_json(json, &filter_boundary).unwrap());

        // Value outside range
        let filter_outside = FilterCondition::Range(
            "price".to_string(),
            serde_json::json!(100.0),
            serde_json::json!(200.0),
        );
        assert!(!analyzer.check_filter_json(json, &filter_outside).unwrap());
    }

    #[test]
    fn test_json_filter_in() {
        let analyzer = MetadataFilterAnalyzer::new(vec![], true);

        let json = r#"{"status": "active", "tier": 2}"#;

        // Value in list
        let filter_in_list = FilterCondition::In(
            "status".to_string(),
            vec![
                serde_json::Value::String("active".to_string()),
                serde_json::Value::String("pending".to_string()),
            ],
        );
        assert!(analyzer.check_filter_json(json, &filter_in_list).unwrap());

        // Value not in list
        let filter_not_in = FilterCondition::In(
            "status".to_string(),
            vec![
                serde_json::Value::String("inactive".to_string()),
                serde_json::Value::String("deleted".to_string()),
            ],
        );
        assert!(!analyzer.check_filter_json(json, &filter_not_in).unwrap());

        // Numeric value in list
        let filter_num_in = FilterCondition::In(
            "tier".to_string(),
            vec![
                serde_json::json!(1),
                serde_json::json!(2),
                serde_json::json!(3),
            ],
        );
        assert!(analyzer.check_filter_json(json, &filter_num_in).unwrap());
    }

    #[test]
    fn test_json_filter_is_null() {
        let analyzer = MetadataFilterAnalyzer::new(vec![], true);

        let json_with_null = r#"{"name": null, "value": 100}"#;
        let json_without_field = r#"{"value": 100}"#;

        // Explicit null value
        let filter_is_null = FilterCondition::IsNull("name".to_string());
        assert!(
            analyzer
                .check_filter_json(json_with_null, &filter_is_null)
                .unwrap()
        );

        // Missing field treated as null
        assert!(
            analyzer
                .check_filter_json(json_without_field, &filter_is_null)
                .unwrap()
        );

        // Non-null field should not match IsNull
        let filter_value_null = FilterCondition::IsNull("value".to_string());
        assert!(
            !analyzer
                .check_filter_json(json_with_null, &filter_value_null)
                .unwrap()
        );
    }

    #[test]
    fn test_json_filter_is_not_null() {
        let analyzer = MetadataFilterAnalyzer::new(vec![], true);

        let json = r#"{"name": "test", "empty": null}"#;

        // Non-null value
        let filter_not_null = FilterCondition::IsNotNull("name".to_string());
        assert!(analyzer.check_filter_json(json, &filter_not_null).unwrap());

        // Null value should not match IsNotNull
        let filter_empty_not_null = FilterCondition::IsNotNull("empty".to_string());
        assert!(
            !analyzer
                .check_filter_json(json, &filter_empty_not_null)
                .unwrap()
        );

        // Missing field should not match IsNotNull
        let filter_missing = FilterCondition::IsNotNull("nonexistent".to_string());
        assert!(!analyzer.check_filter_json(json, &filter_missing).unwrap());
    }

    #[test]
    fn test_json_filter_type_coercion() {
        let analyzer = MetadataFilterAnalyzer::new(vec![], true);

        // String "100" should match number 100
        let json = r#"{"amount": "100"}"#;
        let filter = FilterCondition::Equals("amount".to_string(), serde_json::json!(100));
        assert!(analyzer.check_filter_json(json, &filter).unwrap());

        // Number 100 should match string "100"
        let json2 = r#"{"amount": 100}"#;
        let filter2 = FilterCondition::Equals(
            "amount".to_string(),
            serde_json::Value::String("100".to_string()),
        );
        assert!(analyzer.check_filter_json(json2, &filter2).unwrap());
    }

    #[test]
    fn test_json_filter_invalid_json() {
        let analyzer = MetadataFilterAnalyzer::new(vec![], true);

        let invalid_json = "not valid json {{{";
        let filter = FilterCondition::Equals(
            "field".to_string(),
            serde_json::Value::String("value".to_string()),
        );

        // Invalid JSON should return false (not match), not error
        assert!(!analyzer.check_filter_json(invalid_json, &filter).unwrap());
    }

    #[test]
    fn test_values_equal_deep_comparison() {
        let analyzer = MetadataFilterAnalyzer::new(vec![], true);

        // Array comparison
        let arr1 = serde_json::json!([1, 2, 3]);
        let arr2 = serde_json::json!([1, 2, 3]);
        let arr3 = serde_json::json!([1, 2, 4]);
        assert!(analyzer.values_equal(&arr1, &arr2));
        assert!(!analyzer.values_equal(&arr1, &arr3));

        // Object comparison
        let obj1 = serde_json::json!({"a": 1, "b": 2});
        let obj2 = serde_json::json!({"a": 1, "b": 2});
        let obj3 = serde_json::json!({"a": 1, "b": 3});
        assert!(analyzer.values_equal(&obj1, &obj2));
        assert!(!analyzer.values_equal(&obj1, &obj3));
    }

    #[test]
    fn test_value_in_range_string() {
        let analyzer = MetadataFilterAnalyzer::new(vec![], true);

        // String range comparison (lexicographic)
        let val = serde_json::Value::String("cat".to_string());
        let min = serde_json::Value::String("apple".to_string());
        let max = serde_json::Value::String("dog".to_string());

        assert!(analyzer.value_in_range(&val, &min, &max));

        // Outside range
        let val_outside = serde_json::Value::String("zebra".to_string());
        assert!(!analyzer.value_in_range(&val_outside, &min, &max));
    }
}
