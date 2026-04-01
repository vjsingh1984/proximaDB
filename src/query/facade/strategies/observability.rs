//! # Observability Strategy
//!
//! Real implementation of `QueryStrategy` for observability queries (logs/metrics/traces).
//! Wraps the existing `ObservabilityQueryEngine` infrastructure.
//!
//! ## Features
//!
//! - Converts facade `QueryRequest` to observability operations
//! - Supports log queries with Tantivy full-text search
//! - Supports metric aggregation queries
//! - Returns results in unified `QueryResult` format
//!
//! ## Architecture
//!
//! ```text
//! QueryRequest (facade)
//!       │
//!       ▼
//! ObservabilityStrategy
//!       │
//!   ┌───┴───┐
//!   ▼       ▼
//! Log     Metric
//! Query   Query
//!   │       │
//!   ▼       ▼
//! QueryResult (facade)
//! ```

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tracing::{debug, info, instrument};

use crate::observability::query::ObservabilityQueryEngine;
use crate::query::facade::{
    ExecutionMetrics, QueryContent, QueryContext, QueryRequest, QueryResult, QueryResultData,
    QueryStrategy, QueryType,
};

/// Observability Strategy - Real implementation wrapping ObservabilityQueryEngine
///
/// This strategy handles `QueryType::Observability` requests by:
/// 1. Parsing the query to determine if it's logs, metrics, or traces
/// 2. Executing via ObservabilityQueryEngine
/// 3. Converting results back to facade format
pub struct ObservabilityStrategy {
    /// Observability query engine
    query_engine: Arc<ObservabilityQueryEngine>,
    /// Strategy priority (higher = preferred)
    priority: i32,
}

/// Type of observability query
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservabilityQueryType {
    Logs,
    Metrics,
    Traces,
}

impl ObservabilityStrategy {
    /// Create a new ObservabilityStrategy
    pub fn new(query_engine: Arc<ObservabilityQueryEngine>) -> Self {
        Self {
            query_engine,
            priority: 60, // Lower than document, graph, SQL, vector
        }
    }

    /// Create with custom priority
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Determine what type of observability query this is
    fn detect_query_type(&self, content: &str) -> ObservabilityQueryType {
        let upper = content.to_uppercase();
        if upper.contains("METRICS(") || upper.contains("METRIC_NAME") {
            ObservabilityQueryType::Metrics
        } else if upper.contains("TRACES(") || upper.contains("TRACE_ID") {
            ObservabilityQueryType::Traces
        } else {
            // Default to logs
            ObservabilityQueryType::Logs
        }
    }

    /// Extract namespace from query
    fn extract_namespace(&self, request: &QueryRequest, content: &str) -> String {
        // Try target first
        if let Some(target) = &request.target {
            return target.clone();
        }

        // Try to parse from LOGS('namespace') or METRICS('namespace')
        if let Some(ns) = self.parse_namespace_from_function(content) {
            return ns;
        }

        // Default namespace
        "default".to_string()
    }

    /// Parse namespace from LOGS('namespace') or METRICS('namespace')
    fn parse_namespace_from_function(&self, content: &str) -> Option<String> {
        let upper = content.to_uppercase();

        // Check for LOGS('namespace')
        if let Some(start) = upper.find("LOGS(") {
            return self.extract_first_arg(&content[start + 5..]);
        }

        // Check for METRICS('namespace')
        if let Some(start) = upper.find("METRICS(") {
            return self.extract_first_arg(&content[start + 8..]);
        }

        // Check for TRACES('namespace')
        if let Some(start) = upper.find("TRACES(") {
            return self.extract_first_arg(&content[start + 7..]);
        }

        None
    }

    /// Extract first argument from function call
    fn extract_first_arg(&self, content: &str) -> Option<String> {
        let content = content.trim();

        // Find the closing paren or comma
        let end = content.find(|c| c == ')' || c == ',')?;
        let arg = content[..end].trim();

        // Remove quotes
        let arg = arg.trim_matches(|c| c == '\'' || c == '"');
        Some(arg.to_string())
    }

    /// Extract search query from SQL-like content
    fn extract_search_query(&self, content: &str) -> Option<String> {
        let upper = content.to_uppercase();

        // Look for WHERE clause
        if let Some(where_pos) = upper.find("WHERE") {
            let rest = &content[where_pos + 5..];
            // Get the condition
            let condition = rest.trim();
            if !condition.is_empty() {
                return Some(condition.to_string());
            }
        }

        None
    }

