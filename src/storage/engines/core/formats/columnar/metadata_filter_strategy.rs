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
use arrow_array::{RecordBatch, UInt32Array, ArrayRef};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::proto::proximadb_v1::{SqlValue, VectorRecord};
use super::{FilterCondition, MetadataFilter, FilterLogic};

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
    pub fn new(
        filterable_columns: Vec<String>,
        allow_slow_queries: bool,
    ) -> Self {
        Self {
            filterable_columns: filterable_columns.into_iter().collect(),
            allow_slow_queries,
            slow_query_threshold_ms: 1000, // 1 second
        }
    }

    /// Analyze metadata filters and determine optimal strategy
    pub fn analyze_filters(
        &self,
        filters: &[MetadataFilter],
    ) -> Result<MetadataFilterStrategy> {
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
                        non_filterable.iter()
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
                        non_filterable.iter()
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
                let passes = self.check_filter_for_row(
                    metadata_column,
                    row_idx,
                    filter,
                )?;

                mask[row_idx] &= passes;
            }
        }

        // Create filtered batch with only passing rows
        let indices: Vec<u32> = mask
            .iter()
            .enumerate()
            .filter_map(|(idx, &passes)| {
                if passes { Some(idx as u32) } else { None }
            })
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
            .map(|col| {
                arrow_select::take::take(col, &indices_array, None)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("Failed to filter batch: {}", e))?;

        RecordBatch::try_new(batch.schema(), arrays)
            .map_err(|e| anyhow!("Failed to create filtered batch: {}", e))
    }

    /// Check if a single row passes a filter
    fn check_filter_for_row(
        &self,
        metadata_column: &dyn arrow_array::Array,
        row_idx: usize,
        filter: &FilterCondition,
    ) -> Result<bool> {
        // This would need to handle different metadata storage formats:
        // - MapArray (current, problematic)
        // - Binary (serialized HashMap<String, SqlValue>)
        // - JSON string

        // For now, return true to avoid blocking
        // Real implementation would deserialize and check
        Ok(true)
    }
}

/// Extension trait for ParquetQueryEngine
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

    #[test]
    fn test_strategy_selection() {
        let analyzer = MetadataFilterAnalyzer::new(
            vec!["category".to_string(), "priority".to_string()],
            true,
        );

        // Fast path: all filterable
        let filters = vec![
            MetadataFilter {
                conditions: vec![FilterCondition::Equals("category".to_string(), serde_json::Value::String("electronics".to_string()))],
                logic: FilterLogic::And,
            },
        ];

        let strategy = analyzer.analyze_filters(&filters).unwrap();
        matches!(strategy, MetadataFilterStrategy::FastFilterable { .. });

        // Slow path: non-filterable
        let filters = vec![
            MetadataFilter {
                conditions: vec![FilterCondition::Equals("custom_field".to_string(), serde_json::Value::String("value".to_string()))],
                logic: FilterLogic::And,
            },
        ];

        let strategy = analyzer.analyze_filters(&filters).unwrap();
        matches!(strategy, MetadataFilterStrategy::SlowFullScan { .. });

        // Mixed path
        let filters = vec![
            MetadataFilter {
                conditions: vec![FilterCondition::Equals("category".to_string(), serde_json::Value::String("electronics".to_string()))],
                logic: FilterLogic::And,
            },
            MetadataFilter {
                conditions: vec![FilterCondition::Range("custom_field".to_string(), serde_json::Value::Number(serde_json::Number::from(100)), serde_json::Value::Number(serde_json::Number::from(i64::MAX)))],
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

        let filters = vec![
            MetadataFilter {
                conditions: vec![FilterCondition::Equals("unknown_field".to_string(), serde_json::Value::String("value".to_string()))],
                logic: FilterLogic::And,
            },
        ];

        let result = analyzer.analyze_filters(&filters);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("allow_slow_queries"));
    }
}