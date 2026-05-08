pub mod ast;
pub mod cypher_ast;
pub mod cypher_functions;
pub mod cypher_parser;
pub mod execution;
pub mod executor;
pub mod operators;
pub mod parser;
pub mod pattern;
pub mod planner;
pub mod service;
pub mod storage;
pub mod traversal;
pub mod unified_parser;

use proximadb_kernel::error::ProximaDBError;
use serde::Serialize;
use std::collections::HashMap;

/// Canonical result type for extracted graph-query modules.
pub type QueryResult<T> = std::result::Result<T, ProximaDBError>;

/// Query execution statistics shared across graph query runtimes.
#[derive(Debug, Clone, Serialize)]
pub struct QueryStats {
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

impl QueryStats {
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

impl Default for QueryStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Query execution context.
#[derive(Debug)]
pub struct QueryContext {
    /// Graph ID for the query
    pub graph_id: String,
    /// Query parameters
    pub parameters: HashMap<String, serde_json::Value>,
    /// Execution timeout in milliseconds
    pub timeout_ms: Option<u64>,
    /// Maximum memory limit in bytes
    pub memory_limit: Option<usize>,
    /// Whether to collect detailed statistics
    pub collect_stats: bool,
}

impl QueryContext {
    pub fn new() -> Self {
        Self {
            graph_id: "default".to_string(),
            parameters: HashMap::new(),
            timeout_ms: None,
            memory_limit: None,
            collect_stats: false,
        }
    }

    pub fn with_graph_id(mut self, graph_id: String) -> Self {
        self.graph_id = graph_id;
        self
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn with_memory_limit(mut self, limit: usize) -> Self {
        self.memory_limit = Some(limit);
        self
    }

    pub fn with_stats(mut self) -> Self {
        self.collect_stats = true;
        self
    }
}

impl Default for QueryContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Query execution result.
#[derive(Debug)]
pub struct QueryExecutionResult {
    /// Result data (JSON format for flexibility)
    pub data: serde_json::Value,
    /// Execution statistics
    pub stats: QueryStats,
    /// Any warnings generated during execution
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_query_stats_defaults_to_zeroed_counters() {
        let stats = QueryStats::new();
        let default_stats = QueryStats::default();

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
        let mut context = QueryContext::new()
            .with_graph_id("tenant-graph".to_string())
            .with_timeout(500)
            .with_memory_limit(4096)
            .with_stats();
        context
            .parameters
            .insert("tenant".to_string(), json!("acme"));

        assert_eq!(context.graph_id, "tenant-graph");
        assert_eq!(context.timeout_ms, Some(500));
        assert_eq!(context.memory_limit, Some(4096));
        assert!(context.collect_stats);
        assert_eq!(context.parameters.get("tenant"), Some(&json!("acme")));
    }

    #[test]
    fn test_query_execution_result_holds_data_stats_and_warnings() {
        let result = QueryExecutionResult {
            data: json!([{"node_id": "n1"}]),
            stats: QueryStats {
                nodes_visited: 3,
                ..QueryStats::default()
            },
            warnings: vec!["fallback path".to_string()],
        };

        assert_eq!(result.data, json!([{"node_id": "n1"}]));
        assert_eq!(result.stats.nodes_visited, 3);
        assert_eq!(result.warnings, vec!["fallback path"]);
    }
}