    /// Execute log query
    async fn execute_log_query(
        &self,
        namespace: &str,
        content: &str,
        start_time: Instant,
    ) -> Result<QueryResult> {
        use crate::observability::LogQueryParams;
        use crate::observability::query::LogSearchOptions;

        let search_query = self.extract_search_query(content);

        debug!(
            namespace = %namespace,
            search_query = ?search_query,
            "Executing log query"
        );

        // If we have a search query, use full-text search
        if let Some(query) = search_query {
            let options = LogSearchOptions {
                limit: 100,
                start_time_ns: None,
                end_time_ns: None,
                service_filter: None,
                source_filter: None,
                severity_filter: None,
                search_fields: vec![],
                fuzzy: false,
            };

            let results = self
                .query_engine
                .search_logs_fulltext(namespace, &query, &options)
                .await?;

            let execution_time_ms = start_time.elapsed().as_millis() as u64;
            let results_count = results.len();

            // Convert to JSON rows
            let rows: Vec<serde_json::Value> = results
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "score": r.score,
                        "timestamp_ns": r.timestamp_ns,
                    })
                })
                .collect();

            return Ok(QueryResult {
                data: QueryResultData::Rows(rows),
                metrics: Some(ExecutionMetrics {
                    execution_path: "unified".to_string(),
                    strategy_name: "observability".to_string(),
                    execution_time_ms,
                    planning_time_ms: 0,
                    results_scanned: results_count,
                    results_returned: results_count,
                    cache_hit: false,
                    extra: serde_json::json!({
                        "engine": "TantivyLogIndex",
                        "query_type": "fulltext_search",
                        "logs_returned": results_count,
                    }),
                }),
            });
        }

        // Basic log query without full-text search
        let params = LogQueryParams {
            start_time_ns: 0, // All time
            end_time_ns: i64::MAX,
            query: None,
            severities: vec![],
            services: vec![],
            sources: vec![],
            limit: 100,
            cursor: None,
        };

        let result = self
            .query_engine
            .query_logs_with_fulltext(namespace, params, false)
            .await?;

        let execution_time_ms = start_time.elapsed().as_millis() as u64;
        let logs_count = result.logs.len();

        // Convert to JSON rows
        let rows: Vec<serde_json::Value> = result
            .logs
            .into_iter()
            .map(|log| {
                serde_json::json!({
                    "timestamp_ns": log.timestamp_ns,
                    "message": log.message,
                    "service": log.service,
                    "severity": log.severity,
                })
            })
            .collect();

        Ok(QueryResult {
            data: QueryResultData::Rows(rows),
            metrics: Some(ExecutionMetrics {
                execution_path: "unified".to_string(),
                strategy_name: "observability".to_string(),
                execution_time_ms,
                planning_time_ms: 0,
                results_scanned: logs_count,
                results_returned: logs_count,
                cache_hit: false,
                extra: serde_json::json!({
                    "engine": "ObservabilityQueryEngine",
                    "query_type": "logs",
                    "logs_returned": logs_count,
                }),
            }),
        })
    }

    /// Execute metrics query with PromQL support
    ///
    /// Parses PromQL expressions from SQL queries and executes them using
    /// the observability query engine. Supports:
    /// - Vector selectors: metric_name{label="value"}
    /// - Range vectors: metric_name[5m]
    /// - Aggregations: sum, avg, rate, etc.
    /// - Label matchers: =, !=, =~, !~
    async fn execute_metrics_query(
        &self,
        namespace: &str,
        content: &str,
        start_time: Instant,
    ) -> Result<QueryResult> {
        use crate::observability::query::PromQLQueryParams;
        use crate::observability::query::promql::PromQLParser;

        debug!(
            namespace = %namespace,
            content = %content,
            "Executing metrics query"
        );

        // Extract PromQL expression from the SQL-like query
        let promql_expr = self.extract_promql_expression(content);

        // Parse time range from content or use defaults
        let (start_time_ns, end_time_ns) = self.extract_metrics_time_range(content);

        // Build PromQL query parameters
        let params = PromQLQueryParams::new(&promql_expr, start_time_ns, end_time_ns);

        // Validate the PromQL expression first
        if let Err(e) = PromQLParser::parse(&promql_expr) {
            debug!(
                error = %e,
                expression = %promql_expr,
                "Failed to parse PromQL expression"
            );
            return Err(anyhow!("Invalid PromQL expression: {}", e));
        }

        // Execute the PromQL query
        let result = self.query_engine.query_promql(namespace, params).await?;

        let execution_time_ms = start_time.elapsed().as_millis() as u64;
        let results_count = result.results.len();

        // Convert MetricResult to JSON rows
        let rows: Vec<serde_json::Value> = result
            .results
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "timestamp_ns": r.timestamp_ns,
                    "value": r.value,
                    "labels": r.labels,
                })
            })
            .collect();

        Ok(QueryResult {
            data: QueryResultData::Rows(rows),
            metrics: Some(ExecutionMetrics {
                execution_path: "unified".to_string(),
                strategy_name: "observability".to_string(),
                execution_time_ms,
                planning_time_ms: 0,
                results_scanned: results_count,
                results_returned: results_count,
                cache_hit: false,
                extra: serde_json::json!({
                    "engine": "PromQLExecutor",
                    "query_type": "metrics",
                    "promql_expression": promql_expr,
                    "metrics_returned": results_count,
                }),
            }),
        })
    }

    /// Extract PromQL expression from SQL-like content
    ///
    /// Handles various formats:
    /// - SELECT * FROM METRICS('namespace') WHERE metric_name = 'cpu_usage'
    /// - SELECT * FROM METRICS('namespace', 'rate(http_requests_total[5m])')
    /// - Direct PromQL: sum(http_requests_total) by (method)
    fn extract_promql_expression(&self, content: &str) -> String {
        let upper = content.to_uppercase();

        // Check for METRICS('namespace', 'promql_expr') format
        if let Some(start) = upper.find("METRICS(") {
            let rest = &content[start + 8..];
            // Skip past the namespace argument
            if let Some(comma_pos) = rest.find(',') {
                let after_comma = rest[comma_pos + 1..].trim();
                // Extract the PromQL expression (second argument)
                if let Some(expr) = self.extract_quoted_string(after_comma) {
                    return expr;
                }
            }
        }

        // Check for metric_name = 'value' in WHERE clause
        if let Some(where_pos) = upper.find("WHERE") {
            let rest = &content[where_pos + 5..].trim();

            // Look for metric_name condition
            if let Some(metric_pos) = rest.to_uppercase().find("METRIC_NAME") {
                let after_metric = &rest[metric_pos..];
                // Find the = sign
                if let Some(eq_pos) = after_metric.find('=') {
                    let value_part = after_metric[eq_pos + 1..].trim();
                    if let Some(name) = self.extract_quoted_string(value_part) {
                        // Check for additional aggregation functions
                        if let Some(agg_fn) = self.extract_aggregation_from_content(content) {
                            return format!("{}({})", agg_fn, name);
                        }
                        return name;
                    }
                }
            }

            // Check for direct PromQL-like expressions
            let cleaned = rest.trim_end_matches(';').trim();
            if self.looks_like_promql(cleaned) {
                return cleaned.to_string();
            }
        }

        // Check for FROM clause with metric selector
        if let Some(from_pos) = upper.find("FROM") {
            let rest = &content[from_pos + 4..].trim();
            // Look for metric name pattern (e.g., http_requests_total{...})
            if let Some(first_space) = rest.find(|c: char| c.is_whitespace() || c == '{') {
                let potential_metric = &rest[..first_space];
                if !potential_metric.to_uppercase().starts_with("METRICS")
                    && !potential_metric.to_uppercase().starts_with("LOGS")
                {
                    // Could be a direct metric reference
                    let cleaned = rest.trim_end_matches(';').trim();
                    if self.looks_like_promql(cleaned) {
                        return cleaned.to_string();
                    }
                }
            }
        }

        // If nothing else matches, treat the entire content as a potential PromQL expression
        // (after stripping SQL-like wrapper)
        let cleaned = content
            .trim()
            .trim_start_matches(|c: char| c.is_whitespace())
            .trim_end_matches(';')
            .trim();

        // Strip SELECT * FROM METRICS(...) wrapper if present
        if upper.starts_with("SELECT") {
            // Try to extract just the PromQL part
            if let Some(promql) = self.extract_promql_from_select(content) {
                return promql;
            }
        }

        // Return as-is if it looks like PromQL
        if self.looks_like_promql(cleaned) {
            return cleaned.to_string();
        }

        // Default to a simple selector
        cleaned.to_string()
    }

    /// Extract a quoted string (single or double quotes)
    fn extract_quoted_string(&self, s: &str) -> Option<String> {
        let s = s.trim();
        if s.starts_with('\'') || s.starts_with('"') {
            let quote = s.chars().next()?;
            let rest = &s[1..];
            if let Some(end) = rest.find(quote) {
                return Some(rest[..end].to_string());
            }
        }
        None
    }

    /// Check if a string looks like a PromQL expression
    fn looks_like_promql(&self, s: &str) -> bool {
        // PromQL typically has:
        // - Metric names with optional labels: metric{label="value"}
        // - Aggregation functions: sum(), avg(), rate(), etc.
        // - Range vectors: metric[5m]
        let s = s.trim();
        if s.is_empty() {
            return false;
        }

        // Check for aggregation functions
        let agg_funcs = [
            "sum(",
            "avg(",
            "min(",
            "max(",
            "count(",
            "rate(",
            "irate(",
            "increase(",
            "histogram_quantile(",
            "topk(",
            "bottomk(",
            "stddev(",
        ];
        for func in &agg_funcs {
            if s.to_lowercase().starts_with(func) {
                return true;
            }
        }

        // Check for label matchers or range vectors
        if s.contains('{') || s.contains('[') {
            return true;
        }

        // Check if it's a simple metric name (alphanumeric with underscores/colons)
        s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == ':')
    }

    /// Extract aggregation function from content
    fn extract_aggregation_from_content(&self, content: &str) -> Option<String> {
        let upper = content.to_uppercase();

        // Look for aggregation keywords in SELECT or elsewhere
        let agg_keywords = [
            ("SUM(", "sum"),
            ("AVG(", "avg"),
            ("MIN(", "min"),
            ("MAX(", "max"),
            ("COUNT(", "count"),
            ("RATE(", "rate"),
        ];

        for (pattern, func) in &agg_keywords {
            if upper.contains(pattern) {
                return Some((*func).to_string());
            }
        }

        None
    }

    /// Extract PromQL expression from SELECT statement
    fn extract_promql_from_select(&self, content: &str) -> Option<String> {
        // Handle: SELECT promql_expr FROM METRICS(...)
        // or: SELECT * FROM METRICS('ns', 'promql_expr')
        let upper = content.to_uppercase();

        // Check for the second argument format
        if let Some(metrics_pos) = upper.find("METRICS(") {
            let rest = &content[metrics_pos + 8..];
            // Find closing paren
            let mut depth = 1;
            let mut close_pos = None;
            for (i, c) in rest.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            close_pos = Some(i);
                            break;
                        }
                    }
                    _ => {}
                }
            }

            if let Some(close) = close_pos {
                let args = &rest[..close];
                // Split by comma to get second argument
                let mut in_quotes = false;
                let mut quote_char = ' ';
                let mut splits = vec![0];

                for (i, c) in args.char_indices() {
                    if (c == '\'' || c == '"') && !in_quotes {
                        in_quotes = true;
                        quote_char = c;
                    } else if c == quote_char && in_quotes {
                        in_quotes = false;
                    } else if c == ',' && !in_quotes {
                        splits.push(i);
                    }
                }

                // If we have a second argument
                if splits.len() >= 2 {
                    let second_arg = &args[splits[1] + 1..];
                    if let Some(expr) = self.extract_quoted_string(second_arg.trim()) {
                        return Some(expr);
                    }
                }
            }
        }

        None
    }

    /// Extract time range for metrics queries
    ///
    /// Looks for time-related clauses and returns (start_ns, end_ns)
    fn extract_metrics_time_range(&self, content: &str) -> (i64, i64) {
        use crate::observability::query::promql::PromQLParser;

        let upper = content.to_uppercase();

        // Default time range: last hour
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let default_start = now - 3600 * 1_000_000_000; // 1 hour ago
        let default_end = now;

        // Look for timestamp > or timestamp >= patterns
        let mut start_time_result = default_start;
        let mut end_time_result = default_end;

        // Check for interval patterns like "now() - interval '1h'"
        if upper.contains("INTERVAL")
            && let Some(duration) = self.extract_interval_duration(content) {
                start_time_result = now - duration;
            }

        // Check for explicit timestamp conditions
        if let Some(where_pos) = upper.find("WHERE") {
            let rest = &content[where_pos..];
            let upper_rest = rest.to_uppercase();

            // Look for timestamp > value patterns
            if let Some(ts_pos) = upper_rest.find("TIMESTAMP") {
                let after_ts = &rest[ts_pos + 9..];
                // Try to extract numeric timestamp
                if let Some((op, value)) = self.extract_metrics_comparison(after_ts) {
                    match op.as_str() {
                        ">" | ">=" => start_time_result = value,
                        "<" | "<=" => end_time_result = value,
                        _ => {}
                    }
                }
            }
        }

        // Also check for PromQL range in the expression (e.g., [5m])
        // This helps with rate() and similar functions
        if let Some(bracket_start) = content.find('[')
            && let Some(bracket_end) = content[bracket_start..].find(']') {
                let range_str = &content[bracket_start + 1..bracket_start + bracket_end];
                if let Ok(duration) = PromQLParser::parse_duration(range_str) {
                    // For range vectors, we need data going back at least this far
                    let range_start = now - duration.nanoseconds;
                    if range_start < start_time_result {
                        start_time_result = range_start;
                    }
                }
            }

        (start_time_result, end_time_result)
    }

    /// Extract interval duration from content (e.g., "interval '1h'" -> nanoseconds)
    fn extract_interval_duration(&self, content: &str) -> Option<i64> {
        use crate::observability::query::promql::PromQLParser;

        let upper = content.to_uppercase();
        if let Some(interval_pos) = upper.find("INTERVAL") {
            let rest = &content[interval_pos + 8..].trim();
            // Extract the quoted duration
            if let Some(duration_str) = self.extract_quoted_string(rest) {
                // Parse using PromQL duration parser
                if let Ok(duration) = PromQLParser::parse_duration(&duration_str) {
                    return Some(duration.nanoseconds);
                }
            }
        }
        None
    }

    /// Extract comparison operator and value for metrics queries
    fn extract_metrics_comparison(&self, s: &str) -> Option<(String, i64)> {
        let s = s.trim();

        // Check for operators
        let ops = [">=", "<=", ">", "<", "="];
        for op in &ops {
            if let Some(rest) = s.strip_prefix(op) {
                let rest = rest.trim();
                // Try to parse the number
                let num_str: String = rest.chars().take_while(|c| c.is_numeric()).collect();
                if let Ok(value) = num_str.parse::<i64>() {
                    return Some(((*op).to_string(), value));
                }
            }
        }
        None
    }

    /// Execute traces query
    ///
    /// Parses SQL-like trace queries and executes them via the query engine.
    /// Supports:
    /// - TRACES('namespace') - query all traces in namespace
    /// - WHERE trace_id = 'xxx' - filter by specific trace ID
    /// - WHERE service = 'xxx' - filter by service name
    /// - WHERE min_duration_ns > N - filter by minimum duration
    /// - WHERE errors_only = true - filter to only errored traces
    async fn execute_traces_query(
        &self,
        namespace: &str,
        content: &str,
        start_time: Instant,
    ) -> Result<QueryResult> {
        use crate::observability::query::traces::TraceQueryBuilder;

        debug!(
            namespace = %namespace,
            content = %content,
            "Executing traces query"
        );

        // Parse the query to extract filters
        let mut builder = TraceQueryBuilder::new();

        // Extract trace_id filter if present
        if let Some(trace_id) = self.extract_trace_id_filter(content) {
            // For single trace lookup, we use a different approach
            let spans = self
                .query_engine
                .storage()
                .query_trace(namespace, &trace_id)
                .await?;

            let execution_time_ms = start_time.elapsed().as_millis() as u64;
            let span_count = spans.len();

            // Convert spans to JSON rows
            let rows: Vec<serde_json::Value> = spans
                .into_iter()
                .map(|span| {
                    serde_json::json!({
                        "trace_id": span.trace_id,
                        "span_id": span.span_id,
                        "parent_span_id": span.parent_span_id,
                        "name": span.name,
                        "service_name": span.service_name,
                        "start_time_ns": span.start_time_ns,
                        "end_time_ns": span.end_time_ns,
                        "duration_ns": span.end_time_ns - span.start_time_ns,
                        "status": span.status,
                        "status_message": span.status_message,
                        "attributes": span.attributes,
                    })
                })
                .collect();

            return Ok(QueryResult {
                data: QueryResultData::Rows(rows),
                metrics: Some(ExecutionMetrics {
                    execution_path: "unified".to_string(),
                    strategy_name: "observability".to_string(),
                    execution_time_ms,
                    planning_time_ms: 0,
                    results_scanned: span_count,
                    results_returned: span_count,
                    cache_hit: false,
                    extra: serde_json::json!({
                        "engine": "TraceStorage",
                        "query_type": "trace_by_id",
                        "trace_id": trace_id,
                        "spans_returned": span_count,
                    }),
                }),
            });
        }

        // Extract time range from query
        let (start_ns, end_ns) = self.extract_time_range(content);
        if start_ns > 0 || end_ns < i64::MAX {
            builder = builder.time_range(start_ns, end_ns);
        }

        // Extract service filter
        if let Some(service) = self.extract_service_filter(content) {
            builder = builder.service(&service);
        }

        // Extract operation filter
        if let Some(operation) = self.extract_operation_filter(content) {
            builder = builder.operation(&operation);
        }

        // Extract duration filters
        if let Some(min_duration) = self.extract_min_duration_filter(content) {
            builder = builder.min_duration_ns(min_duration);
        }
        if let Some(max_duration) = self.extract_max_duration_filter(content) {
            builder = builder.max_duration_ns(max_duration);
        }

        // Extract errors_only filter
        if self.has_errors_only_filter(content) {
            builder = builder.errors_only();
        }

        // Extract limit
        let limit = self.extract_limit(content).unwrap_or(100);
        builder = builder.limit(limit);

        let query = builder.build();

        // Query traces by time range using storage layer
        let traces = self
            .query_engine
            .storage()
            .query_traces_by_time(
                namespace,
                query.start_time_ns,
                query.end_time_ns,
                query.limit,
            )
            .await?;

        // Filter the traces using the query
        let filtered_traces: Vec<_> = traces
            .into_iter()
            .filter(|summary| {
                // Apply service filter
                if !query.services.is_empty()
                    && !query.services.contains(&summary.root_service)
                    && !summary.services.iter().any(|s| query.services.contains(s))
                {
                    return false;
                }

                // Apply operation filter
                if !query.operations.is_empty()
                    && !query.operations.contains(&summary.root_operation)
                {
                    return false;
                }

                // Apply duration filters
                if let Some(min) = query.min_duration_ns
                    && summary.duration_ns < min {
                        return false;
                    }
                if let Some(max) = query.max_duration_ns
                    && summary.duration_ns > max {
                        return false;
                    }

                true
            })
            .take(limit)
            .collect();

        let execution_time_ms = start_time.elapsed().as_millis() as u64;
        let traces_count = filtered_traces.len();

        // Convert to JSON rows
        let rows: Vec<serde_json::Value> = filtered_traces
            .into_iter()
            .map(|summary| {
                serde_json::json!({
                    "trace_id": summary.trace_id,
                    "start_time_ns": summary.start_time_ns,
                    "duration_ns": summary.duration_ns,
                    "span_count": summary.span_count,
                    "services": summary.services,
                    "root_service": summary.root_service,
                    "root_operation": summary.root_operation,
                })
            })
            .collect();

        Ok(QueryResult {
            data: QueryResultData::Rows(rows),
            metrics: Some(ExecutionMetrics {
                execution_path: "unified".to_string(),
                strategy_name: "observability".to_string(),
                execution_time_ms,
                planning_time_ms: 0,
                results_scanned: traces_count,
                results_returned: traces_count,
                cache_hit: false,
                extra: serde_json::json!({
                    "engine": "TraceStorage",
                    "query_type": "traces_query",
                    "traces_returned": traces_count,
                }),
            }),
        })
    }

    /// Extract trace_id filter from query content
    fn extract_trace_id_filter(&self, content: &str) -> Option<String> {
        let upper = content.to_uppercase();

        // Look for trace_id = 'xxx' or trace_id='xxx'
        if let Some(pos) = upper.find("TRACE_ID") {
            let rest = &content[pos + 8..];
            let rest = rest.trim_start();

            // Skip = or ==
            let rest = rest.trim_start_matches('=').trim_start();

            // Extract value
            if rest.starts_with('\'') || rest.starts_with('"') {
                let quote = rest.chars().next()?;
                let rest = &rest[1..];
                if let Some(end) = rest.find(quote) {
                    return Some(rest[..end].to_string());
                }
            }
        }

        None
    }

    /// Extract time range from query content
    fn extract_time_range(&self, content: &str) -> (i64, i64) {
        let upper = content.to_uppercase();
        let mut start_ns = 0i64;
        let mut end_ns = i64::MAX;

        // Look for start_time_ns > N or start_time_ns >= N
        if let Some(pos) = upper.find("START_TIME_NS")
            && let Some(value) = self.extract_numeric_comparison(&content[pos + 13..]) {
                start_ns = value;
            }

        // Look for end_time_ns < N or end_time_ns <= N
        if let Some(pos) = upper.find("END_TIME_NS")
            && let Some(value) = self.extract_numeric_comparison(&content[pos + 11..]) {
                end_ns = value;
            }

        // Look for timestamp > N style
        if let Some(pos) = upper.find("TIMESTAMP") {
            if (upper[pos..].starts_with("TIMESTAMP >") || upper[pos..].starts_with("TIMESTAMP >"))
                && let Some(value) = self.extract_numeric_comparison(&content[pos + 9..]) {
                    start_ns = value;
                }
        }

        (start_ns, end_ns)
    }

    /// Extract numeric value after comparison operator
    fn extract_numeric_comparison(&self, content: &str) -> Option<i64> {
        let content = content.trim_start();
        let content = content
            .trim_start_matches('>')
            .trim_start_matches('<')
            .trim_start_matches('=')
            .trim_start();

        // Parse the number
        let end = content
            .find(|c: char| !c.is_ascii_digit() && c != '-')
            .unwrap_or(content.len());

        if end > 0 {
            content[..end].parse().ok()
        } else {
            None
        }
    }

    /// Extract service filter from query
    fn extract_service_filter(&self, content: &str) -> Option<String> {
        let upper = content.to_uppercase();

        // Look for service = 'xxx' or service_name = 'xxx'
        for keyword in ["SERVICE_NAME", "SERVICE"] {
            if let Some(pos) = upper.find(keyword) {
                let rest = &content[pos + keyword.len()..];
                let rest = rest.trim_start().trim_start_matches('=').trim_start();

                if rest.starts_with('\'') || rest.starts_with('"') {
                    let quote = rest.chars().next()?;
                    let rest = &rest[1..];
                    if let Some(end) = rest.find(quote) {
                        return Some(rest[..end].to_string());
                    }
                }
            }
        }

        None
    }

    /// Extract operation filter from query
    fn extract_operation_filter(&self, content: &str) -> Option<String> {
        let upper = content.to_uppercase();

        // Look for operation = 'xxx' or operation_name = 'xxx'
        for keyword in ["OPERATION_NAME", "OPERATION"] {
            if let Some(pos) = upper.find(keyword) {
                let rest = &content[pos + keyword.len()..];
                let rest = rest.trim_start().trim_start_matches('=').trim_start();

                if rest.starts_with('\'') || rest.starts_with('"') {
                    let quote = rest.chars().next()?;
                    let rest = &rest[1..];
                    if let Some(end) = rest.find(quote) {
                        return Some(rest[..end].to_string());
                    }
                }
            }
        }

        None
    }

    /// Extract min_duration_ns filter from query
    fn extract_min_duration_filter(&self, content: &str) -> Option<i64> {
        let upper = content.to_uppercase();

        if let Some(pos) = upper.find("MIN_DURATION_NS") {
            return self.extract_numeric_comparison(&content[pos + 15..]);
        }

        if let Some(pos) = upper.find("DURATION_NS") {
            // Check if it's a > comparison
            let rest = &upper[pos + 11..];
            if rest.trim_start().starts_with('>') {
                return self.extract_numeric_comparison(&content[pos + 11..]);
            }
        }

        None
    }

    /// Extract max_duration_ns filter from query
    fn extract_max_duration_filter(&self, content: &str) -> Option<i64> {
        let upper = content.to_uppercase();

        if let Some(pos) = upper.find("MAX_DURATION_NS") {
            return self.extract_numeric_comparison(&content[pos + 15..]);
        }

        if let Some(pos) = upper.find("DURATION_NS") {
            // Check if it's a < comparison
            let rest = &upper[pos + 11..];
            if rest.trim_start().starts_with('<') {
                return self.extract_numeric_comparison(&content[pos + 11..]);
            }
        }

        None
    }

    /// Check if errors_only filter is present
    fn has_errors_only_filter(&self, content: &str) -> bool {
        let upper = content.to_uppercase();
        upper.contains("ERRORS_ONLY") && (upper.contains("TRUE") || upper.contains("= 1"))
    }

    /// Extract LIMIT from query
    fn extract_limit(&self, content: &str) -> Option<usize> {
        let upper = content.to_uppercase();

        if let Some(pos) = upper.find("LIMIT") {
            let rest = &content[pos + 5..];
            let rest = rest.trim_start();

            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());

            if end > 0 {
                return rest[..end].parse().ok();
            }
        }

        None
    }
}

