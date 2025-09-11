//! Orchestration-level EXPLAIN plan structures for SQL queries.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExplainPlan {
    pub orchestration_steps: Vec<String>,
    pub vector_hints: Option<VectorHints>,
    pub graph_hints: Option<GraphHints>,
    pub join_costs: Option<JoinCostEstimate>,
    pub query_stats: Option<AnalyzeMetrics>,
    pub execution_strategy: Option<String>,
    pub estimated_total_cost: Option<f64>,
}

impl ExplainPlan {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an EXPLAIN plan with orchestration steps
    pub fn with_steps(steps: Vec<String>) -> Self {
        Self {
            orchestration_steps: steps,
            ..Default::default()
        }
    }

    /// Add vector hints to the plan
    pub fn with_vector_hints(mut self, hints: VectorHints) -> Self {
        self.vector_hints = Some(hints);
        self
    }

    /// Add graph hints to the plan
    pub fn with_graph_hints(mut self, hints: GraphHints) -> Self {
        self.graph_hints = Some(hints);
        self
    }

    /// Add join cost estimates
    pub fn with_join_costs(mut self, costs: JoinCostEstimate) -> Self {
        self.join_costs = Some(costs);
        self
    }

    /// Add ANALYZE metrics
    pub fn with_analyze_metrics(mut self, metrics: AnalyzeMetrics) -> Self {
        self.query_stats = Some(metrics);
        self
    }

    /// Set the overall execution strategy
    pub fn with_execution_strategy(mut self, strategy: String) -> Self {
        self.execution_strategy = Some(strategy);
        self
    }

    /// Set estimated total cost
    pub fn with_total_cost(mut self, cost: f64) -> Self {
        self.estimated_total_cost = Some(cost);
        self
    }
}

/// Lightweight vector-side hints surfaced from VectorOperationsService when available.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VectorHints {
    pub cache_hit: bool,
    pub pruned_files: Option<usize>,
    pub ef_search: Option<usize>,
    pub nprobe: Option<usize>,
    pub candidates: Option<usize>,
    pub progressive_stages: Option<Vec<String>>,
    pub recall_estimates: Option<Vec<f32>>,
    pub index_type: Option<String>,
    pub quantization_level: Option<String>,
    pub estimated_io_cost: Option<f64>,
    pub estimated_compute_cost: Option<f64>,
}

/// Graph-side hints from graph query planning and execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphHints {
    /// Graph traversal algorithm used
    pub traversal_algorithm: Option<String>,
    /// Maximum traversal depth
    pub max_depth: Option<u32>,
    /// Starting nodes for traversal
    pub start_nodes: Option<usize>,
    /// Index usage in graph operations
    pub index_usage: Vec<GraphIndexUsage>,
    /// Estimated nodes to visit
    pub estimated_nodes_visited: Option<usize>,
    /// Estimated edges to traverse
    pub estimated_edges_traversed: Option<usize>,
    /// Graph statistics used in planning
    pub graph_stats: Option<GraphPlannerStats>,
    /// Edge filters applied
    pub edge_filters: Option<usize>,
    /// Node filters applied
    pub node_filters: Option<usize>,
    /// Memory estimate for graph operation
    pub estimated_memory_mb: Option<f64>,
    /// Estimated I/O cost for graph operations
    pub estimated_io_cost: Option<f64>,
}

/// Information about index usage in graph operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphIndexUsage {
    /// Index name
    pub index_name: String,
    /// Index type (label_index, property_index, composite_index)
    pub index_type: String,
    /// Estimated selectivity (0.0 to 1.0)
    pub selectivity: f64,
    /// Whether index was actually used
    pub used: bool,
    /// Reason if index was not used
    pub skip_reason: Option<String>,
}

/// Graph planner statistics used for cost estimation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphPlannerStats {
    /// Total node count in graph
    pub total_nodes: usize,
    /// Total edge count in graph
    pub total_edges: usize,
    /// Average node degree
    pub avg_node_degree: f64,
    /// Label selectivity map
    pub label_selectivity: HashMap<String, usize>,
    /// Property cardinality estimates
    pub property_cardinality: HashMap<String, usize>,
}

/// Join cost estimation for hybrid queries
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JoinCostEstimate {
    /// Join algorithm used
    pub join_algorithm: String,
    /// Estimated cost of the join
    pub estimated_cost: f64,
    /// Left input cardinality estimate
    pub left_cardinality: usize,
    /// Right input cardinality estimate  
    pub right_cardinality: usize,
    /// Join selectivity estimate
    pub join_selectivity: f64,
    /// Memory requirements in MB
    pub memory_mb: f64,
    /// Expected output cardinality
    pub output_cardinality: usize,
    /// Join key information
    pub join_keys: Vec<String>,
}

