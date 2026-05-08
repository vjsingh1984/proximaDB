//! Compatibility wrapper for the graph query planner.
//!
//! The canonical planner implementation now lives in the `proximadb-graph`
//! workspace crate. This root module preserves the existing import surface and
//! adapts root-only `GraphMemoryPool` statistics into the extracted planner's
//! snapshot-based API.

use crate::graph::GraphMemoryPool;
use proximadb_graph::query::QueryResult;
use proximadb_graph::query::ast::CompiledPattern;
use std::sync::Arc;

pub use proximadb_graph::query::planner::{
    CostEstimate, CostEstimator, CostModel, EdgeFilter, FilterCondition, FilterOperator,
    GraphStatistics, IndexStats, JoinType, OptimizationFlags, PlanStep, PlanStepType,
    PlannerConfig, PropertyFilter, QueryPlan, SortField, TraversalAlgorithm,
};

/// Root compatibility wrapper for the extracted graph query planner.
pub struct QueryPlanner {
    inner: proximadb_graph::query::planner::QueryPlanner,
}

impl QueryPlanner {
    /// Create a new query planner.
    pub fn new() -> Self {
        Self {
            inner: proximadb_graph::query::planner::QueryPlanner::new(),
        }
    }

    /// Create a new planner with custom configuration.
    pub fn with_config(config: PlannerConfig) -> Self {
        Self {
            inner: proximadb_graph::query::planner::QueryPlanner::with_config(config),
        }
    }

    /// Update planner statistics from the root graph memory pool.
    pub fn update_statistics(&self, memory_pool: &Arc<GraphMemoryPool>) -> QueryResult<()> {
        let mut stats = GraphStatistics {
            node_count: memory_pool.node_count() as u64,
            edge_count: memory_pool.edge_count() as u64,
            ..Default::default()
        };

        if stats.node_count > 0 {
            stats.avg_node_degree = stats.edge_count as f64 / stats.node_count as f64;
        }

        for entry in memory_pool.label_indexes.iter() {
            stats
                .label_selectivity
                .insert(entry.key().clone(), entry.value().len() as u64);
        }

        for entry in memory_pool.edge_type_indexes.iter() {
            stats
                .edge_type_selectivity
                .insert(entry.key().clone(), entry.value().len() as u64);
        }

        for entry in memory_pool.node_property_indexes.iter() {
            let prop_name = entry.key().clone();
            let prop_values = entry.value();
            let unique_values = prop_values.len() as u64;
            let total_entries = prop_values.len() as u64;

            stats
                .property_selectivity
                .insert(prop_name.clone(), unique_values);
            stats.index_stats.insert(
                format!("node_prop_{}", prop_name),
                IndexStats {
                    cardinality: total_entries,
                    selectivity: if stats.node_count > 0 {
                        total_entries as f64 / stats.node_count as f64
                    } else {
                        0.0
                    },
                    avg_seek_time_us: 0.0,
                    last_updated: std::time::Instant::now(),
                },
            );
        }

        for entry in memory_pool.edge_property_indexes.iter() {
            let prop_name = entry.key().clone();
            let prop_values = entry.value();
            let unique_values = prop_values.len() as u64;
            let total_entries = prop_values.len() as u64;

            stats
                .property_selectivity
                .insert(prop_name.clone(), unique_values);
            stats.index_stats.insert(
                format!("edge_prop_{}", prop_name),
                IndexStats {
                    cardinality: total_entries,
                    selectivity: if stats.edge_count > 0 {
                        total_entries as f64 / stats.edge_count as f64
                    } else {
                        0.0
                    },
                    avg_seek_time_us: 0.0,
                    last_updated: std::time::Instant::now(),
                },
            );
        }

        self.inner.update_statistics(&stats)
    }

    /// Replace planner statistics directly with a precomputed snapshot.
    pub fn set_statistics(&self, stats: &GraphStatistics) -> QueryResult<()> {
        self.inner.update_statistics(stats)
    }

    /// Create an optimized query plan from query type and parameters.
    pub fn create_plan(
        &self,
        query_type: &str,
        parameters: &std::collections::HashMap<String, serde_json::Value>,
    ) -> QueryResult<QueryPlan> {
        self.inner.create_plan(query_type, parameters)
    }

    /// Create an optimized plan for a compiled pattern query.
    pub fn plan_pattern_query(&self, pattern: &CompiledPattern) -> QueryResult<QueryPlan> {
        self.inner.plan_pattern_query(pattern)
    }

    /// Plan a pattern match query from raw query parameters.
    pub fn plan_pattern_match_query(
        &self,
        parameters: &std::collections::HashMap<String, serde_json::Value>,
    ) -> QueryResult<QueryPlan> {
        self.inner.plan_pattern_match_query(parameters)
    }

    /// Get current planner statistics snapshot.
    pub fn get_statistics(&self) -> QueryResult<GraphStatistics> {
        self.inner.get_statistics()
    }

    /// Clear cached plans.
    pub fn clear_cache(&self) -> QueryResult<()> {
        self.inner.clear_cache()
    }
}

impl Default for QueryPlanner {
    fn default() -> Self {
        Self::new()
    }
}
