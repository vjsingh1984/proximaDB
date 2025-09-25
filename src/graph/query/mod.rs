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
pub mod executor;
pub mod parser;
pub mod pattern;
pub mod planner;

// Re-export public types
pub use ast::{CompiledPattern, FoundPath, MatchResult};
pub use pattern::PatternMatcher;
pub use planner::{CostEstimate, PlanStep, QueryPlan, QueryPlanner};

use crate::core::error::ProximaDBError;
use serde::Serialize;
use std::collections::HashMap;

/// Result type for query operations
pub type QueryResult<T> = std::result::Result<T, ProximaDBError>;

/// Query execution statistics
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

/// Query execution context
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

/// Query execution result
#[derive(Debug)]
pub struct QueryExecutionResult {
    /// Result data (JSON format for flexibility)
    pub data: serde_json::Value,
    /// Execution statistics
    pub stats: QueryStats,
    /// Any warnings generated during execution
    pub warnings: Vec<String>,
}