/// ANALYZE metrics from actual query execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnalyzeMetrics {
    /// Actual execution time in milliseconds
    pub actual_execution_time_ms: u64,
    /// Actual rows returned
    pub actual_rows: usize,
    /// Actual memory usage in MB
    pub actual_memory_mb: f64,
    /// Cache hit rates
    pub cache_statistics: CacheStatistics,
    /// I/O statistics
    pub io_statistics: IOStatistics,
    /// Operator timing breakdown
    pub operator_timings: Vec<OperatorTiming>,
    /// Resource utilization
    pub resource_usage: ResourceUsage,
}

/// Cache statistics for ANALYZE
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheStatistics {
    /// Vector cache hit rate
    pub vector_cache_hit_rate: f64,
    /// Graph cache hit rate  
    pub graph_cache_hit_rate: f64,
    /// Plan cache hit
    pub plan_cache_hit: bool,
    /// Total cache requests
    pub total_cache_requests: usize,
    /// Total cache hits
    pub total_cache_hits: usize,
}

/// I/O statistics for ANALYZE
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IOStatistics {
    /// Total bytes read
    pub bytes_read: u64,
    /// Total bytes written
    pub bytes_written: u64,
    /// Number of disk seeks
    pub disk_seeks: usize,
    /// Files accessed
    pub files_accessed: usize,
    /// Average I/O latency in microseconds
    pub avg_io_latency_us: f64,
}

/// Individual operator timing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorTiming {
    /// Operator name
    pub operator: String,
    /// Time spent in milliseconds
    pub time_ms: u64,
    /// Rows processed
    pub rows_processed: usize,
    /// Memory used in MB
    pub memory_mb: f64,
}

/// Resource utilization metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceUsage {
    /// Peak memory usage in MB
    pub peak_memory_mb: f64,
    /// CPU time in milliseconds
    pub cpu_time_ms: u64,
    /// Number of threads used
    pub threads_used: usize,
    /// GPU utilization if applicable
    pub gpu_utilization: Option<f64>,
}

impl GraphHints {
    /// Create GraphHints from a graph query plan
    pub fn from_query_plan(
        plan: &crate::graph::query::QueryPlan,
        stats: Option<&crate::graph::query::planner::GraphStatistics>,
    ) -> Self {
        let mut hints = GraphHints::default();

        // Extract information from plan steps
        for step in &plan.steps {
            match &step.step_type {
                crate::graph::query::planner::PlanStepType::IndexSeek { index_name, .. } => {
                    hints.index_usage.push(GraphIndexUsage {
                        index_name: index_name.clone(),
                        index_type: "label_index".to_string(),
                        selectivity: step.cost.selectivity_estimate.unwrap_or(1.0),
                        used: true,
                        skip_reason: None,
                    });
                }
                crate::graph::query::planner::PlanStepType::Traverse {
                    algorithm,
                    max_depth,
                    ..
                } => {
                    hints.traversal_algorithm = Some(format!("{:?}", algorithm));
                    hints.max_depth = *max_depth;
                }
                _ => {}
            }
        }

        // Estimate costs and cardinalities
        hints.estimated_nodes_visited = Some(plan.estimated_result_size);
        hints.estimated_memory_mb = Some(plan.estimated_cost.memory_cost);
        hints.estimated_io_cost = Some(plan.estimated_cost.io_cost);

        // Add graph statistics if available
        if let Some(stats) = stats {
            hints.graph_stats = Some(GraphPlannerStats {
                total_nodes: stats.node_count,
                total_edges: stats.edge_count,
                avg_node_degree: stats.avg_node_degree,
                label_selectivity: stats.label_selectivity.clone(),
                property_cardinality: HashMap::new(), // TODO: Add property stats
            });
        }

        hints
    }
}

impl JoinCostEstimate {
    /// Create a join cost estimate for vector-graph hybrid queries
    pub fn for_hybrid_join(
        vector_cardinality: usize,
        graph_cardinality: usize,
        join_selectivity: f64,
    ) -> Self {
        let estimated_cost = (vector_cardinality as f64) * (graph_cardinality as f64) * 0.001; // Simple cost model
        let output_cardinality =
            ((vector_cardinality as f64) * (graph_cardinality as f64) * join_selectivity) as usize;
        let memory_mb = ((vector_cardinality + graph_cardinality) as f64 * 0.001).max(1.0); // Rough estimate

        JoinCostEstimate {
            join_algorithm: "hybrid_hash_join".to_string(),
            estimated_cost,
            left_cardinality: vector_cardinality,
            right_cardinality: graph_cardinality,
            join_selectivity,
            memory_mb,
            output_cardinality,
            join_keys: vec!["id".to_string()], // Typical join key
        }
    }
}

impl AnalyzeMetrics {
    /// Create minimal ANALYZE metrics for testing
    pub fn minimal(execution_time_ms: u64, rows: usize) -> Self {
        AnalyzeMetrics {
            actual_execution_time_ms: execution_time_ms,
            actual_rows: rows,
            actual_memory_mb: 1.0,
            cache_statistics: CacheStatistics::default(),
            io_statistics: IOStatistics::default(),
            operator_timings: vec![],
            resource_usage: ResourceUsage::default(),
        }
    }
}
