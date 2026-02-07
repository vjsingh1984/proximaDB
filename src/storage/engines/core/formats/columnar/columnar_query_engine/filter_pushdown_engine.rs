//! Predicate Pushdown and Filter Building
//!
//! This module handles the construction and optimization of predicates
//! for Parquet filter pushdown, enabling efficient query execution.

use anyhow::{Result, anyhow};
use arrow::datatypes::Schema;
use parquet::file::metadata::RowGroupMetaData;
use std::sync::Arc;
use tracing::debug;

// Use the columnar module's MetadataFilter, not the proto one
use crate::storage::engines::core::formats::columnar::{FilterCondition, MetadataFilter};

/// Predicate builder for filter pushdown
pub struct PredicateBuilder {
    filters: Vec<MetadataFilter>,
    schema: Option<Arc<Schema>>,
}

impl PredicateBuilder {
    /// Create new predicate builder
    pub fn new() -> Self {
        Self {
            filters: Vec::new(),
            schema: None,
        }
    }

    /// Set the schema for validation
    pub fn with_schema(mut self, schema: Arc<Schema>) -> Self {
        self.schema = Some(schema);
        self
    }

    /// Add a metadata filter
    pub fn add_filter(mut self, filter: MetadataFilter) -> Self {
        self.filters.push(filter);
        self
    }

    /// Add multiple filters
    pub fn add_filters(mut self, filters: Vec<MetadataFilter>) -> Self {
        self.filters.extend(filters);
        self
    }

    /// Build Arrow predicate expression
    pub fn build_arrow_predicate(&self) -> Result<Option<String>> {
        if self.filters.is_empty() {
            return Ok(None);
        }

        let mut filter_groups = Vec::new();

        for filter in &self.filters {
            let mut predicates = Vec::new();

            for condition in &filter.conditions {
                let predicate = self.build_single_predicate(condition)?;
                predicates.push(predicate);
            }

            if !predicates.is_empty() {
                let _filter_logic = match filter.logic {
                    crate::storage::engines::core::formats::columnar::FilterLogic::And => " AND ",
                    crate::storage::engines::core::formats::columnar::FilterLogic::Or => " OR ",
                };
                let group = format!("({})", predicates.join(" AND "));
                filter_groups.push(group);
            }
        }

        if filter_groups.is_empty() {
            Ok(None)
        } else {
            // Multiple filter groups are combined with AND
            Ok(Some(filter_groups.join(" AND ")))
        }
    }

    /// Build a single predicate from a condition
    fn build_single_predicate(&self, condition: &FilterCondition) -> Result<String> {
        let predicate = match condition {
            FilterCondition::Equals(field, value) => {
                let value_str = self.format_value(value)?;
                format!("{} = {}", field, value_str)
            }
            FilterCondition::Range(field, min, max) => {
                let min_str = self.format_value(min)?;
                let max_str = self.format_value(max)?;
                format!("{} >= {} AND {} <= {}", field, min_str, field, max_str)
            }
            FilterCondition::In(field, values) => {
                let value_strs: Result<Vec<String>> =
                    values.iter().map(|v| self.format_value(v)).collect();
                let values_list = value_strs?.join(", ");
                format!("{} IN ({})", field, values_list)
            }
            FilterCondition::IsNull(field) => {
                format!("{} IS NULL", field)
            }
            FilterCondition::IsNotNull(field) => {
                format!("{} IS NOT NULL", field)
            }
        };

        Ok(predicate)
    }

    /// Format a serde_json::Value for SQL
    fn format_value(&self, value: &serde_json::Value) -> Result<String> {
        match value {
            serde_json::Value::String(s) => Ok(format!("'{}'", s)),
            serde_json::Value::Number(n) => Ok(n.to_string()),
            serde_json::Value::Bool(b) => Ok(b.to_string()),
            serde_json::Value::Null => Ok("NULL".to_string()),
            _ => Err(anyhow!("Unsupported value type: {:?}", value)),
        }
    }

    /// Check if filters can be pushed down to Parquet
    pub fn can_pushdown(&self) -> bool {
        // Check if all filters are on columns that exist in schema
        if let Some(ref schema) = self.schema {
            for filter in &self.filters {
                for condition in &filter.conditions {
                    let field = condition.column();
                    if schema.field_with_name(field).is_err() {
                        return false;
                    }
                }
            }
            true
        } else {
            // Conservative: don't push down without schema
            false
        }
    }

