// Trace query engine
//
// Provides:
// - Trace search and filtering
// - Span analysis
// - Latency analysis
// - Error analysis

use std::collections::HashMap;

use crate::observability::storage::traces::TraceSpan;

/// Trace query builder
pub struct TraceQueryBuilder {
    /// Time range start
    start_time_ns: Option<i64>,
    /// Time range end
    end_time_ns: Option<i64>,
    /// Service filter
    services: Vec<String>,
    /// Operation filter
    operations: Vec<String>,
    /// Minimum duration filter
    min_duration_ns: Option<i64>,
    /// Maximum duration filter
    max_duration_ns: Option<i64>,
    /// Error only filter
    errors_only: bool,
    /// Attribute filters
    attributes: HashMap<String, String>,
    /// Limit
    limit: usize,
}

impl TraceQueryBuilder {
    /// Create a new query builder
    pub fn new() -> Self {
        Self {
            start_time_ns: None,
            end_time_ns: None,
            services: Vec::new(),
            operations: Vec::new(),
            min_duration_ns: None,
            max_duration_ns: None,
            errors_only: false,
            attributes: HashMap::new(),
            limit: 100,
        }
    }

    /// Set time range
    pub fn time_range(mut self, start_ns: i64, end_ns: i64) -> Self {
        self.start_time_ns = Some(start_ns);
        self.end_time_ns = Some(end_ns);
        self
    }

    /// Filter by service
    pub fn service(mut self, service: &str) -> Self {
        self.services.push(service.to_string());
        self
    }

    /// Filter by operation
    pub fn operation(mut self, operation: &str) -> Self {
        self.operations.push(operation.to_string());
        self
    }

    /// Set minimum duration filter
    pub fn min_duration_ns(mut self, duration_ns: i64) -> Self {
        self.min_duration_ns = Some(duration_ns);
        self
    }

    /// Set maximum duration filter
    pub fn max_duration_ns(mut self, duration_ns: i64) -> Self {
        self.max_duration_ns = Some(duration_ns);
        self
    }

    /// Only show traces with errors
    pub fn errors_only(mut self) -> Self {
        self.errors_only = true;
        self
    }

    /// Add attribute filter
    pub fn attribute(mut self, key: &str, value: &str) -> Self {
        self.attributes.insert(key.to_string(), value.to_string());
        self
    }

    /// Set result limit
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Build the query
    pub fn build(self) -> TraceQuery {
        TraceQuery {
            start_time_ns: self.start_time_ns.unwrap_or(0),
            end_time_ns: self.end_time_ns.unwrap_or(i64::MAX),
            services: self.services,
            operations: self.operations,
            min_duration_ns: self.min_duration_ns,
            max_duration_ns: self.max_duration_ns,
            errors_only: self.errors_only,
            attributes: self.attributes,
            limit: self.limit,
        }
    }
}

impl Default for TraceQueryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Compiled trace query
#[derive(Debug, Clone)]
pub struct TraceQuery {
    /// Time range start
    pub start_time_ns: i64,
    /// Time range end
    pub end_time_ns: i64,
    /// Service filter
    pub services: Vec<String>,
    /// Operation filter
    pub operations: Vec<String>,
    /// Minimum duration filter
    pub min_duration_ns: Option<i64>,
    /// Maximum duration filter
    pub max_duration_ns: Option<i64>,
    /// Error only filter
    pub errors_only: bool,
    /// Attribute filters
    pub attributes: HashMap<String, String>,
    /// Limit
    pub limit: usize,
}

impl TraceQuery {
    /// Check if a span matches this query
    pub fn matches_span(&self, span: &TraceSpan) -> bool {
        // Time filter
        if span.start_time_ns < self.start_time_ns || span.start_time_ns > self.end_time_ns {
            return false;
        }

        // Service filter
        if !self.services.is_empty() && !self.services.contains(&span.service_name) {
            return false;
        }

        // Operation filter
        if !self.operations.is_empty() && !self.operations.contains(&span.name) {
            return false;
        }

        // Duration filters
        let duration = span.end_time_ns - span.start_time_ns;
        if let Some(min) = self.min_duration_ns {
            if duration < min {
                return false;
            }
        }
        if let Some(max) = self.max_duration_ns {
            if duration > max {
                return false;
            }
        }

        // Error filter (status != 0 indicates error)
        if self.errors_only && span.status == 0 {
            return false;
        }

        // Attribute filters
        for (key, value) in &self.attributes {
            match span.attributes.get(key) {
                Some(v) if v == value => {}
                _ => return false,
            }
        }

        true
    }
}

