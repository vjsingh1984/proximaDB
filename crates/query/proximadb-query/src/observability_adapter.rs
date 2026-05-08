//! Pure observability-query adaptation helpers shared across query surfaces.

use std::collections::HashMap;

use proximadb_data_model::DataModel;
use proximadb_observability_query::{LogQueryExpr, MetricAggregation, MetricQueryExpr};
use proximadb_proto::proximadb_v1::{LogEntry, Severity};

use crate::UnifiedRecord;

/// Default metric aggregation resolution, in seconds.
pub const DEFAULT_METRIC_STEP_SECONDS: u32 = 60;

/// Normalized log-query request for observability service callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogQueryRequest {
    /// Namespace to query.
    pub namespace: String,
    /// Start of time range (nanoseconds since epoch).
    pub start_time_ns: i64,
    /// End of time range (nanoseconds since epoch).
    pub end_time_ns: i64,
    /// Optional text query.
    pub query: Option<String>,
    /// Severity filters normalized to the protocol contract.
    pub severities: Vec<Severity>,
    /// Service filters.
    pub services: Vec<String>,
    /// Source filters.
    pub sources: Vec<String>,
    /// Result limit.
    pub limit: u32,
    /// Optional pagination cursor.
    pub cursor: Option<String>,
}

/// Normalized metric-query request for observability service callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricQueryRequest {
    /// Namespace to query.
    pub namespace: String,
    /// Metric name.
    pub metric_name: String,
    /// Start of time range (nanoseconds since epoch).
    pub start_time_ns: i64,
    /// End of time range (nanoseconds since epoch).
    pub end_time_ns: i64,
    /// Aggregation requested by the query IR.
    pub aggregation: MetricAggregation,
    /// Aggregation resolution in seconds.
    pub step_seconds: u32,
    /// Label filters.
    pub label_filters: HashMap<String, String>,
    /// Group-by label keys.
    pub group_by: Vec<String>,
}

/// Build a normalized log-query request from the cross-model observability IR.
pub fn build_log_query_request(expr: &LogQueryExpr) -> LogQueryRequest {
    let severities = expr
        .severities
        .iter()
        .filter_map(|severity| normalize_severity_filter(severity))
        .collect();

    LogQueryRequest {
        namespace: expr.namespace.clone(),
        start_time_ns: expr.start_time_ns,
        end_time_ns: expr.end_time_ns,
        query: expr.query.clone(),
        severities,
        services: expr.services.clone(),
        sources: Vec::new(),
        limit: expr.limit,
        cursor: None,
    }
}

/// Build a normalized metric-query request from the cross-model observability IR.
pub fn build_metric_query_request(expr: &MetricQueryExpr) -> MetricQueryRequest {
    MetricQueryRequest {
        namespace: expr.namespace.clone(),
        metric_name: expr.metric_name.clone(),
        start_time_ns: expr.start_time_ns,
        end_time_ns: expr.end_time_ns,
        aggregation: expr.aggregation.clone(),
        step_seconds: DEFAULT_METRIC_STEP_SECONDS,
        label_filters: expr.label_filters.clone(),
        group_by: expr.group_by.clone(),
    }
}

/// Build a unified record from an observability log entry.
pub fn build_log_record(log: &LogEntry, ordinal: usize) -> UnifiedRecord {
    let log_id = format!("log_{}_{}", log.timestamp_ns, ordinal);
    UnifiedRecord {
        id: log_id.clone(),
        source_model: DataModel::Observability,
        data: serde_json::json!({
            "id": log_id,
            "timestamp_ns": log.timestamp_ns,
            "message": log.message,
            "service": log.service,
            "severity": log.severity,
            "source": log.source,
        }),
        score: None,
        metadata: HashMap::new(),
    }
}

