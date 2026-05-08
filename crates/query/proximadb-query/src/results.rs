//! Shared cross-model query result contracts for the extracted query runtime.

use std::collections::HashMap;

use proximadb_data_model::DataModel;

/// Result of a unified query.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Result records.
    pub records: Vec<UnifiedRecord>,
    /// Total count (if available).
    pub total_count: Option<u64>,
    /// Execution metrics.
    pub metrics: QueryMetrics,
}

/// A unified record from any data model.
#[derive(Debug, Clone)]
pub struct UnifiedRecord {
    /// Record ID.
    pub id: String,
    /// Source model.
    pub source_model: DataModel,
    /// Record data as JSON.
    pub data: serde_json::Value,
    /// Relevance score (if applicable).
    pub score: Option<f64>,
    /// Additional metadata.
    pub metadata: HashMap<String, String>,
}

/// Query execution metrics.
#[derive(Debug, Clone, Default)]
pub struct QueryMetrics {
    /// Total execution time in microseconds.
    pub total_time_us: u64,
    /// Time per sub-query.
    pub sub_query_times: Vec<(DataModel, u64)>,
    /// Number of records scanned.
    pub records_scanned: u64,
    /// Number of records returned.
    pub records_returned: u64,
    /// Cache hit rate.
    pub cache_hit_rate: f64,
}