/// Trace analysis result
#[derive(Debug, Clone)]
pub struct TraceAnalysis {
    /// Trace ID
    pub trace_id: String,
    /// Total duration
    pub duration_ns: i64,
    /// Number of spans
    pub span_count: usize,
    /// Services involved
    pub services: Vec<String>,
    /// Has errors
    pub has_errors: bool,
    /// Critical path spans
    pub critical_path: Vec<CriticalPathSpan>,
    /// Span breakdown by service
    pub service_breakdown: Vec<ServiceBreakdown>,
}

/// Critical path span info
#[derive(Debug, Clone)]
pub struct CriticalPathSpan {
    /// Span ID
    pub span_id: String,
    /// Service name
    pub service_name: String,
    /// Operation name
    pub operation_name: String,
    /// Duration
    pub duration_ns: i64,
    /// Percentage of total trace duration
    pub percentage: f64,
}

/// Service breakdown
#[derive(Debug, Clone)]
pub struct ServiceBreakdown {
    /// Service name
    pub service_name: String,
    /// Total time spent in this service
    pub total_duration_ns: i64,
    /// Number of spans
    pub span_count: usize,
    /// Percentage of total trace duration
    pub percentage: f64,
}

/// Trace analyzer
pub struct TraceAnalyzer;

impl TraceAnalyzer {
    /// Analyze a trace
    pub fn analyze(spans: &[TraceSpan]) -> Option<TraceAnalysis> {
        if spans.is_empty() {
            return None;
        }

        let trace_id = spans[0].trace_id.clone();

        // Calculate duration from root span
        let root_span = spans.iter().find(|s| s.parent_span_id.is_empty())?;
        let duration_ns = root_span.end_time_ns - root_span.start_time_ns;

        // Collect services
        let mut services: Vec<String> = spans.iter().map(|s| s.service_name.clone()).collect();
        services.sort();
        services.dedup();

        // Check for errors
        let has_errors = spans.iter().any(|s| s.status != 0);

        // Calculate service breakdown
        let mut service_durations: HashMap<String, (i64, usize)> = HashMap::new();
        for span in spans {
            let span_duration = span.end_time_ns - span.start_time_ns;
            let entry = service_durations
                .entry(span.service_name.clone())
                .or_insert((0, 0));
            entry.0 += span_duration;
            entry.1 += 1;
        }

        let service_breakdown: Vec<_> = service_durations
            .into_iter()
            .map(|(service, (dur, count))| ServiceBreakdown {
                service_name: service,
                total_duration_ns: dur,
                span_count: count,
                percentage: if duration_ns > 0 {
                    (dur as f64 / duration_ns as f64) * 100.0
                } else {
                    0.0
                },
            })
            .collect();

        // Calculate critical path (simplified: just the longest spans)
        let mut sorted_spans: Vec<_> = spans.iter().collect();
        sorted_spans.sort_by_key(|s| -(s.end_time_ns - s.start_time_ns));

        let critical_path: Vec<_> = sorted_spans
            .iter()
            .take(5)
            .map(|s| {
                let span_dur = s.end_time_ns - s.start_time_ns;
                CriticalPathSpan {
                    span_id: s.span_id.clone(),
                    service_name: s.service_name.clone(),
                    operation_name: s.name.clone(),
                    duration_ns: span_dur,
                    percentage: if duration_ns > 0 {
                        (span_dur as f64 / duration_ns as f64) * 100.0
                    } else {
                        0.0
                    },
                }
            })
            .collect();

        Some(TraceAnalysis {
            trace_id,
            duration_ns,
            span_count: spans.len(),
            services,
            has_errors,
            critical_path,
            service_breakdown,
        })
    }
}