/// Build a unified record from an aggregated metric point.
pub fn build_metric_record(
    metric_name: &str,
    timestamp_ns: i64,
    value: f64,
    labels: HashMap<String, String>,
) -> UnifiedRecord {
    UnifiedRecord {
        id: format!("{}_{}", metric_name, timestamp_ns),
        source_model: DataModel::Observability,
        data: serde_json::json!({
            "metric": metric_name,
            "timestamp_ns": timestamp_ns,
            "value": value,
            "labels": labels.clone(),
        }),
        score: Some(value),
        metadata: labels,
    }
}

fn normalize_severity_filter(value: &str) -> Option<Severity> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "trace" => Some(Severity::Trace),
        "debug" => Some(Severity::Debug),
        "info" => Some(Severity::Info),
        "warn" | "warning" => Some(Severity::Warn),
        "error" | "err" => Some(Severity::Error),
        "fatal" | "critical" => Some(Severity::Fatal),
        "unspecified" | "default" => Some(Severity::Unspecified),
        _ => Severity::from_str_name(&normalized.to_ascii_uppercase()).or_else(|| {
            Severity::from_str_name(&format!("SEVERITY_{}", normalized.to_ascii_uppercase()))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_log_query_request_normalizes_known_severities() {
        let expr = LogQueryExpr {
            namespace: "prod".to_string(),
            start_time_ns: 10,
            end_time_ns: 20,
            query: Some("timeout".to_string()),
            severities: vec![
                "warning".to_string(),
                "SEVERITY_ERROR".to_string(),
                "unknown".to_string(),
            ],
            services: vec!["api".to_string()],
            limit: 25,
        };

        let request = build_log_query_request(&expr);
        assert_eq!(request.namespace, "prod");
        assert_eq!(request.severities, vec![Severity::Warn, Severity::Error]);
        assert_eq!(request.services, vec!["api".to_string()]);
        assert_eq!(request.limit, 25);
        assert!(request.cursor.is_none());
    }

    #[test]
    fn build_metric_query_request_preserves_requested_shape() {
        let expr = MetricQueryExpr {
            namespace: "prod".to_string(),
            metric_name: "latency_ms".to_string(),
            start_time_ns: 100,
            end_time_ns: 200,
            aggregation: MetricAggregation::P95,
            group_by: vec!["service".to_string()],
            label_filters: HashMap::from([("env".to_string(), "prod".to_string())]),
        };

        let request = build_metric_query_request(&expr);
        assert_eq!(request.metric_name, "latency_ms");
        assert_eq!(request.aggregation, MetricAggregation::P95);
        assert_eq!(request.group_by, vec!["service".to_string()]);
        assert_eq!(request.label_filters.get("env"), Some(&"prod".to_string()));
        assert_eq!(request.step_seconds, DEFAULT_METRIC_STEP_SECONDS);
    }

    #[test]
    fn build_log_record_preserves_observability_shape() {
        let log = LogEntry {
            timestamp_ns: 42,
            severity: Severity::Warn as i32,
            message: "disk almost full".to_string(),
            fields: HashMap::new(),
            source: Some("syslog".to_string()),
            service: Some("storage".to_string()),
        };

        let record = build_log_record(&log, 3);
        assert_eq!(record.id, "log_42_3");
        assert_eq!(record.data["message"], "disk almost full");
        assert_eq!(record.data["service"], "storage");
        assert_eq!(record.data["severity"], Severity::Warn as i32);
        assert!(record.score.is_none());
    }

    #[test]
    fn build_metric_record_keeps_labels_in_data_and_metadata() {
        let labels = HashMap::from([("service".to_string(), "api".to_string())]);
        let record = build_metric_record("latency_ms", 99, 123.4, labels.clone());
        assert_eq!(record.id, "latency_ms_99");
        assert_eq!(record.data["metric"], "latency_ms");
        assert_eq!(record.data["value"], 123.4);
        assert_eq!(record.metadata, labels);
        assert_eq!(record.score, Some(123.4));
    }
}
