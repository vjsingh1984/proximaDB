// ============================================================================
// Observability Service Contract (Phase 2.2 - TDD Implementation)
// ============================================================================

use async_trait::async_trait;
use proximadb_kernel::error::ProximaDBError;
use std::collections::HashMap;

/// Canonical observability-query contract result type.
pub type ObservabilityQueryResult<T> = std::result::Result<T, ProximaDBError>;

/// Log entry result from observability queries.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Timestamp (nanoseconds since epoch).
    pub timestamp_ns: i64,
    /// Log level (e.g., INFO, WARN, ERROR).
    pub level: String,
    /// Service/component name.
    pub service: String,
    /// Log message.
    pub message: String,
    /// Additional structured metadata.
    pub metadata: HashMap<String, String>,
}

/// Metric data point result from observability queries.
#[derive(Debug, Clone)]
pub struct MetricDataPoint {
    /// Timestamp (nanoseconds since epoch).
    pub timestamp_ns: i64,
    /// Metric value.
    pub value: f64,
    /// Labels for this data point.
    pub labels: HashMap<String, String>,
}

/// Log search request for observability queries.
#[derive(Debug, Clone)]
pub struct LogSearchRequest {
    /// Namespace to query.
    pub namespace: String,
    /// Start time (nanoseconds since epoch).
    pub start_time_ns: i64,
    /// End time (nanoseconds since epoch).
    pub end_time_ns: i64,
    /// Optional full-text/log search query.
    pub query: Option<String>,
    /// Severity filters.
    pub severities: Vec<String>,
    /// Service filters.
    pub services: Vec<String>,
    /// Result limit.
    pub limit: usize,
}

/// Log search result.
#[derive(Debug, Clone)]
pub struct LogSearchResult {
    /// Retrieved log entries.
    pub results: Vec<LogEntry>,
    /// Total count before limit was applied.
    pub total_count: usize,
    /// Query execution time in milliseconds.
    pub execution_time_ms: u64,
}

/// Metric search request for observability queries.
#[derive(Debug, Clone)]
pub struct MetricSearchRequest {
    /// Namespace to query.
    pub namespace: String,
    /// Metric name.
    pub metric_name: String,
    /// Start time (nanoseconds since epoch).
    pub start_time_ns: i64,
    /// End time (nanoseconds since epoch).
    pub end_time_ns: i64,
    /// Aggregation function.
    pub aggregation: MetricAggregation,
    /// Group-by labels.
    pub group_by: Vec<String>,
    /// Label filters.
    pub label_filters: HashMap<String, String>,
}

/// Metric search result.
#[derive(Debug, Clone)]
pub struct MetricSearchResult {
    /// Retrieved metric data points.
    pub results: Vec<MetricDataPoint>,
    /// Query execution time in milliseconds.
    pub execution_time_ms: u64,
}

/// Narrow async observability-query contract for observability-facing query runtimes.
///
/// This trait defines the core observability search operations that cross-modal query
/// orchestration depends on. It is intentionally narrow, focusing on read/query
/// operations for logs and metrics.
///
/// Design principles:
/// - **Narrow**: Only essential search operations to keep the trait focused
/// - **Stable types**: Uses simple, stable types for results
/// - **Async**: All operations are async to support multiple storage backends
/// - **Error handling**: Uses `ProximaDBError` for consistent error reporting
#[async_trait]
pub trait ObservabilityQueryService: Send + Sync {
    /// Execute a log search.
    ///
    /// # Arguments
    ///
    /// * `request` - Log search parameters including namespace, time range, filters
    ///
    /// # Returns
    ///
    /// * `LogSearchResult` - Search results with log entries and timing
    async fn search_logs(
        &self,
        request: LogSearchRequest,
    ) -> ObservabilityQueryResult<LogSearchResult>;

    /// Execute a metric search.
    ///
    /// # Arguments
    ///
    /// * `request` - Metric search parameters including namespace, metric name, aggregation
    ///
    /// # Returns
    ///
    /// * `MetricSearchResult` - Search results with metric data points and timing
    async fn search_metrics(
        &self,
        request: MetricSearchRequest,
    ) -> ObservabilityQueryResult<MetricSearchResult>;
}

// ============================================================================
// Legacy Expression Types (kept for backward compatibility)
// ============================================================================

/// Log query expression used by cross-model query orchestration.
#[derive(Debug, Clone)]
pub struct LogQueryExpr {
    /// Namespace to query.
    pub namespace: String,
    /// Start time (nanoseconds since epoch).
    pub start_time_ns: i64,
    /// End time (nanoseconds since epoch).
    pub end_time_ns: i64,
    /// Optional full-text/log search query.
    pub query: Option<String>,
    /// Severity filters.
    pub severities: Vec<String>,
    /// Service filters.
    pub services: Vec<String>,
    /// Result limit.
    pub limit: u32,
}

/// Metric query expression used by cross-model query orchestration.
#[derive(Debug, Clone)]
pub struct MetricQueryExpr {
    /// Namespace to query.
    pub namespace: String,
    /// Metric name.
    pub metric_name: String,
    /// Start time (nanoseconds since epoch).
    pub start_time_ns: i64,
    /// End time (nanoseconds since epoch).
    pub end_time_ns: i64,
    /// Aggregation function.
    pub aggregation: MetricAggregation,
    /// Group-by labels.
    pub group_by: Vec<String>,
    /// Label filters.
    pub label_filters: HashMap<String, String>,
}