/// Latency analysis
#[derive(Debug, Clone)]
pub struct LatencyAnalysis {
    /// Service name
    pub service_name: String,
    /// Operation name
    pub operation_name: String,
    /// Sample count
    pub count: usize,
    /// Minimum latency
    pub min_ns: i64,
    /// Maximum latency
    pub max_ns: i64,
    /// Average latency
    pub avg_ns: i64,
    /// P50 latency
    pub p50_ns: i64,
    /// P95 latency
    pub p95_ns: i64,
    /// P99 latency
    pub p99_ns: i64,
}

impl LatencyAnalysis {
    /// Calculate latency analysis from spans
    pub fn from_spans(spans: &[TraceSpan]) -> Self {
        if spans.is_empty() {
            return Self {
                service_name: String::new(),
                operation_name: String::new(),
                count: 0,
                min_ns: 0,
                max_ns: 0,
                avg_ns: 0,
                p50_ns: 0,
                p95_ns: 0,
                p99_ns: 0,
            };
        }

        let service_name = spans[0].service_name.clone();
        let operation_name = spans[0].name.clone();

        let mut latencies: Vec<i64> = spans
            .iter()
            .map(|s| s.end_time_ns - s.start_time_ns)
            .collect();
        latencies.sort();

        let count = latencies.len();
        let min_ns = *latencies.first().unwrap_or(&0);
        let max_ns = *latencies.last().unwrap_or(&0);
        let avg_ns = latencies.iter().sum::<i64>() / count as i64;

        let p50_ns = latencies[((count as f64 * 0.50) as usize).min(count - 1)];
        let p95_ns = latencies[((count as f64 * 0.95) as usize).min(count - 1)];
        let p99_ns = latencies[((count as f64 * 0.99) as usize).min(count - 1)];

        Self {
            service_name,
            operation_name,
            count,
            min_ns,
            max_ns,
            avg_ns,
            p50_ns,
            p95_ns,
            p99_ns,
        }
    }
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

    #[test]
    fn test_query_builder() {
        let query = TraceQueryBuilder::new()
            .service("api")
            .operation("handle_request")
            .min_duration_ns(1_000_000)
            .limit(50)
            .build();

        assert_eq!(query.services, vec!["api"]);
        assert_eq!(query.operations, vec!["handle_request"]);
        assert_eq!(query.min_duration_ns, Some(1_000_000));
        assert_eq!(query.limit, 50);
    }

    #[test]
    fn test_query_matches_span() {
        let query = TraceQueryBuilder::new()
            .service("api")
            .min_duration_ns(100)
            .build();

        let span1 = make_span("t1", "s1", "", "api", "handle", 0, 200);
        let span2 = make_span("t1", "s2", "s1", "db", "query", 50, 150);
        let span3 = make_span("t1", "s3", "", "api", "handle", 0, 50);

        assert!(query.matches_span(&span1));
        assert!(!query.matches_span(&span2)); // Wrong service
        assert!(!query.matches_span(&span3)); // Duration too short
    }

    #[test]
    fn test_trace_analysis() {
        let spans = vec![
            make_span("t1", "s1", "", "frontend", "handle", 0, 1000),
            make_span("t1", "s2", "s1", "backend", "process", 100, 800),
            make_span("t1", "s3", "s2", "database", "query", 200, 600),
        ];

        let analysis = TraceAnalyzer::analyze(&spans).unwrap();
        assert_eq!(analysis.trace_id, "t1");
        assert_eq!(analysis.duration_ns, 1000);
        assert_eq!(analysis.span_count, 3);
        assert_eq!(analysis.services.len(), 3);
    }

    #[test]
    fn test_latency_analysis() {
        let spans = vec![
            make_span("t1", "s1", "", "api", "handle", 0, 100),
            make_span("t2", "s2", "", "api", "handle", 0, 200),
            make_span("t3", "s3", "", "api", "handle", 0, 150),
        ];

        let analysis = LatencyAnalysis::from_spans(&spans);
        assert_eq!(analysis.count, 3);
        assert_eq!(analysis.min_ns, 100);
        assert_eq!(analysis.max_ns, 200);
        assert_eq!(analysis.avg_ns, 150);
    }
}
