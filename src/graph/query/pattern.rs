//! Compatibility wrapper for the extracted graph pattern runtime.
//!
//! The canonical `PatternMatcher` implementation now lives in the
//! `proximadb-graph` workspace crate. This module preserves the historical root
//! API surface that accepts a `GraphMemoryPool`, while adapting that pool to the
//! narrower read-side storage contract used by the extracted graph-query crate.

use super::{CompiledPattern, MatchResult, QueryContext, QueryResult};
use crate::graph::{Edge, GraphMemoryPool, Node};
use anyhow::Result;
use proximadb_graph::query::pattern::PatternMatcher as InnerPatternMatcher;
use proximadb_graph::query::storage::GraphQueryStorage;
use std::sync::Arc;

pub use proximadb_graph::query::pattern::PatternCompiler;

struct GraphMemoryPoolQueryStorageAdapter {
    memory_pool: Arc<GraphMemoryPool>,
}

impl GraphMemoryPoolQueryStorageAdapter {
    fn new(memory_pool: Arc<GraphMemoryPool>) -> Self {
        Self { memory_pool }
    }
}

impl GraphQueryStorage for GraphMemoryPoolQueryStorageAdapter {
    fn get_node(&self, id: &str) -> Result<Option<Arc<Node>>> {
        Ok(self.memory_pool.get_node(&id.to_string()))
    }

    fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>> {
        Ok(self
            .memory_pool
            .label_indexes
            .get(label)
            .map(|node_ids| {
                node_ids
                    .iter()
                    .filter_map(|node_id| self.memory_pool.get_node(node_id))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>> {
        Ok(self
            .memory_pool
            .nodes
            .iter()
            .map(|entry| Arc::clone(entry.value()))
            .collect())
    }

    fn get_outgoing_edges(&self, node_id: &str, edge_type: Option<&str>) -> Result<Vec<Arc<Edge>>> {
        Ok(self
            .memory_pool
            .edges
            .iter()
            .filter_map(|entry| {
                let edge = entry.value();
                if edge.from_node_id != node_id {
                    return None;
                }
                if let Some(expected_type) = edge_type
                    && edge.edge_type != expected_type
                {
                    return None;
                }
                Some(Arc::clone(edge))
            })
            .collect())
    }

    fn get_incoming_edges(&self, node_id: &str, edge_type: Option<&str>) -> Result<Vec<Arc<Edge>>> {
        Ok(self
            .memory_pool
            .edges
            .iter()
            .filter_map(|entry| {
                let edge = entry.value();
                if edge.to_node_id != node_id {
                    return None;
                }
                if let Some(expected_type) = edge_type
                    && edge.edge_type != expected_type
                {
                    return None;
                }
                Some(Arc::clone(edge))
            })
            .collect())
    }
}

fn graph_memory_pool_query_storage(
    memory_pool: Arc<GraphMemoryPool>,
) -> Arc<dyn GraphQueryStorage> {
    Arc::new(GraphMemoryPoolQueryStorageAdapter::new(memory_pool))
}

/// Compatibility wrapper preserving the historical root `PatternMatcher` API.
pub struct PatternMatcher {
    inner: InnerPatternMatcher,
}

impl PatternMatcher {
    pub fn new() -> QueryResult<Self> {
        Ok(Self {
            inner: InnerPatternMatcher::new()?,
        })
    }

    pub fn compile_pattern(&mut self, pattern_str: &str) -> QueryResult<CompiledPattern> {
        self.inner.compile_pattern(pattern_str)
    }

    pub fn validate_query(&self, query: &str) -> QueryResult<()> {
        self.inner.validate_query(query)
    }

    pub fn execute_query(
        &mut self,
        query: &str,
        memory_pool: &Arc<GraphMemoryPool>,
        context: &QueryContext,
    ) -> QueryResult<Vec<MatchResult>> {
        let storage = graph_memory_pool_query_storage(Arc::clone(memory_pool));
        self.inner.execute_query(query, &storage, context)
    }

    pub fn execute_pattern(
        &self,
        pattern: &CompiledPattern,
        memory_pool: &Arc<GraphMemoryPool>,
        context: &QueryContext,
    ) -> QueryResult<Vec<MatchResult>> {
        let storage = graph_memory_pool_query_storage(Arc::clone(memory_pool));
        self.inner.execute_pattern(pattern, &storage, context)
    }

    pub fn apply_union(
        &self,
        left_results: Vec<MatchResult>,
        right_results: Vec<MatchResult>,
        distinct: bool,
    ) -> QueryResult<Vec<MatchResult>> {
        self.inner
            .apply_union(left_results, right_results, distinct)
    }
}
