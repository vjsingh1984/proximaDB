//! Plain-data types shared across the observability storage seam.
//!
//! These move here (out of the root `src/observability/storage/`) so the
//! `ObservabilityStoragePort` contract ([`crate::ports`]) can name them without
//! an upward edge into the root. The root storage layer re-imports them.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Trace span for distributed tracing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceSpan {
    /// Trace ID
    pub trace_id: String,
    /// Span ID
    pub span_id: String,
    /// Parent span ID (empty for root span)
    pub parent_span_id: String,
    /// Operation name
    pub name: String,
    /// Service name
    pub service_name: String,
    /// Start time in nanoseconds
    pub start_time_ns: i64,
    /// End time in nanoseconds
    pub end_time_ns: i64,
    /// Span attributes
    pub attributes: HashMap<String, String>,
    /// Status code (0 = OK, non-zero = error)
    pub status: i32,
    /// Status message
    pub status_message: String,
}

/// Summary of a trace.
#[derive(Debug, Clone)]
pub struct TraceSummary {
    /// Trace ID
    pub trace_id: String,
    /// Start time
    pub start_time_ns: i64,
    /// Total duration
    pub duration_ns: i64,
    /// Number of spans
    pub span_count: usize,
    /// Services involved
    pub services: Vec<String>,
    /// Root service name
    pub root_service: String,
    /// Root operation name
    pub root_operation: String,
}

/// Statistics about a namespace's storage usage — counts of logs, metric series,
/// and traces plus total bytes.
#[derive(Debug, Clone)]
pub struct ObservabilityStorageStats {
    /// Number of log entries
    pub log_count: u64,
    /// Number of metric series
    pub metric_series_count: u64,
    /// Number of traces
    pub trace_count: u64,
    /// Total storage bytes
    pub total_bytes: u64,
}
