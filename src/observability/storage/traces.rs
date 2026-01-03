// Trace storage for distributed tracing
//
// Provides:
// - Span storage and indexing
// - Trace assembly from spans
// - Service dependency graph
// - Latency analysis

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Trace span for distributed tracing
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

/// Trace storage service
pub struct TraceStorage {
    /// Base path for storage
    base_path: String,
    /// Traces indexed by trace ID
    traces: RwLock<HashMap<String, TraceData>>,
    /// Spans indexed by time
    spans_by_time: RwLock<BTreeMap<i64, Vec<String>>>,
    /// Service index (service -> trace IDs)
    service_index: RwLock<HashMap<String, Vec<String>>>,
    /// Total span count
    span_count: AtomicU64,
}

/// Data for a single trace
struct TraceData {
    /// Trace ID
    trace_id: String,
    /// All spans in this trace
    spans: Vec<TraceSpan>,
    /// Root span ID
    root_span_id: Option<String>,
    /// First timestamp
    start_time_ns: i64,
    /// Last timestamp
    end_time_ns: i64,
    /// Services involved
    services: Vec<String>,
}

impl TraceStorage {
    /// Create a new trace storage
    pub fn new(base_path: &str) -> Result<Self> {
        Ok(Self {
            base_path: base_path.to_string(),
            traces: RwLock::new(HashMap::new()),
            spans_by_time: RwLock::new(BTreeMap::new()),
            service_index: RwLock::new(HashMap::new()),
            span_count: AtomicU64::new(0),
        })
    }

    /// Write a trace span
    pub async fn write(&self, span: &TraceSpan) -> Result<()> {
        let trace_id = &span.trace_id;

        // Update trace data
        {
            let mut traces = self.traces.write().await;
            let trace = traces.entry(trace_id.clone()).or_insert_with(|| TraceData {
                trace_id: trace_id.clone(),
                spans: Vec::new(),
                root_span_id: None,
                start_time_ns: span.start_time_ns,
                end_time_ns: span.end_time_ns,
                services: Vec::new(),
            });

            // Update root span if this is a root span (no parent)
            if span.parent_span_id.is_empty() {
                trace.root_span_id = Some(span.span_id.clone());
            }

            // Update time bounds
            trace.start_time_ns = trace.start_time_ns.min(span.start_time_ns);
            trace.end_time_ns = trace.end_time_ns.max(span.end_time_ns);

            // Add service if not present
            if !trace.services.contains(&span.service_name) {
                trace.services.push(span.service_name.clone());
            }

            trace.spans.push(span.clone());
        }

        // Update time index
        {
            let mut spans_by_time = self.spans_by_time.write().await;
            spans_by_time
                .entry(span.start_time_ns)
                .or_insert_with(Vec::new)
                .push(span.span_id.clone());
        }

        // Update service index
        {
            let mut service_index = self.service_index.write().await;
            let traces = service_index
                .entry(span.service_name.clone())
                .or_insert_with(Vec::new);
            if !traces.contains(trace_id) {
                traces.push(trace_id.clone());
            }
        }

        self.span_count.fetch_add(1, Ordering::Relaxed);

        Ok(())
    }

    /// Query spans by trace ID
    pub async fn query_by_trace_id(&self, trace_id: &str) -> Result<Vec<TraceSpan>> {
        let traces = self.traces.read().await;

        if let Some(trace) = traces.get(trace_id) {
            Ok(trace.spans.clone())
        } else {
            Ok(Vec::new())
        }
    }

    /// Query traces by time range
    pub async fn query_by_time(
        &self,
        start_ns: i64,
        end_ns: i64,
        limit: usize,
    ) -> Result<Vec<TraceSummary>> {
        let traces = self.traces.read().await;

        let mut results: Vec<_> = traces
            .values()
            .filter(|t| t.start_time_ns >= start_ns && t.start_time_ns <= end_ns)
            .map(|t| TraceSummary {
                trace_id: t.trace_id.clone(),
                start_time_ns: t.start_time_ns,
                duration_ns: t.end_time_ns - t.start_time_ns,
                span_count: t.spans.len(),
                services: t.services.clone(),
                root_service: t
                    .spans
                    .iter()
                    .find(|s| s.parent_span_id.is_empty())
                    .map(|s| s.service_name.clone())
                    .unwrap_or_default(),
                root_operation: t
                    .spans
                    .iter()
                    .find(|s| s.parent_span_id.is_empty())
                    .map(|s| s.name.clone())
                    .unwrap_or_default(),
            })
            .collect();

        // Sort by start time descending
        results.sort_by(|a, b| b.start_time_ns.cmp(&a.start_time_ns));

        if results.len() > limit {
            results.truncate(limit);
        }

        Ok(results)
    }