/// Metric aggregation functions for query IR.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MetricAggregation {
    Sum,
    #[default]
    Avg,
    Min,
    Max,
    Count,
    P50,
    P90,
    P95,
    P99,
    Rate,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_aggregation_defaults_to_avg() {
        assert_eq!(MetricAggregation::default(), MetricAggregation::Avg);
    }

    #[test]
    fn log_and_metric_query_expr_hold_requested_fields() {
        let log_query = LogQueryExpr {
            namespace: "prod".to_string(),
            start_time_ns: 10,
            end_time_ns: 20,
            query: Some("timeout".to_string()),
            severities: vec!["ERROR".to_string()],
            services: vec!["api".to_string()],
            limit: 50,
        };
        assert_eq!(log_query.namespace, "prod");
        assert_eq!(log_query.limit, 50);

        let metric_query = MetricQueryExpr {
            namespace: "prod".to_string(),
            metric_name: "latency_ms".to_string(),
            start_time_ns: 10,
            end_time_ns: 20,
            aggregation: MetricAggregation::P95,
            group_by: vec!["service".to_string()],
            label_filters: HashMap::from([("env".to_string(), "prod".to_string())]),
        };
        assert_eq!(metric_query.metric_name, "latency_ms");
        assert_eq!(metric_query.aggregation, MetricAggregation::P95);
        assert_eq!(metric_query.group_by, vec!["service"]);
    }

    // ========================================================================
    // ObservabilityQueryService Trait Tests (TDD)
    // ========================================================================

    #[test]
    fn log_search_request_has_required_fields() {
        let request = LogSearchRequest {
            namespace: "prod".to_string(),
            start_time_ns: 1000,
            end_time_ns: 2000,
            query: Some("timeout".to_string()),
            severities: vec!["ERROR".to_string()],
            services: vec!["api".to_string()],
            limit: 50,
        };

        assert_eq!(request.namespace, "prod");
        assert_eq!(request.start_time_ns, 1000);
        assert_eq!(request.end_time_ns, 2000);
        assert_eq!(request.query.as_deref(), Some("timeout"));
        assert_eq!(request.severities, vec!["ERROR"]);
        assert_eq!(request.services, vec!["api"]);
        assert_eq!(request.limit, 50);
    }

    #[test]
    fn metric_search_request_has_required_fields() {
        let mut label_filters = HashMap::new();
        label_filters.insert("env".to_string(), "prod".to_string());

        let request = MetricSearchRequest {
            namespace: "prod".to_string(),
            metric_name: "latency_ms".to_string(),
            start_time_ns: 1000,
            end_time_ns: 2000,
            aggregation: MetricAggregation::P95,
            group_by: vec!["service".to_string()],
            label_filters,
        };

        assert_eq!(request.namespace, "prod");
        assert_eq!(request.metric_name, "latency_ms");
        assert_eq!(request.aggregation, MetricAggregation::P95);
        assert_eq!(request.group_by, vec!["service"]);
        assert_eq!(request.label_filters.get("env"), Some(&"prod".to_string()));
    }

    #[test]
    fn log_search_result_contains_results_and_metadata() {
        let result = LogSearchResult {
            results: vec![],
            total_count: 0,
            execution_time_ms: 100,
        };

        assert_eq!(result.results.len(), 0);
        assert_eq!(result.total_count, 0);
        assert_eq!(result.execution_time_ms, 100);
    }

    #[test]
    fn metric_search_result_contains_results_and_timing() {
        let result = MetricSearchResult {
            results: vec![],
            execution_time_ms: 50,
        };

        assert_eq!(result.results.len(), 0);
        assert_eq!(result.execution_time_ms, 50);
    }

    #[test]
    fn log_entry_structure() {
        let mut metadata = HashMap::new();
        metadata.insert("user_id".to_string(), "12345".to_string());

        let entry = LogEntry {
            timestamp_ns: 1640000000000000,
            level: "INFO".to_string(),
            service: "api".to_string(),
            message: "Request processed".to_string(),
            metadata,
        };

        assert_eq!(entry.level, "INFO");
        assert_eq!(entry.service, "api");
        assert_eq!(entry.message, "Request processed");
        assert_eq!(entry.metadata.get("user_id"), Some(&"12345".to_string()));
    }

    #[test]
    fn metric_data_point_structure() {
        let mut labels = HashMap::new();
        labels.insert("service".to_string(), "api".to_string());

        let point = MetricDataPoint {
            timestamp_ns: 1640000000000000,
            value: 123.45,
            labels,
        };

        assert_eq!(point.value, 123.45);
        assert_eq!(point.labels.get("service"), Some(&"api".to_string()));
    }

    #[test]
    fn observability_query_result_type_alias() {
        // Verify that ObservabilityQueryResult is the canonical result type
        fn check_alias() -> ObservabilityQueryResult<String> {
            Ok("test".to_string())
        }
        // This just verifies the type alias compiles correctly
        let _ = check_alias();
    }
}
