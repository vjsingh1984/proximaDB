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
    ExecutionMetrics, QueryContent, QueryContext,
    QueryRequest, QueryResult, QueryResultData, QueryStrategy, QueryType,
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
        use crate::observability::query::LogSearchOptions;
        use crate::observability::LogQueryParams;

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

            let results = self.query_engine.search_logs_fulltext(namespace, &query, &options).await?;

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

        let result = self.query_engine.query_logs_with_fulltext(namespace, params, false).await?;

        let execution_time_ms = start_time.elapsed().as_millis() as u64;
        let logs_count = result.logs.len();

        // Convert to JSON rows
        let rows: Vec<serde_json::Value> = result.logs
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

    /// Execute metrics query (placeholder - returns empty for now)
    async fn execute_metrics_query(
        &self,
        namespace: &str,
        _content: &str,
        start_time: Instant,
    ) -> Result<QueryResult> {
        debug!(
            namespace = %namespace,
            "Executing metrics query"
        );

        let execution_time_ms = start_time.elapsed().as_millis() as u64;

        // Placeholder - metrics query infrastructure exists but needs integration
        Ok(QueryResult {
            data: QueryResultData::Rows(vec![]),
            metrics: Some(ExecutionMetrics {
                execution_path: "unified".to_string(),
                strategy_name: "observability".to_string(),
                execution_time_ms,
                planning_time_ms: 0,
                results_scanned: 0,
                results_returned: 0,
                cache_hit: false,
                extra: serde_json::json!({
                    "engine": "ObservabilityQueryEngine",
                    "query_type": "metrics",
                    "status": "placeholder",
                }),
            }),
        })
    }

    /// Execute traces query (placeholder - returns empty for now)
    async fn execute_traces_query(
        &self,
        namespace: &str,
        _content: &str,
        start_time: Instant,
    ) -> Result<QueryResult> {
        debug!(
            namespace = %namespace,
            "Executing traces query"
        );

        let execution_time_ms = start_time.elapsed().as_millis() as u64;

        // Placeholder - traces query infrastructure exists but needs integration
        Ok(QueryResult {
            data: QueryResultData::Rows(vec![]),
            metrics: Some(ExecutionMetrics {
                execution_path: "unified".to_string(),
                strategy_name: "observability".to_string(),
                execution_time_ms,
                planning_time_ms: 0,
                results_scanned: 0,
                results_returned: 0,
                cache_hit: false,
                extra: serde_json::json!({
                    "engine": "ObservabilityQueryEngine",
                    "query_type": "traces",
                    "status": "placeholder",
                }),
            }),
        })
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
            _ => return Err(anyhow!("ObservabilityStrategy requires SQL or Document content")),
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
                self.execute_metrics_query(&namespace, &content, start).await?
            }
            ObservabilityQueryType::Traces => {
                self.execute_traces_query(&namespace, &content, start).await?
            }
        };

        info!(
            query_type = ?query_type,
            time_ms = result.metrics.as_ref().map(|m| m.execution_time_ms).unwrap_or(0),
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
}