    /// Query traces by service
    pub async fn query_by_service(
        &self,
        service: &str,
        start_ns: i64,
        end_ns: i64,
        limit: usize,
    ) -> Result<Vec<TraceSummary>> {
        let service_index = self.service_index.read().await;
        let trace_ids = service_index.get(service);

        if trace_ids.is_none() {
            return Ok(Vec::new());
        }

        let traces = self.traces.read().await;
        let mut results = Vec::new();

        for trace_id in trace_ids.unwrap() {
            if let Some(trace) = traces.get(trace_id) {
                if trace.start_time_ns >= start_ns && trace.start_time_ns <= end_ns {
                    results.push(TraceSummary {
                        trace_id: trace.trace_id.clone(),
                        start_time_ns: trace.start_time_ns,
                        duration_ns: trace.end_time_ns - trace.start_time_ns,
                        span_count: trace.spans.len(),
                        services: trace.services.clone(),
                        root_service: trace
                            .spans
                            .iter()
                            .find(|s| s.parent_span_id.is_empty())
                            .map(|s| s.service_name.clone())
                            .unwrap_or_default(),
                        root_operation: trace
                            .spans
                            .iter()
                            .find(|s| s.parent_span_id.is_empty())
                            .map(|s| s.name.clone())
                            .unwrap_or_default(),
                    });

                    if results.len() >= limit {
                        break;
                    }
                }
            }
        }

        Ok(results)
    }

    /// Get service dependency graph
    pub async fn service_dependencies(&self) -> Result<Vec<ServiceDependency>> {
        let traces = self.traces.read().await;
        let mut deps: HashMap<(String, String), u64> = HashMap::new();

        for trace in traces.values() {
            // Build parent -> child relationships
            let span_map: HashMap<_, _> =
                trace.spans.iter().map(|s| (s.span_id.clone(), s)).collect();

            for span in &trace.spans {
                if !span.parent_span_id.is_empty() {
                    if let Some(parent) = span_map.get(&span.parent_span_id) {
                        if parent.service_name != span.service_name {
                            let key = (parent.service_name.clone(), span.service_name.clone());
                            *deps.entry(key).or_insert(0) += 1;
                        }
                    }
                }
            }
        }

        let results: Vec<_> = deps
            .into_iter()
            .map(|((from, to), count)| ServiceDependency {
                source_service: from,
                target_service: to,
                call_count: count,
            })
            .collect();

        Ok(results)
    }

    /// Get total span count
    pub async fn count(&self) -> u64 {
        self.span_count.load(Ordering::Relaxed)
    }

    /// Get trace count
    pub async fn trace_count(&self) -> usize {
        self.traces.read().await.len()
    }

    /// Delete old traces
    pub async fn delete_before(&self, timestamp_ns: i64) -> Result<usize> {
        let mut traces = self.traces.write().await;
        let to_remove: Vec<_> = traces
            .iter()
            .filter(|(_, t)| t.end_time_ns < timestamp_ns)
            .map(|(k, _)| k.clone())
            .collect();

        let count = to_remove.len();
        for trace_id in to_remove {
            traces.remove(&trace_id);
        }

        Ok(count)
    }
}

/// Summary of a trace
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

/// Service dependency edge
#[derive(Debug, Clone)]
pub struct ServiceDependency {
    /// Source service
    pub source_service: String,
    /// Target service
    pub target_service: String,
    /// Number of calls
    pub call_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_span(
        trace_id: &str,
        span_id: &str,
        parent_span_id: &str,
        service: &str,
        name: &str,
        start_ns: i64,
        end_ns: i64,
    ) -> TraceSpan {
        TraceSpan {
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            parent_span_id: parent_span_id.to_string(),
            name: name.to_string(),
            service_name: service.to_string(),
            start_time_ns: start_ns,
            end_time_ns: end_ns,
            attributes: HashMap::new(),
            status: 0,
            status_message: String::new(),
        }
    }

    #[tokio::test]
    async fn test_write_and_query() {
        let storage = TraceStorage::new("/tmp/test").unwrap();

        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        storage
            .write(&make_span(
                "trace1",
                "span1",
                "",
                "svc-a",
                "op1",
                now,
                now + 1000,
            ))
            .await
            .unwrap();
        storage
            .write(&make_span(
                "trace1",
                "span2",
                "span1",
                "svc-b",
                "op2",
                now + 100,
                now + 500,
            ))
            .await
            .unwrap();

        let spans = storage.query_by_trace_id("trace1").await.unwrap();
        assert_eq!(spans.len(), 2);
    }

    #[tokio::test]
    async fn test_service_dependencies() {
        let storage = TraceStorage::new("/tmp/test").unwrap();

        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        storage
            .write(&make_span(
                "trace1",
                "span1",
                "",
                "frontend",
                "handle",
                now,
                now + 1000,
            ))
            .await
            .unwrap();
        storage
            .write(&make_span(
                "trace1",
                "span2",
                "span1",
                "backend",
                "process",
                now + 100,
                now + 500,
            ))
            .await
            .unwrap();
        storage
            .write(&make_span(
                "trace1",
                "span3",
                "span2",
                "database",
                "query",
                now + 200,
                now + 400,
            ))
            .await
            .unwrap();

        let deps = storage.service_dependencies().await.unwrap();
        assert_eq!(deps.len(), 2);
    }
}