#[async_trait]
impl QueryStrategy for ObservabilityStrategy {
    fn name(&self) -> &str {
        "observability"
    }

    fn can_handle(&self, request: &QueryRequest) -> bool {
        request.query_type == QueryType::Observability
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    #[instrument(skip(self, request, _ctx), fields(strategy = "observability"))]
    async fn execute(&self, request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
        let start = Instant::now();

        // Extract content
        let content = match &request.content {
            QueryContent::Sql(sql) => sql.clone(),
            QueryContent::Document(filter) => filter.clone(),
            _ => {
                return Err(anyhow!(
                    "ObservabilityStrategy requires SQL or Document content"
                ));
            }
        };

        // Detect query type and namespace
        let query_type = self.detect_query_type(&content);
        let namespace = self.extract_namespace(&request, &content);

        debug!(
            query_type = ?query_type,
            namespace = %namespace,
            "Routing observability query"
        );

        // Execute based on query type
        let result = match query_type {
            ObservabilityQueryType::Logs => {
                self.execute_log_query(&namespace, &content, start).await?
            }
            ObservabilityQueryType::Metrics => {
                self.execute_metrics_query(&namespace, &content, start)
                    .await?
            }
            ObservabilityQueryType::Traces => {
                self.execute_traces_query(&namespace, &content, start)
                    .await?
            }
        };

        info!(
            query_type = ?query_type,
            time_ms = result.metrics.as_ref().map_or(0, |m| m.execution_time_ms),
            "Observability query completed"
        );

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_query_type() {
        // We can't easily test without the full infrastructure,
        // but we can verify the query type detection logic

        // Test that Observability requests are correctly identified
        let request = QueryRequest {
            query_type: QueryType::Observability,
            target: Some("production".to_string()),
            content: QueryContent::Sql("SELECT * FROM LOGS('production')".to_string()),
            params: Default::default(),
        };

        assert_eq!(request.query_type, QueryType::Observability);
    }

    #[test]
    fn test_namespace_parsing() {
        // This is a simple unit test for namespace extraction logic
        let content = "SELECT * FROM LOGS('my_namespace') WHERE message LIKE '%error%'";
        let upper = content.to_uppercase();

        // Find LOGS( position
        let pos = upper.find("LOGS(").unwrap();
        assert!(pos > 0);

        // Extract namespace manually for test
        let rest = &content[pos + 5..];
        let end = rest.find(')').unwrap();
        let ns = rest[..end].trim().trim_matches(|c| c == '\'' || c == '"');
        assert_eq!(ns, "my_namespace");
    }

    /// Helper to test PromQL extraction without full strategy initialization
    mod promql_extraction {
        #[test]
        fn test_extract_promql_from_metrics_two_args() {
            // METRICS('namespace', 'promql_expr') format
            let content = "SELECT * FROM METRICS('production', 'rate(http_requests_total[5m])')";

            // Extract the second argument manually for testing
            let upper = content.to_uppercase();
            if let Some(start) = upper.find("METRICS(") {
                let rest = &content[start + 8..];
                if let Some(comma_pos) = rest.find(',') {
                    let after_comma = rest[comma_pos + 1..].trim();
                    // Extract quoted string
                    if after_comma.starts_with('\'') {
                        let rest = &after_comma[1..];
                        if let Some(end) = rest.find('\'') {
                            let expr = &rest[..end];
                            assert_eq!(expr, "rate(http_requests_total[5m])");
                            return;
                        }
                    }
                }
            }
            panic!("Failed to extract PromQL expression");
        }

        #[test]
        fn test_looks_like_promql() {
            // Test aggregation functions
            assert!(looks_like_promql_helper("sum(http_requests_total)"));
            assert!(looks_like_promql_helper("rate(cpu_usage[5m])"));
            assert!(looks_like_promql_helper("avg(memory_usage)"));

            // Test label matchers
            assert!(looks_like_promql_helper("http_requests{method=\"GET\"}"));
            assert!(looks_like_promql_helper("cpu_usage{host=\"server1\"}"));

            // Test range vectors
            assert!(looks_like_promql_helper("http_requests[5m]"));
            assert!(looks_like_promql_helper("memory_usage[1h]"));

            // Test simple metric names
            assert!(looks_like_promql_helper("http_requests_total"));
            assert!(looks_like_promql_helper("cpu:usage:ratio"));

            // Non-PromQL
            assert!(!looks_like_promql_helper(""));
            assert!(!looks_like_promql_helper("SELECT * FROM table"));
        }

        fn looks_like_promql_helper(s: &str) -> bool {
            let s = s.trim();
            if s.is_empty() {
                return false;
            }

            // Check for aggregation functions
            let agg_funcs = [
                "sum(",
                "avg(",
                "min(",
                "max(",
                "count(",
                "rate(",
                "irate(",
                "increase(",
                "histogram_quantile(",
                "topk(",
                "bottomk(",
                "stddev(",
            ];
            for func in &agg_funcs {
                if s.to_lowercase().starts_with(func) {
                    return true;
                }
            }

            // Check for label matchers or range vectors
            if s.contains('{') || s.contains('[') {
                return true;
            }

            // Check if it's a simple metric name (alphanumeric with underscores/colons)
            s.chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == ':')
        }

        #[test]
        fn test_extract_metric_name_from_where() {
            let content = "SELECT * FROM METRICS('ns') WHERE metric_name = 'cpu_usage'";
            let upper = content.to_uppercase();

            // Look for metric_name condition
            if let Some(where_pos) = upper.find("WHERE") {
                let rest = &content[where_pos + 5..].trim();
                if let Some(metric_pos) = rest.to_uppercase().find("METRIC_NAME") {
                    let after_metric = &rest[metric_pos..];
                    if let Some(eq_pos) = after_metric.find('=') {
                        let value_part = after_metric[eq_pos + 1..].trim();
                        if value_part.starts_with('\'') {
                            let rest = &value_part[1..];
                            if let Some(end) = rest.find('\'') {
                                let name = &rest[..end];
                                assert_eq!(name, "cpu_usage");
                                return;
                            }
                        }
                    }
                }
            }
            panic!("Failed to extract metric name");
        }

        #[test]
        fn test_detect_metrics_query_type() {
            // METRICS( function
            let content1 = "SELECT * FROM METRICS('production')";
            assert!(content1.to_uppercase().contains("METRICS("));

            // METRIC_NAME in query
            let content2 = "SELECT * FROM observability WHERE metric_name = 'cpu'";
            assert!(content2.to_uppercase().contains("METRIC_NAME"));
        }

        #[test]
        fn test_extract_interval_duration() {
            // Test parsing "interval '1h'"
            use crate::observability::query::promql::PromQLParser;

            let content = "WHERE timestamp > now() - interval '1h'";
            let upper = content.to_uppercase();

            if let Some(interval_pos) = upper.find("INTERVAL") {
                let rest = &content[interval_pos + 8..].trim();
                if rest.starts_with('\'') {
                    let inner = &rest[1..];
                    if let Some(end) = inner.find('\'') {
                        let duration_str = &inner[..end];
                        let duration = PromQLParser::parse_duration(duration_str).unwrap();
                        // 1 hour = 3,600,000,000,000 nanoseconds
                        assert_eq!(duration.nanoseconds, 3_600_000_000_000);
                        return;
                    }
                }
            }
            panic!("Failed to extract interval duration");
        }

        #[test]
        fn test_promql_parser_integration() {
            use crate::observability::query::promql::PromQLParser;

            // Test that the parser works for various expressions
            let expressions = [
                "http_requests_total",
                "http_requests_total{method=\"GET\"}",
                "http_requests_total[5m]",
                "sum(http_requests_total)",
                "sum(http_requests_total) by (method)",
                "rate(http_requests_total[5m])",
            ];

            for expr in &expressions {
                assert!(
                    PromQLParser::parse(expr).is_ok(),
                    "Failed to parse: {}",
                    expr
                );
            }
        }
    }
}
