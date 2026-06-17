use std::collections::HashMap;

use proximadb_kernel::ExecutionContext;
use serde::Serialize;

/// Canonical row shape produced by graph query execution.
///
/// This remains graph-query-specific until the execution layer is converted to
/// the unified `ProximaRecord`/`ProximaValue` projection model.
pub type GraphQueryRow = HashMap<String, serde_json::Value>;

/// Query execution statistics shared across graph query runtimes.
#[derive(Debug, Clone, Serialize)]
pub struct GraphQueryStats {
    /// Planning time in microseconds
    pub planning_time_us: u64,
    /// Execution time in microseconds
    pub execution_time_us: u64,
    /// Number of nodes visited
    pub nodes_visited: usize,
    /// Number of edges traversed
    pub edges_traversed: usize,
    /// Memory used in bytes
    pub memory_used: usize,
    /// Index hits
    pub index_hits: usize,
    /// Cache hits
    pub cache_hits: usize,
}

impl GraphQueryStats {
    pub fn new() -> Self {
        Self {
            planning_time_us: 0,
            execution_time_us: 0,
            nodes_visited: 0,
            edges_traversed: 0,
            memory_used: 0,
            index_hits: 0,
            cache_hits: 0,
        }
    }
}

impl Default for GraphQueryStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Graph query execution context.
#[derive(Debug)]
pub struct GraphQueryContext {
    /// Graph ID for the query
    pub graph_id: String,
    /// Common execution metadata shared across modality runtimes.
    pub execution: ExecutionContext,
}

impl GraphQueryContext {
    pub fn new() -> Self {
        Self {
            graph_id: "default".to_string(),
            execution: ExecutionContext::new(),
        }
    }

    pub fn with_graph_id(mut self, graph_id: String) -> Self {
        self.graph_id = graph_id;
        self
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.execution.limits.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn with_memory_limit(mut self, limit: usize) -> Self {
        self.execution.limits.memory_limit_bytes = Some(limit);
        self
    }

    pub fn with_stats(mut self) -> Self {
        self.execution.collect_stats = true;
        self
    }

    pub fn with_execution(mut self, execution: ExecutionContext) -> Self {
        self.execution = execution;
        self
    }
}

impl Default for GraphQueryContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Query execution result.
#[derive(Debug)]
pub struct GraphQueryExecutionResult {
    /// Result data (JSON format for flexibility)
    pub data: serde_json::Value,
    /// Execution statistics
    pub stats: GraphQueryStats,
    /// Any warnings generated during execution
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_query_stats_defaults_to_zeroed_counters() {
        let stats = GraphQueryStats::new();
        let default_stats = GraphQueryStats::default();

        assert_eq!(stats.planning_time_us, 0);
        assert_eq!(stats.execution_time_us, 0);
        assert_eq!(stats.nodes_visited, 0);
        assert_eq!(stats.edges_traversed, 0);
        assert_eq!(stats.memory_used, 0);
        assert_eq!(stats.index_hits, 0);
        assert_eq!(stats.cache_hits, 0);
        assert_eq!(default_stats.cache_hits, 0);
    }

    #[test]
    fn test_query_context_builder_chain_preserves_requested_limits() {
        let mut context = GraphQueryContext::new()
            .with_graph_id("tenant-graph".to_string())
            .with_timeout(500)
            .with_memory_limit(4096)
            .with_stats();
        context
            .execution
            .parameters
            .insert("tenant".to_string(), json!("acme"));

        assert_eq!(context.graph_id, "tenant-graph");
        assert_eq!(context.execution.limits.timeout_ms, Some(500));
        assert_eq!(context.execution.limits.memory_limit_bytes, Some(4096));
        assert!(context.execution.collect_stats);
        assert_eq!(
            context.execution.parameters.get("tenant"),
            Some(&json!("acme"))
        );
    }

    #[test]
    fn test_query_execution_result_holds_data_stats_and_warnings() {
        let result = GraphQueryExecutionResult {
            data: json!([{"node_id": "n1"}]),
            stats: GraphQueryStats {
                nodes_visited: 3,
                ..GraphQueryStats::default()
            },
            warnings: vec!["fallback path".to_string()],
        };

        assert_eq!(result.data, json!([{"node_id": "n1"}]));
        assert_eq!(result.stats.nodes_visited, 3);
        assert_eq!(result.warnings, vec!["fallback path"]);
    }
}