    /// Evaluate filters against row group statistics
    pub fn evaluate_row_group(&self, metadata: &RowGroupMetaData) -> bool {
        // This would check row group statistics to see if it can be skipped
        // For now, return true to read all row groups
        true
    }
}

/// Filter pushdown optimizer
pub struct FilterPushdown {
    enable_statistics_pruning: bool,
    enable_bloom_filter: bool,
}

impl FilterPushdown {
    /// Create new filter pushdown optimizer
    pub fn new() -> Self {
        Self {
            enable_statistics_pruning: true,
            enable_bloom_filter: true,
        }
    }

    /// Optimize filters for pushdown
    pub fn optimize_filters(&self, filters: &[MetadataFilter]) -> FilterPushdownPlan {
        let mut pushdown_filters = Vec::new();
        let mut post_filters = Vec::new();

        for filter in filters {
            if self.can_push_filter(filter) {
                pushdown_filters.push(filter.clone());
            } else {
                post_filters.push(filter.clone());
            }
        }

        FilterPushdownPlan {
            pushdown_filters,
            post_filters,
            use_statistics: self.enable_statistics_pruning,
            use_bloom_filter: self.enable_bloom_filter,
        }
    }

    /// Check if a filter can be pushed down
    fn can_push_filter(&self, _filter: &MetadataFilter) -> bool {
        // Check if filter uses only supported operations
        // All FilterCondition enum variants are supported for pushdown
        // FilterCondition enum already validated
        true
    }

    /// Prune row groups based on statistics
    pub fn prune_row_groups(
        &self,
        filters: &[MetadataFilter],
        row_groups: &[RowGroupMetaData],
    ) -> Vec<usize> {
        if !self.enable_statistics_pruning {
            return (0..row_groups.len()).collect();
        }

        let mut selected = Vec::new();

        for (idx, rg) in row_groups.iter().enumerate() {
            if self.row_group_matches(filters, rg) {
                selected.push(idx);
            }
        }

        debug!(
            "Pruned row groups: selected {} out of {}",
            selected.len(),
            row_groups.len()
        );

        selected
    }

    /// Check if row group matches filters based on statistics
    fn row_group_matches(&self, filters: &[MetadataFilter], rg: &RowGroupMetaData) -> bool {
        // This would check column statistics (min/max) against filters
        // For now, return true to read all row groups
        true
    }
}

/// Plan for filter pushdown execution
#[derive(Debug, Clone)]
pub struct FilterPushdownPlan {
    /// Filters that can be pushed to Parquet
    pub pushdown_filters: Vec<MetadataFilter>,

    /// Filters that must be applied post-read
    pub post_filters: Vec<MetadataFilter>,

    /// Whether to use statistics for pruning
    pub use_statistics: bool,

    /// Whether to use bloom filters
    pub use_bloom_filter: bool,
}

impl FilterPushdownPlan {
    /// Check if any pushdown is possible
    pub fn has_pushdown(&self) -> bool {
        !self.pushdown_filters.is_empty()
    }

    /// Check if post-filtering is needed
    pub fn needs_post_filter(&self) -> bool {
        !self.post_filters.is_empty()
    }

    /// Get total filter count
    pub fn total_filters(&self) -> usize {
        self.pushdown_filters.len() + self.post_filters.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::core::formats::columnar::{FilterCondition, FilterLogic};

    #[test]
    fn test_predicate_builder() {
        let filter = MetadataFilter {
            conditions: vec![FilterCondition::Equals(
                "category".to_string(),
                serde_json::Value::String("test".to_string()),
            )],
            logic: FilterLogic::And,
        };

        let builder = PredicateBuilder::new().add_filter(filter);

        let predicate = builder.build_arrow_predicate().unwrap();
        assert!(predicate.is_some());
        assert!(predicate.unwrap().contains("category = 'test'"));
    }

    #[test]
    fn test_filter_pushdown_optimization() {
        let pushdown = FilterPushdown::new();

        let filter = MetadataFilter {
            conditions: vec![FilterCondition::Range(
                "score".to_string(),
                serde_json::Value::Number(serde_json::Number::from_f64(0.5).unwrap()),
                serde_json::Value::Number(serde_json::Number::from_f64(1.0).unwrap()),
            )],
            logic: FilterLogic::And,
        };

        let plan = pushdown.optimize_filters(&[filter]);
        assert_eq!(plan.pushdown_filters.len(), 1);
        assert_eq!(plan.post_filters.len(), 0);
    }
}
