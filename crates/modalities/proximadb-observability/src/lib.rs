//! # ProximaDB Observability Modality
//!
//! This crate contains observability operations for logs, metrics, and traces
//! in the ProximaDB vector database.
//!
//! ## Architecture
//!
//! The observability modality is organized into several key modules:
//!
//! - **`query`** - Observability query expressions (log queries, metric queries)
//! - **`logs`** - Log ingestion and query
//! - **`metrics`** - Metrics aggregation and query
//! - **`traces`** - Distributed tracing
//!
//! ## Foundation
//!
//! This crate serves as the foundation for observability operations across ProximaDB,
//! providing reusable contracts and implementations for:
//!
//! - Storage engines that need observability data retention
//! - Query executors that need observability operations
//! - SIEM adapters and external observability systems
//!
//! ## Dependencies
//!
//! - `proximadb-kernel` - Core error types and foundational contracts
//! - `proximadb-proto` - Protocol buffer types
//! - `proximadb-query-filter` - Filter expression contracts
//! - `arrow` - Columnar data structures for observability operations

use proximadb_query_filter::{FilterOperator, FilterValue};

/// Log query expression
#[derive(Debug, Clone)]
pub struct LogQueryExpr {
    /// Collection to query
    pub collection: String,
    /// Filters to apply
    pub filters: Vec<LogFilter>,
    /// Time range
    pub time_range: Option<TimeRange>,
    /// Fields to return
    pub projection: Vec<String>,
    /// Maximum results
    pub limit: Option<u32>,
}

/// Metric query expression
#[derive(Debug, Clone)]
pub struct MetricQueryExpr {
    /// Collection to query
    pub collection: String,
    /// Metric name
    pub metric_name: String,
    /// Aggregation
    pub aggregation: MetricAggregation,
    /// Filters to apply
    pub filters: Vec<MetricFilter>,
    /// Time range
    pub time_range: Option<TimeRange>,
    /// Group by fields
    pub group_by: Vec<String>,
}

/// Metric aggregation
#[derive(Debug, Clone)]
pub enum MetricAggregation {
    /// Average
    Avg,
    /// Sum
    Sum,
    /// Count
    Count,
    /// Min
    Min,
    /// Max
    Max,
    /// Percentile
    Percentile(f64),
}

/// Log filter
#[derive(Debug, Clone)]
pub struct LogFilter {
    pub field: String,
    pub operator: FilterOperator,
    pub value: FilterValue,
}

/// Metric filter
#[derive(Debug, Clone)]
pub struct MetricFilter {
    pub label: String,
    pub operator: FilterOperator,
    pub value: FilterValue,
}

/// Time range
#[derive(Debug, Clone)]
pub struct TimeRange {
    pub start: i64,
    pub end: i64,
}

// TODO: Move these from src/observability
// pub mod logs;
// pub mod metrics;
// pub mod traces;
// pub mod event_log;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_observability_module_imports() {
        // Basic test to verify the module structure is working
        let _log_query = LogQueryExpr {
            collection: "logs".to_string(),
            filters: vec![],
            time_range: None,
            projection: vec![],
            limit: None,
        };
        // More comprehensive tests will be added as modules are extracted
    }
}
