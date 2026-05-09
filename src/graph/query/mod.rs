/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # Graph Query Processing Module
//!
//! This module implements the query processing pipeline for ProximaDB's graph database,
//! including cost-based query planning, pattern matching, and execution optimization.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │              Query Input                │
//! │  (SQL, Cypher, Programmatic API)        │
//! └───────────────┬─────────────────────────┘
//!                 │
//! ┌───────────────▼─────────────────────────┐
//! │            Query Parser                 │
//! │  • Pattern recognition                  │
//! │  • AST generation                       │
//! │  • Syntax validation                    │
//! └───────────────┬─────────────────────────┘
//!                 │
//! ┌───────────────▼─────────────────────────┐
//! │           Query Planner                 │
//! │  • Cost-based optimization              │
//! │  • Index selection                      │
//! │  • Join order optimization              │
//! │  • Statistics-based decisions           │
//! └───────────────┬─────────────────────────┘
//!                 │
//! ┌───────────────▼─────────────────────────┐
//! │          Query Executor                 │
//! │  • Parallel execution                   │
//! │  • Pipeline processing                  │
//! │  • Result streaming                     │
//! └─────────────────────────────────────────┘
//! ```

pub mod ast;
pub mod cypher_ast;
pub mod cypher_functions;
pub mod cypher_parser;
pub mod execution_traits;
pub mod executor;
pub mod operators;
pub mod parser;
pub mod pattern;
pub mod planner;
pub mod unified_parser;

// Re-export public types
pub use ast::{CompiledPattern, FoundPath, MatchResult};
pub use cypher_ast::{CypherClause, CypherStatement, MatchClause, ReturnClause};
pub use cypher_functions::CypherFunctionRegistry;
pub use cypher_parser::CypherParser;
pub use execution_traits::{
    ColumnSpec, ExecutionContext, ExecutionStats, PathElement, PhysicalOperator, QueryValue,
    ResultTuple, ValueType,
};
pub use pattern::PatternMatcher;
pub use planner::{CostEstimate, PlanStep, QueryPlan, QueryPlanner};
pub use proximadb_graph_query::{
    GraphQueryContext as QueryContext, GraphQueryExecutionResult as QueryExecutionResult,
    GraphQueryStats as QueryStats,
};
pub use unified_parser::{parse_cypher, parse_cypher_with_context};

/// Result type for query operations
pub type QueryResult<T> = proximadb_graph_query::GraphQueryResult<T>;

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
