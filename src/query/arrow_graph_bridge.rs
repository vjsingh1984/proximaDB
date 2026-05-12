//! Compatibility shim for the extracted graph Arrow bridge crate.
//!
//! The canonical Arrow conversion/runtime helpers now live in the
//! `proximadb-graph-arrow` workspace crate. This module preserves the
//! historical root import path.

use async_trait::async_trait;
use proximadb_graph_query::{GraphQueryContext, GraphQueryRow};
use proximadb_kernel::error::VectorDBError;

// TODO: Move implementation to proximadb-graph-arrow crate
// For now, provide stub implementations and use local QueryPlan

/// Graph Arrow bridge
#[derive(Debug, Clone)]
pub struct GraphArrowBridge;

/// Graph Arrow query executor trait
#[async_trait]
pub trait GraphArrowQueryExecutor: Send + Sync {
    async fn execute_query_rows(
        &self,
        plan: &crate::graph::query::planner::QueryPlan,
        context: &GraphQueryContext,
    ) -> Result<Vec<GraphQueryRow>, VectorDBError>;
}

/// Graph Arrow result
#[derive(Debug, Clone)]
pub struct GraphArrowResult {
    pub rows: Vec<GraphQueryRow>,
}

/// Graph column
#[derive(Debug, Clone)]
pub struct GraphColumn {
    pub name: String,
    pub values: Vec<arrow::array::ArrayRef>,
}

/// Graph schema
#[derive(Debug, Clone)]
pub struct GraphSchema {
    pub columns: Vec<GraphColumn>,
}

// Re-export local QueryPlan for compatibility
pub use crate::graph::query::planner::QueryPlan;

#[async_trait]
impl GraphArrowQueryExecutor for crate::graph::query::executor::QueryExecutor {
    async fn execute_query_rows(
        &self,
        _plan: &crate::graph::query::planner::QueryPlan,
        _context: &GraphQueryContext,
    ) -> Result<Vec<GraphQueryRow>, VectorDBError> {
        Ok(vec![])
    }
}
