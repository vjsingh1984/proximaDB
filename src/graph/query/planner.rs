//! Compatibility wrapper for the graph query planner.
//!
//! The canonical planner implementation is being migrated to the `proximadb-graph`
//! workspace crate. This module preserves the historical root import surface.

use super::ast::CompiledPattern;
use crate::graph::GraphMemoryPool;
use std::sync::Arc;

// TODO: Move implementation to proximadb-graph crate
// For now, provide stub implementations

/// Backwards-compat alias for [`GraphPlannerCostEstimate`].
pub type CostEstimate = GraphPlannerCostEstimate;

/// Cost estimate for a query plan step
#[derive(Debug, Clone, Default)]
pub struct GraphPlannerCostEstimate {
    pub cost: f64,
    pub rows: usize,
    pub total_cost: f64,
    pub memory_cost: f64,
    pub io_cost: f64,
}

/// Query plan step
#[derive(Debug, Clone)]
pub struct PlanStep {
    pub step_type: PlanStepType,
    pub cost: GraphPlannerCostEstimate,
    pub children: Vec<PlanStep>,
}

/// Type of query plan step
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanStepType {
    Scan,
    IndexSeek {
        index_name: String,
    },
    Traverse {
        algorithm: TraversalAlgorithm,
        max_depth: usize,
    },
    Traversal(TraversalAlgorithm),
    Filter,
    Join(JoinType),
    Sort,
    Aggregate,
}

/// Traversal algorithm
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraversalAlgorithm {
    Bfs,
    Dfs,
    ShortestPath,
}

/// Join type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinType {
    Inner,
    LeftOuter,
    RightOuter,
    FullOuter,
}

/// Query plan
#[derive(Debug, Clone)]
pub struct QueryPlan {
    pub steps: Vec<PlanStep>,
    pub estimated_cost: GraphPlannerCostEstimate,
    pub estimated_result_size: usize,
}

/// Graph statistics for planning
#[derive(Debug, Clone, Default)]
pub struct GraphStatistics {
    pub node_count: u64,
    pub edge_count: u64,
    pub avg_node_degree: f64,
    pub index_stats: Vec<PlannerIndexStats>,
    pub label_selectivity: std::collections::HashMap<String, f64>,
}

/// Index statistics
#[derive(Debug, Clone)]
pub struct PlannerIndexStats {
    pub index_type: String,
    pub cardinality: usize,
}

/// Planner configuration
#[derive(Debug, Clone)]
pub struct PlannerConfig {
    pub use_statistics: bool,
    pub max_planning_time_ms: u64,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            use_statistics: true,
            max_planning_time_ms: 1000,
        }
    }
}

/// Cost model for query optimization
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostModel {
    Simple,
    StatisticsBased,
}

/// Cost estimator
#[derive(Debug, Clone)]
pub struct CostEstimator {
    pub model: CostModel,
}

/// Filter condition
#[derive(Debug, Clone)]
pub struct FilterCondition {
    pub field: String,
    pub operator: FilterOperator,
    pub value: String,
}

/// Filter operator
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOperator {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Backwards-compat alias for [`PlannerEdgeFilter`].
pub type EdgeFilter = PlannerEdgeFilter;

/// Edge filter
#[derive(Debug, Clone)]
pub struct PlannerEdgeFilter {
    pub conditions: Vec<FilterCondition>,
}

/// Property filter
#[derive(Debug, Clone)]
pub struct PropertyFilter {
    pub property: String,
    pub conditions: Vec<FilterCondition>,
}

/// Sort field
#[derive(Debug, Clone)]
pub struct SortField {
    pub field: String,
    pub ascending: bool,
}

/// Optimization flags
#[derive(Debug, Clone, Default)]
pub struct OptimizationFlags {
    pub push_down_filters: bool,
    pub reorder_joins: bool,
    pub use_indices: bool,
}

/// Result type
pub type QueryResult<T> = Result<T, String>;

/// Root compatibility wrapper for the extracted graph query planner.
pub struct QueryPlanner {
    _private: (),
}

impl QueryPlanner {
    /// Create a new query planner.
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Create a new planner with custom configuration.
    pub fn with_config(_config: PlannerConfig) -> Self {
        Self { _private: () }
    }

    /// Update planner statistics from the root graph memory pool.
    pub fn update_statistics(&self, _memory_pool: &Arc<GraphMemoryPool>) -> QueryResult<()> {
        Ok(())
    }

    /// Plan a graph query.
    pub fn plan(&self, _query: &str) -> QueryResult<QueryPlan> {
        Ok(QueryPlan {
            steps: vec![],
            estimated_cost: GraphPlannerCostEstimate::default(),
            estimated_result_size: 0,
        })
    }

    /// Create a plan from a compiled pattern.
    pub fn plan_pattern(&self, _pattern: &CompiledPattern) -> QueryResult<QueryPlan> {
        Ok(QueryPlan {
            steps: vec![],
            estimated_cost: GraphPlannerCostEstimate::default(),
            estimated_result_size: 0,
        })
    }
}

impl Default for QueryPlanner {
    fn default() -> Self {
        Self::new()
    }
}

// Re-exports for compatibility - CompiledPattern already imported above
