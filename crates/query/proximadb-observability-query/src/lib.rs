use std::collections::HashMap;

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
}
