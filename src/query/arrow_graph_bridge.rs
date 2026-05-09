//! Compatibility shim for the extracted graph Arrow bridge crate.
//!
//! The canonical Arrow conversion/runtime helpers now live in the
//! `proximadb-graph-arrow` workspace crate. This module preserves the
//! historical root import path while root-only compatibility impls remain here.

pub use proximadb_graph_arrow::{
    GraphArrowBridge, GraphArrowQueryExecutor, GraphArrowResult, GraphColumn, GraphSchema,
};

use async_trait::async_trait;
use proximadb_graph::query::planner::QueryPlan;
use proximadb_graph_query::{GraphQueryContext, GraphQueryRow};
use proximadb_kernel::error::VectorDBError;

#[async_trait]
impl GraphArrowQueryExecutor<QueryPlan> for crate::graph::query::executor::QueryExecutor {
    async fn execute_query_rows(
        &self,
        plan: &QueryPlan,
        context: &GraphQueryContext,
    ) -> Result<Vec<GraphQueryRow>, VectorDBError> {
        self.execute(plan, context).await
    }
}
