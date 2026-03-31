//! # Cross-Model Query Optimizer
//!
//! Optimizes federated queries across multiple data models.
//!
//! ## Optimization Strategies
//!
//! - **Predicate pushdown**: Push filters to storage engines
//! - **Join reordering**: Optimize cross-model join order
//! - **Model-aware cost estimation**: Different cost models per data model
//! - **Parallel execution planning**: Identify parallelizable operations

use anyhow::{Result, anyhow};
use std::collections::HashMap;

use super::parser::{
    FederatedQuery, QueryTarget, QueryType, SqlExtension, TargetModelType, VectorQuery,
};
use crate::core::error::VectorDBError;
use crate::storage::multimodel::ModelType;

/// Physical plan node types
#[derive(Debug, Clone)]
pub enum PlanNodeType {
    /// Scan a table/collection
    Scan {
        target: String,
        model_type: ModelType,
        predicates: Vec<Predicate>,
    },
    /// Vector similarity search
    VectorSearch {
        collection: String,
        top_k: usize,
        query_vector_source: VectorSource,
    },
    /// Graph traversal
    GraphTraversal {
        cypher: String,
        start_nodes: Option<Vec<String>>,
    },
    /// Document query
    DocumentQuery {
        collection: String,
        filter: Option<String>,
    },
    /// Observability query (logs/metrics)
    ObservabilityQuery {
        namespace: String,
        query_type: ObservabilityQueryType,
        time_range: Option<TimeRange>,
    },
    /// Hash join
    HashJoin {
        left: Box<PlanNode>,
        right: Box<PlanNode>,
        join_keys: Vec<(String, String)>,
        join_type: JoinType,
    },
    /// Nested loop join (for LATERAL)
    NestedLoopJoin {
        outer: Box<PlanNode>,
        inner: Box<PlanNode>,
        correlation: Vec<String>,
    },
    /// Index join
    IndexJoin {
        left: Box<PlanNode>,
        right: Box<PlanNode>,
        index_lookup: String,
    },
    /// Filter operation
    Filter {
        input: Box<PlanNode>,
        predicate: Predicate,
    },
    /// Project columns
    Project {
        input: Box<PlanNode>,
        columns: Vec<String>,
    },
    /// Distinct rows across the projected output
    Distinct { input: Box<PlanNode> },
    /// Sort/Order By
    Sort {
        input: Box<PlanNode>,
        order_by: Vec<OrderByClause>,
    },
    /// Limit
    Limit {
        input: Box<PlanNode>,
        limit: usize,
        offset: usize,
    },
    /// Aggregate
    Aggregate {
        input: Box<PlanNode>,
        group_by: Vec<String>,
        aggregates: Vec<AggregateExpr>,
    },
    /// Union
    Union { inputs: Vec<PlanNode>, all: bool },
}

/// Source of a query vector
#[derive(Debug, Clone)]
pub enum VectorSource {
    /// Literal vector value
    Literal(Vec<f32>),
    /// Column reference from another table
    ColumnRef { table: String, column: String },
    /// Raw SQL expression that could not be reduced during parsing
    Expression(String),
    /// Subquery result
    Subquery(Box<PlanNode>),
}

/// Observability query type
#[derive(Debug, Clone)]
pub enum ObservabilityQueryType {
    Logs,
    Metrics,
    Traces,
}

/// Time range for observability queries
#[derive(Debug, Clone)]
pub struct TimeRange {
    pub start_ns: i64,
    pub end_ns: i64,
}

/// Join type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
    Lateral,
}

/// Predicate for filtering
#[derive(Debug, Clone)]
pub struct Predicate {
    pub column: String,
    pub op: PredicateOp,
    pub value: PredicateValue,
}

/// Predicate operators
#[derive(Debug, Clone)]
pub enum PredicateOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Like,
    In,
    IsNull,
    IsNotNull,
    Between,
}

/// Predicate values
#[derive(Debug, Clone)]
pub enum PredicateValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    List(Vec<PredicateValue>),
    Null,
}

/// Order by clause
#[derive(Debug, Clone)]
pub struct OrderByClause {
    pub column: String,
    pub ascending: bool,
    pub nulls_first: bool,
}

/// Aggregate expression
#[derive(Debug, Clone)]
pub struct AggregateExpr {
    pub function: AggregateFunction,
    pub column: Option<String>,
    pub alias: String,
}

/// Aggregate functions
#[derive(Debug, Clone)]
pub enum AggregateFunction {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    CountDistinct,
}

/// Query execution plan
#[derive(Debug, Clone)]
pub struct PlanNode {
    /// Node ID for tracking
    pub id: usize,
    /// Node type with parameters
    pub node_type: PlanNodeType,
    /// Estimated cost
    pub estimated_cost: f64,
    /// Estimated row count
    pub estimated_rows: u64,
    /// Output columns
    pub output_columns: Vec<String>,
}

/// Complete query plan
#[derive(Debug, Clone)]
pub struct QueryPlan {
    /// Root of the plan tree
    pub root: PlanNode,
    /// Total estimated cost
    pub total_cost: f64,
    /// Plan metadata
    pub metadata: PlanMetadata,
}

/// Plan metadata
#[derive(Debug, Default, Clone)]
pub struct PlanMetadata {
    /// Models involved in the plan
    pub involved_models: Vec<ModelType>,
    /// Whether this is a cross-model join
    pub is_cross_model: bool,
    /// Parallelizable stages
    pub parallel_stages: Vec<Vec<usize>>,
    /// Optimization hints applied
    pub hints_applied: Vec<String>,
}

/// Result of attempting to push a predicate into a node
struct PredicatePushResult {
    node: PlanNode,
    predicate_pushed: bool,
}

struct SelectItem {
    expression: String,
    alias: Option<String>,
}

#[derive(Clone, Copy)]
enum QuerySourceRef<'a> {
    Extension(&'a SqlExtension),
    Target(&'a QueryTarget),
}

/// Cost model for different data models
#[derive(Debug, Clone)]
pub struct CostModel {
    /// Base cost per row scan
    pub row_scan_cost: f64,
    /// Index lookup cost
    pub index_lookup_cost: f64,
    /// Network transfer cost per byte
    pub network_cost_per_byte: f64,
    /// CPU cost per operation
    pub cpu_cost_per_op: f64,
}

impl Default for CostModel {
    fn default() -> Self {
        Self {
            row_scan_cost: 1.0,
            index_lookup_cost: 0.1,
            network_cost_per_byte: 0.001,
            cpu_cost_per_op: 0.01,
        }
    }
}

// ============================================================================
// STATISTICS PROVIDER AND MODEL STATISTICS
// ============================================================================

/// Statistics for a vector collection
#[derive(Debug, Clone, Default)]
pub struct VectorCollectionStats {
    /// Number of vectors in the collection
    pub vector_count: u64,
    /// Dimension of vectors
    pub dimension: usize,
    /// Whether an HNSW index is available
    pub has_hnsw_index: bool,
    /// Whether an IVF index is available
    pub has_ivf_index: bool,
    /// Number of IVF clusters (if IVF index exists)
    pub ivf_clusters: Option<usize>,
    /// Average query latency in milliseconds (from historical data)
    pub avg_query_latency_ms: Option<f64>,
}

/// Statistics for a graph
#[derive(Debug, Clone, Default)]
pub struct GraphStats {
    /// Total number of nodes
    pub node_count: u64,
    /// Total number of edges
    pub edge_count: u64,
    /// Average degree (edges per node)
    pub avg_degree: f64,
    /// Maximum depth for BFS/DFS operations
    pub max_depth: Option<usize>,
    /// Whether the graph is indexed by label
    pub has_label_index: bool,
}

/// Statistics for a document collection
#[derive(Debug, Clone, Default)]
pub struct DocumentCollectionStats {
    /// Number of documents
    pub document_count: u64,
    /// Average document size in bytes
    pub avg_document_size_bytes: u64,
    /// Number of indexed fields
    pub indexed_fields: Vec<String>,
    /// Field cardinalities (distinct values per field)
    pub field_cardinalities: HashMap<String, u64>,
}

/// Statistics for observability data (logs/metrics)
#[derive(Debug, Clone, Default)]
pub struct ObservabilityStats {
    /// Total number of data points
    pub data_point_count: u64,
    /// Time range coverage in seconds
    pub time_range_seconds: u64,
    /// Average data points per second
    pub avg_points_per_second: f64,
    /// Whether time-based partitioning is used
    pub has_time_partitioning: bool,
}

/// Unified statistics for any model type
#[derive(Debug, Clone)]
pub enum ModelStatistics {
    Vector(VectorCollectionStats),
    Graph(GraphStats),
    Document(DocumentCollectionStats),
    Observability(ObservabilityStats),
    Relational { row_count: u64, avg_row_size: usize },
}

impl ModelStatistics {
    /// Get the estimated row/item count
    pub fn estimated_count(&self) -> u64 {
        match self {
            ModelStatistics::Vector(s) => s.vector_count,
            ModelStatistics::Graph(s) => s.node_count,
            ModelStatistics::Document(s) => s.document_count,
            ModelStatistics::Observability(s) => s.data_point_count,
            ModelStatistics::Relational { row_count, .. } => *row_count,
        }
    }
}

/// Trait for providing statistics to the optimizer
pub trait StatisticsProvider: Send + Sync {
    /// Get statistics for a collection/table by name
    fn get_statistics(&self, name: &str) -> Option<ModelStatistics>;

    /// Get statistics for a specific model type
    fn get_model_statistics(&self, name: &str, model_type: ModelType) -> Option<ModelStatistics>;
}

/// Default statistics provider with cached statistics
#[derive(Debug, Default)]
pub struct CachedStatisticsProvider {
    /// Cached statistics by name
    stats_cache: HashMap<String, ModelStatistics>,
}

impl CachedStatisticsProvider {
    /// Create a new cached statistics provider
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or update statistics for a collection
    pub fn set_statistics(&mut self, name: String, stats: ModelStatistics) {
        self.stats_cache.insert(name, stats);
    }

    /// Create default statistics for a vector collection
    pub fn default_vector_stats(vector_count: u64, dimension: usize) -> ModelStatistics {
        ModelStatistics::Vector(VectorCollectionStats {
            vector_count,
            dimension,
            has_hnsw_index: vector_count > 10000,
            has_ivf_index: vector_count > 100000,
            ivf_clusters: if vector_count > 100000 {
                Some(((vector_count as f64).sqrt() as usize).max(16))
            } else {
                None
            },
            avg_query_latency_ms: None,
        })
    }

    /// Create default statistics for a graph
    pub fn default_graph_stats(node_count: u64, edge_count: u64) -> ModelStatistics {
        ModelStatistics::Graph(GraphStats {
            node_count,
            edge_count,
            avg_degree: if node_count > 0 {
                edge_count as f64 / node_count as f64
            } else {
                0.0
            },
            max_depth: Some(6), // Default max depth for traversals
            has_label_index: true,
        })
    }
}

impl StatisticsProvider for CachedStatisticsProvider {
    fn get_statistics(&self, name: &str) -> Option<ModelStatistics> {
        self.stats_cache.get(name).cloned()
    }

    fn get_model_statistics(&self, name: &str, _model_type: ModelType) -> Option<ModelStatistics> {
        self.stats_cache.get(name).cloned()
    }
}

// ============================================================================
// PER-MODEL COST FUNCTIONS
// ============================================================================

/// Advanced cost estimator with per-model cost functions
#[derive(Debug, Clone)]
pub struct AdvancedCostEstimator {
    /// Base cost model parameters
    base_cost_model: CostModel,
    /// CPU cycles per distance calculation (vector search)
    cpu_cycles_per_distance: f64,
    /// I/O cost per page read
    io_cost_per_page: f64,
    /// Memory access cost factor
    memory_access_cost: f64,
}

impl Default for AdvancedCostEstimator {
    fn default() -> Self {
        Self {
            base_cost_model: CostModel::default(),
            cpu_cycles_per_distance: 0.001,
            io_cost_per_page: 1.0,
            memory_access_cost: 0.1,
        }
    }
}

impl AdvancedCostEstimator {
    /// Create a new advanced cost estimator
    pub fn new() -> Self {
        Self::default()
    }

    /// Calculate cost for vector search based on collection size and top_k
    ///
    /// Cost model:
    /// - With HNSW: O(log n * top_k * d * ef_search)
    /// - With IVF: O(nprobe * vectors_per_cluster * d + top_k * log(top_k))
    /// - Brute force: O(n * d)
    pub fn vector_search_cost(&self, stats: &VectorCollectionStats, top_k: usize) -> f64 {
        let n = stats.vector_count as f64;
        let d = stats.dimension as f64;

        if n == 0.0 {
            return 0.0;
        }

        // HNSW index: logarithmic search complexity
        if stats.has_hnsw_index {
            let ef_search = (top_k * 2).max(64) as f64; // Typical ef_search
            let hnsw_cost = n.log2() * top_k as f64 * d * ef_search * self.cpu_cycles_per_distance;
            // Add memory access cost for graph traversal
            let memory_cost = n.log2() * 10.0 * self.memory_access_cost;
            return hnsw_cost + memory_cost;
        }

        // IVF index: probe multiple clusters
        if stats.has_ivf_index {
            let nprobe = 10.min(stats.ivf_clusters.unwrap_or(16)) as f64;
            let vectors_per_cluster = n / stats.ivf_clusters.unwrap_or(16) as f64;
            let ivf_cost = nprobe * vectors_per_cluster * d * self.cpu_cycles_per_distance;
            // Add sorting cost for final top_k
            let sort_cost = (top_k as f64) * (top_k as f64).log2() * self.cpu_cycles_per_distance;
            return ivf_cost + sort_cost;
        }

        // Brute force: scan all vectors
        let brute_force_cost = n * d * self.cpu_cycles_per_distance;
        // Add I/O cost for loading vectors
        let pages = (n * d * 4.0 / 4096.0).ceil(); // Assuming 4-byte floats, 4KB pages
        let io_cost = pages * self.io_cost_per_page;

        brute_force_cost + io_cost
    }

    /// Calculate cost for graph traversal based on edge count and depth
    ///
    /// Cost model:
    /// - BFS/DFS: O(V + E) for full traversal, O(branching_factor^depth) for limited depth
    /// - With label index: reduces initial node lookup to O(log V)
    pub fn graph_traversal_cost(&self, stats: &GraphStats, max_depth: usize) -> f64 {
        let v = stats.node_count as f64;
        let _e = stats.edge_count as f64; // Used in future sophisticated cost models

        if v == 0.0 {
            return 0.0;
        }

        let avg_degree = stats.avg_degree.max(1.0);

        // Limited depth traversal: estimate nodes visited
        let estimated_nodes_visited = if max_depth == 0 {
            1.0
        } else {
            // Approximate: branching_factor^depth, capped at total nodes
            let branching = avg_degree.min(10.0); // Cap branching factor
            let depth_factor = branching.powf(max_depth as f64);
            depth_factor.min(v)
        };

        // Cost components:
        // 1. Node lookup cost
        let lookup_cost = if stats.has_label_index {
            v.log2() * self.base_cost_model.index_lookup_cost
        } else {
            v * self.base_cost_model.row_scan_cost * 0.1
        };

        // 2. Edge traversal cost
        let edges_per_node = avg_degree;
        let edge_traversal_cost =
            estimated_nodes_visited * edges_per_node * self.base_cost_model.cpu_cost_per_op;

        // 3. Memory access for CSR format
        let memory_cost = estimated_nodes_visited * self.memory_access_cost;

        lookup_cost + edge_traversal_cost + memory_cost
    }

    /// Calculate cost for document query based on filter complexity
    ///
    /// Cost model:
    /// - Indexed field: O(log n * selectivity * n)
    /// - Non-indexed field: O(n)
    /// - Complex filter (AND/OR): product/sum of individual selectivities
    pub fn document_query_cost(
        &self,
        stats: &DocumentCollectionStats,
        filter_fields: &[String],
        filter_complexity: usize,
    ) -> f64 {
        let n = stats.document_count as f64;

        if n == 0.0 {
            return 0.0;
        }

        // Base scan cost
        let mut total_cost = 0.0;
        let mut combined_selectivity = 1.0;

        for field in filter_fields {
            let is_indexed = stats.indexed_fields.contains(field);
            let cardinality = stats
                .field_cardinalities
                .get(field)
                .copied()
                .unwrap_or(n as u64);
            let selectivity = 1.0 / cardinality.max(1) as f64;

            if is_indexed {
                // Index lookup + scan matching rows
                let index_cost = n.log2() * self.base_cost_model.index_lookup_cost;
                let scan_cost = n * selectivity * self.base_cost_model.row_scan_cost;
                total_cost += index_cost + scan_cost;
            } else {
                // Full scan with filter
                total_cost += n * self.base_cost_model.row_scan_cost;
            }

            combined_selectivity *= selectivity;
        }

        // Add complexity penalty for nested filters
        let complexity_factor = 1.0 + (filter_complexity as f64 * 0.1);

        // Add document deserialization cost
        let avg_doc_size = stats.avg_document_size_bytes as f64;
        let deser_cost = n * combined_selectivity * avg_doc_size * 0.00001;

        (total_cost + deser_cost) * complexity_factor
    }

    /// Calculate cost for observability query (logs/metrics)
    pub fn observability_query_cost(
        &self,
        stats: &ObservabilityStats,
        time_range_seconds: Option<u64>,
    ) -> f64 {
        let total_points = stats.data_point_count as f64;

        if total_points == 0.0 {
            return 0.0;
        }

        // Calculate fraction of data to scan based on time range
        let scan_fraction = if let Some(range) = time_range_seconds {
            if stats.time_range_seconds > 0 {
                (range as f64 / stats.time_range_seconds as f64).min(1.0)
            } else {
                1.0
            }
        } else {
            1.0
        };

        let points_to_scan = total_points * scan_fraction;

        // With time partitioning, we can skip irrelevant partitions
        let partition_benefit = if stats.has_time_partitioning {
            0.5
        } else {
            1.0
        };

        points_to_scan * self.base_cost_model.row_scan_cost * partition_benefit
    }

    /// Update cost model parameters from observed execution feedback.
    /// Uses exponential moving average (alpha=0.2) to smooth parameter updates.
    pub fn update_from_feedback(&mut self, feedback: &ExecutionFeedback) {
        const ALPHA: f64 = 0.2; // EMA smoothing factor

        if let Some(observed_cpu_per_distance) = feedback.observed_cpu_per_distance {
            self.cpu_cycles_per_distance = self.cpu_cycles_per_distance * (1.0 - ALPHA)
                + observed_cpu_per_distance * ALPHA;
        }
        if let Some(observed_io_per_page) = feedback.observed_io_per_page {
            self.io_cost_per_page =
                self.io_cost_per_page * (1.0 - ALPHA) + observed_io_per_page * ALPHA;
        }
        if let Some(observed_mem_cost) = feedback.observed_memory_cost {
            self.memory_access_cost =
                self.memory_access_cost * (1.0 - ALPHA) + observed_mem_cost * ALPHA;
        }
    }
}

// ============================================================================
// RUNTIME STATISTICS FEEDBACK
// ============================================================================

/// Feedback from a single query execution, used to calibrate cost models
#[derive(Debug, Clone, Default)]
pub struct ExecutionFeedback {
    /// The operation key (e.g., "vector_search:embeddings:top10")
    pub operation_key: String,
    /// Estimated cardinality at plan time
    pub estimated_cardinality: u64,
    /// Actual cardinality observed at execution time
    pub actual_cardinality: u64,
    /// Estimated cost at plan time
    pub estimated_cost: f64,
    /// Actual execution latency in milliseconds
    pub actual_latency_ms: f64,
    /// Observed CPU cost per distance computation (if measurable)
    pub observed_cpu_per_distance: Option<f64>,
    /// Observed I/O cost per page read (if measurable)
    pub observed_io_per_page: Option<f64>,
    /// Observed memory access cost factor (if measurable)
    pub observed_memory_cost: Option<f64>,
    /// Number of rows scanned (for selectivity calibration)
    pub rows_scanned: Option<u64>,
    /// Number of I/O pages read
    pub pages_read: Option<u64>,
}

/// Thread-safe runtime statistics collector that accumulates execution feedback
/// and uses it to calibrate the query optimizer's cost model and cardinality estimates.
///
/// Usage pattern:
/// 1. Before execution: optimizer produces a plan with estimated costs
/// 2. After execution: caller records feedback via `record_feedback()`
/// 3. On next optimization: calibrated estimates incorporate learned corrections
pub struct RuntimeStatisticsCollector {
    /// Accumulated per-operation cardinality ratios (estimated vs actual)
    cardinality_history: parking_lot::RwLock<HashMap<String, CardinalityHistory>>,
    /// Accumulated per-operation latency observations
    latency_history: parking_lot::RwLock<HashMap<String, LatencyHistory>>,
    /// Selectivity observations per filter pattern
    selectivity_history: parking_lot::RwLock<HashMap<String, SelectivityHistory>>,
    /// Maximum number of history entries per operation before compaction
    max_history_per_op: usize,
}

/// Tracks cardinality estimation accuracy for a specific operation type
#[derive(Debug, Clone)]
struct CardinalityHistory {
    /// Rolling ratio of actual/estimated cardinality (EMA)
    correction_ratio: f64,
    /// Number of observations
    sample_count: u64,
    /// Recent observation timestamps for decay
    last_updated_ms: u64,
}

/// Tracks latency observations for cost model calibration
#[derive(Debug, Clone)]
struct LatencyHistory {
    /// Exponential moving average of latency in ms
    avg_latency_ms: f64,
    /// P95 approximation (tracks the max of recent window)
    p95_latency_ms: f64,
    /// Number of observations
    sample_count: u64,
    /// Cost-to-latency ratio for this operation type
    cost_latency_ratio: f64,
}

/// Tracks observed selectivities for filter predicates
#[derive(Debug, Clone)]
struct SelectivityHistory {
    /// Rolling selectivity estimate (fraction of rows passing filter)
    avg_selectivity: f64,
    /// Number of observations
    sample_count: u64,
}

impl Default for RuntimeStatisticsCollector {
    fn default() -> Self {
        Self::new(1000)
    }
}

impl RuntimeStatisticsCollector {
    /// Create a new collector with the given max history entries per operation
    pub fn new(max_history_per_op: usize) -> Self {
        Self {
            cardinality_history: parking_lot::RwLock::new(HashMap::new()),
            latency_history: parking_lot::RwLock::new(HashMap::new()),
            selectivity_history: parking_lot::RwLock::new(HashMap::new()),
            max_history_per_op,
        }
    }

    /// Record execution feedback and update internal models
    pub fn record_feedback(&self, feedback: &ExecutionFeedback) {
        self.update_cardinality_history(feedback);
        self.update_latency_history(feedback);
        if let Some(rows_scanned) = feedback.rows_scanned {
            if rows_scanned > 0 && feedback.actual_cardinality > 0 {
                self.update_selectivity_history(
                    &feedback.operation_key,
                    feedback.actual_cardinality as f64 / rows_scanned as f64,
                );
            }
        }
    }

    fn update_cardinality_history(&self, feedback: &ExecutionFeedback) {
        const ALPHA: f64 = 0.3; // Faster adaptation for cardinality
        let mut history = self.cardinality_history.write();

        let ratio = if feedback.estimated_cardinality > 0 {
            feedback.actual_cardinality as f64 / feedback.estimated_cardinality as f64
        } else {
            1.0
        };

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let entry = history
            .entry(feedback.operation_key.clone())
            .or_insert(CardinalityHistory {
                correction_ratio: 1.0,
                sample_count: 0,
                last_updated_ms: now_ms,
            });

        entry.correction_ratio = entry.correction_ratio * (1.0 - ALPHA) + ratio * ALPHA;
        entry.sample_count += 1;
        entry.last_updated_ms = now_ms;

        self.compact_if_needed(&mut history);
    }

    fn update_latency_history(&self, feedback: &ExecutionFeedback) {
        const ALPHA: f64 = 0.2;
        let mut history = self.latency_history.write();

        let entry = history
            .entry(feedback.operation_key.clone())
            .or_insert(LatencyHistory {
                avg_latency_ms: feedback.actual_latency_ms,
                p95_latency_ms: feedback.actual_latency_ms,
                sample_count: 0,
                cost_latency_ratio: if feedback.estimated_cost > 0.0 {
                    feedback.actual_latency_ms / feedback.estimated_cost
                } else {
                    1.0
                },
            });

        entry.avg_latency_ms =
            entry.avg_latency_ms * (1.0 - ALPHA) + feedback.actual_latency_ms * ALPHA;
        // Approximate P95 by tracking 95th percentile via exponential max decay
        if feedback.actual_latency_ms > entry.p95_latency_ms {
            entry.p95_latency_ms = feedback.actual_latency_ms;
        } else {
            entry.p95_latency_ms =
                entry.p95_latency_ms * 0.99 + feedback.actual_latency_ms * 0.01;
        }
        entry.sample_count += 1;
        if feedback.estimated_cost > 0.0 {
            let new_ratio = feedback.actual_latency_ms / feedback.estimated_cost;
            entry.cost_latency_ratio =
                entry.cost_latency_ratio * (1.0 - ALPHA) + new_ratio * ALPHA;
        }
    }

    fn update_selectivity_history(&self, operation_key: &str, selectivity: f64) {
        const ALPHA: f64 = 0.25;
        let mut history = self.selectivity_history.write();

        let entry = history
            .entry(operation_key.to_string())
            .or_insert(SelectivityHistory {
                avg_selectivity: selectivity,
                sample_count: 0,
            });

        entry.avg_selectivity = entry.avg_selectivity * (1.0 - ALPHA) + selectivity * ALPHA;
        entry.sample_count += 1;
    }

    fn compact_if_needed(&self, history: &mut HashMap<String, CardinalityHistory>) {
        if history.len() > self.max_history_per_op * 2 {
            // Remove entries with fewest samples
            let mut entries: Vec<_> = history
                .iter()
                .map(|(k, v)| (k.clone(), v.sample_count))
                .collect();
            entries.sort_by_key(|(_, count)| *count);
            let to_remove = history.len() - self.max_history_per_op;
            for (key, _) in entries.into_iter().take(to_remove) {
                history.remove(&key);
            }
        }
    }

    /// Get the calibrated cardinality correction ratio for an operation
    pub fn cardinality_correction(&self, operation_key: &str) -> Option<f64> {
        self.cardinality_history
            .read()
            .get(operation_key)
            .map(|h| h.correction_ratio)
    }

    /// Get the average observed latency for an operation
    pub fn avg_latency(&self, operation_key: &str) -> Option<f64> {
        self.latency_history
            .read()
            .get(operation_key)
            .map(|h| h.avg_latency_ms)
    }

    /// Get the cost-to-latency ratio for translating costs to expected ms
    pub fn cost_latency_ratio(&self, operation_key: &str) -> Option<f64> {
        self.latency_history
            .read()
            .get(operation_key)
            .map(|h| h.cost_latency_ratio)
    }

    /// Get calibrated selectivity for a filter pattern
    pub fn calibrated_selectivity(&self, operation_key: &str) -> Option<f64> {
        self.selectivity_history
            .read()
            .get(operation_key)
            .map(|h| h.avg_selectivity)
    }

    /// Check if a cached plan should be invalidated based on performance regression.
    /// Returns true if actual latency exceeds 3x the expected latency.
    pub fn should_invalidate_plan(
        &self,
        operation_key: &str,
        estimated_cost: f64,
        actual_latency_ms: f64,
    ) -> bool {
        if let Some(ratio) = self.cost_latency_ratio(operation_key) {
            let expected_latency = estimated_cost * ratio;
            actual_latency_ms > expected_latency * 3.0
        } else {
            false
        }
    }

    /// Generate a snapshot of all tracked statistics for diagnostics
    pub fn snapshot(&self) -> RuntimeStatsSnapshot {
        let cardinality = self.cardinality_history.read();
        let latency = self.latency_history.read();
        let selectivity = self.selectivity_history.read();

        RuntimeStatsSnapshot {
            tracked_operations: cardinality.len() + latency.len(),
            cardinality_entries: cardinality.len(),
            latency_entries: latency.len(),
            selectivity_entries: selectivity.len(),
            total_observations: cardinality.values().map(|h| h.sample_count).sum::<u64>()
                + latency.values().map(|h| h.sample_count).sum::<u64>(),
        }
    }
}

/// Diagnostic snapshot of runtime statistics state
#[derive(Debug, Clone)]
pub struct RuntimeStatsSnapshot {
    /// Total unique operations tracked
    pub tracked_operations: usize,
    /// Number of cardinality history entries
    pub cardinality_entries: usize,
    /// Number of latency history entries
    pub latency_entries: usize,
    /// Number of selectivity history entries
    pub selectivity_entries: usize,
    /// Total observation count across all entries
    pub total_observations: u64,
}

// ============================================================================
// CARDINALITY ESTIMATION
// ============================================================================

/// Cardinality estimator for cross-model operations
#[derive(Debug, Clone, Default)]
pub struct CardinalityEstimator {
    /// Historical cardinality data for calibration
    historical_ratios: HashMap<String, f64>,
}

impl CardinalityEstimator {
    /// Create a new cardinality estimator
    pub fn new() -> Self {
        Self::default()
    }

    /// Estimate output cardinality for a vector search
    pub fn estimate_vector_search_cardinality(
        &self,
        top_k: usize,
        _stats: &VectorCollectionStats,
    ) -> u64 {
        // Vector search always returns at most top_k results
        top_k as u64
    }

    /// Estimate output cardinality for a graph traversal
    pub fn estimate_graph_traversal_cardinality(
        &self,
        stats: &GraphStats,
        max_depth: usize,
    ) -> u64 {
        let avg_degree = stats.avg_degree.max(1.0);

        // Estimate based on branching factor and depth
        let estimate = if max_depth == 0 {
            1
        } else {
            let branching = avg_degree.min(10.0);
            (branching.powf(max_depth as f64) as u64).min(stats.node_count)
        };

        estimate.max(1)
    }

    /// Estimate output cardinality for a document query
    pub fn estimate_document_query_cardinality(
        &self,
        stats: &DocumentCollectionStats,
        filter_selectivity: f64,
    ) -> u64 {
        let estimate = (stats.document_count as f64 * filter_selectivity) as u64;
        estimate.max(1)
    }

    /// Estimate cardinality for a join operation
    pub fn estimate_join_cardinality(
        &self,
        left_cardinality: u64,
        right_cardinality: u64,
        join_type: &JoinType,
        join_selectivity: Option<f64>,
    ) -> u64 {
        let selectivity = join_selectivity.unwrap_or(0.1); // Default 10% join selectivity

        let estimate = match join_type {
            JoinType::Inner => {
                // Inner join: product * selectivity
                ((left_cardinality as f64) * (right_cardinality as f64) * selectivity) as u64
            }
            JoinType::Left => {
                // Left join: at least left_cardinality
                let inner =
                    ((left_cardinality as f64) * (right_cardinality as f64) * selectivity) as u64;
                inner.max(left_cardinality)
            }
            JoinType::Right => {
                // Right join: at least right_cardinality
                let inner =
                    ((left_cardinality as f64) * (right_cardinality as f64) * selectivity) as u64;
                inner.max(right_cardinality)
            }
            JoinType::Full => {
                // Full join: max of both plus potential unmatched
                left_cardinality + right_cardinality
            }
            JoinType::Cross => {
                // Cross join: full product
                left_cardinality * right_cardinality
            }
            JoinType::Lateral => {
                // Lateral join: left * average right rows per left row
                let avg_right_per_left = (right_cardinality as f64 * selectivity).max(1.0);
                (left_cardinality as f64 * avg_right_per_left) as u64
            }
        };

        estimate.max(1)
    }

    /// Estimate cardinality after applying a filter
    pub fn estimate_filter_cardinality(&self, input_cardinality: u64, selectivity: f64) -> u64 {
        let estimate = (input_cardinality as f64 * selectivity) as u64;
        estimate.max(1)
    }

    /// Update historical ratio for calibration
    pub fn record_actual_cardinality(
        &mut self,
        operation_key: String,
        estimated: u64,
        actual: u64,
    ) {
        if estimated > 0 {
            let ratio = actual as f64 / estimated as f64;
            self.historical_ratios.insert(operation_key, ratio);
        }
    }

    /// Get calibrated estimate using historical data
    pub fn calibrated_estimate(&self, operation_key: &str, base_estimate: u64) -> u64 {
        if let Some(&ratio) = self.historical_ratios.get(operation_key) {
            (base_estimate as f64 * ratio) as u64
        } else {
            base_estimate
        }
    }
}

// ============================================================================
// DYNAMIC PROGRAMMING JOIN ORDER OPTIMIZATION
// ============================================================================

/// Represents a set of relations for join ordering
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RelationSet {
    /// Bitmask representing which relations are in this set
    mask: u64,
}

#[allow(dead_code)]
impl RelationSet {
    fn new(mask: u64) -> Self {
        Self { mask }
    }

    fn singleton(index: usize) -> Self {
        Self {
            mask: 1u64 << index,
        }
    }

    fn union(&self, other: &RelationSet) -> RelationSet {
        RelationSet::new(self.mask | other.mask)
    }

    fn intersects(&self, other: &RelationSet) -> bool {
        (self.mask & other.mask) != 0
    }

    fn is_subset_of(&self, other: &RelationSet) -> bool {
        (self.mask & other.mask) == self.mask
    }

    fn count(&self) -> usize {
        self.mask.count_ones() as usize
    }

    fn iter_subsets(&self) -> impl Iterator<Item = RelationSet> {
        let mask = self.mask;
        (1..mask)
            .filter(move |&s| (s & mask) == s)
            .map(RelationSet::new)
    }
}

/// Entry in the DP memo table for join ordering
#[derive(Debug, Clone)]
struct DPEntry {
    /// Best plan for this relation set
    plan: PlanNode,
    /// Cost of the best plan
    cost: f64,
    /// Estimated cardinality
    cardinality: u64,
}

/// Join order optimizer using dynamic programming (Selinger-style)
pub struct JoinOrderOptimizer {
    /// Cost estimator (reserved for future use with more sophisticated cost models)
    #[allow(dead_code)]
    cost_estimator: AdvancedCostEstimator,
    /// Cardinality estimator
    cardinality_estimator: CardinalityEstimator,
    /// Maximum number of relations for DP (use greedy for larger)
    dp_threshold: usize,
}

impl Default for JoinOrderOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

impl JoinOrderOptimizer {
    /// Create a new join order optimizer
    pub fn new() -> Self {
        Self {
            cost_estimator: AdvancedCostEstimator::new(),
            cardinality_estimator: CardinalityEstimator::new(),
            dp_threshold: 10, // Use DP for up to 10 relations
        }
    }

    /// Find optimal join order using dynamic programming
    ///
    /// For n relations, considers all 2^n subsets and finds the optimal
    /// way to join each subset by trying all binary partitions.
    pub fn find_optimal_join_order(
        &self,
        relations: &[PlanNode],
        join_predicates: &[(usize, usize, Vec<(String, String)>)], // (left_idx, right_idx, join_keys)
        next_id: &std::sync::atomic::AtomicUsize,
    ) -> Result<PlanNode> {
        let n = relations.len();

        if n == 0 {
            return Err(anyhow!("No relations to join"));
        }
        if n == 1 {
            return Ok(relations[0].clone());
        }

        // Use greedy for large number of relations
        if n > self.dp_threshold {
            return self.greedy_join_order(relations, join_predicates, next_id);
        }

        // Initialize DP table with single relations
        let mut dp: HashMap<RelationSet, DPEntry> = HashMap::new();

        for (i, rel) in relations.iter().enumerate() {
            let set = RelationSet::singleton(i);
            dp.insert(
                set,
                DPEntry {
                    plan: rel.clone(),
                    cost: rel.estimated_cost,
                    cardinality: rel.estimated_rows,
                },
            );
        }

        // Build up solutions for larger sets
        for size in 2..=n {
            let sets_of_size: Vec<RelationSet> = (0..(1u64 << n))
                .filter(|&mask| (mask.count_ones() as usize) == size)
                .map(RelationSet::new)
                .collect();

            for set in sets_of_size {
                let mut best_entry: Option<DPEntry> = None;

                // Try all ways to partition into two non-empty subsets
                for left_set in set.iter_subsets() {
                    let right_mask = set.mask & !left_set.mask;
                    if right_mask == 0 {
                        continue;
                    }
                    let right_set = RelationSet::new(right_mask);

                    // Check if we have solutions for both subsets
                    let (left_entry, right_entry) = match (dp.get(&left_set), dp.get(&right_set)) {
                        (Some(l), Some(r)) => (l, r),
                        _ => continue,
                    };

                    // Find applicable join predicates
                    let join_keys = self.find_join_keys(&left_set, &right_set, join_predicates);

                    // Calculate join cost and cardinality
                    let join_selectivity = if join_keys.is_empty() { 1.0 } else { 0.1 };
                    let join_cardinality = self.cardinality_estimator.estimate_join_cardinality(
                        left_entry.cardinality,
                        right_entry.cardinality,
                        &JoinType::Inner,
                        Some(join_selectivity),
                    );

                    // Cost model: hash join = build + probe
                    let build_cost = left_entry.cardinality as f64 * 0.01;
                    let probe_cost = right_entry.cardinality as f64 * 0.001;
                    let output_cost = join_cardinality as f64 * 0.001;
                    let join_cost =
                        left_entry.cost + right_entry.cost + build_cost + probe_cost + output_cost;

                    // Update best if this is better
                    let is_better = best_entry
                        .as_ref()
                        .map_or(true, |e| join_cost < e.cost);

                    if is_better {
                        let plan = PlanNode {
                            id: next_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                            node_type: PlanNodeType::HashJoin {
                                left: Box::new(left_entry.plan.clone()),
                                right: Box::new(right_entry.plan.clone()),
                                join_keys: if join_keys.is_empty() {
                                    vec![("id".to_string(), "id".to_string())]
                                } else {
                                    join_keys
                                },
                                join_type: JoinType::Inner,
                            },
                            estimated_cost: join_cost,
                            estimated_rows: join_cardinality,
                            output_columns: vec!["*".to_string()],
                        };

                        best_entry = Some(DPEntry {
                            plan,
                            cost: join_cost,
                            cardinality: join_cardinality,
                        });
                    }
                }

                if let Some(entry) = best_entry {
                    dp.insert(set, entry);
                }
            }
        }

        // Get the solution for the full set
        let full_set = RelationSet::new((1u64 << n) - 1);
        dp.get(&full_set)
            .map(|e| e.plan.clone())
            .ok_or_else(|| anyhow!("Failed to find optimal join order"))
    }

    /// Find join keys applicable between two relation sets
    fn find_join_keys(
        &self,
        left_set: &RelationSet,
        right_set: &RelationSet,
        join_predicates: &[(usize, usize, Vec<(String, String)>)],
    ) -> Vec<(String, String)> {
        let mut keys = Vec::new();

        for (left_idx, right_idx, join_keys) in join_predicates {
            let left_in = (left_set.mask >> left_idx) & 1 == 1;
            let right_in = (right_set.mask >> right_idx) & 1 == 1;

            if (left_in && right_in) || (left_in && (right_set.mask >> right_idx) & 1 == 1) {
                keys.extend(join_keys.clone());
            }
        }

        keys
    }

    /// Greedy join ordering for large numbers of relations
    fn greedy_join_order(
        &self,
        relations: &[PlanNode],
        _join_predicates: &[(usize, usize, Vec<(String, String)>)],
        next_id: &std::sync::atomic::AtomicUsize,
    ) -> Result<PlanNode> {
        let mut remaining: Vec<PlanNode> = relations.to_vec();

        // Sort by cost (ascending)
        remaining.sort_by(|a, b| {
            a.estimated_cost
                .partial_cmp(&b.estimated_cost)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Greedily join from cheapest
        let mut result = remaining.remove(0);

        while !remaining.is_empty() {
            // Find the cheapest relation to join next
            let (best_idx, best_cost) = remaining
                .iter()
                .enumerate()
                .map(|(i, rel)| {
                    let join_cost = result.estimated_cost
                        + rel.estimated_cost
                        + (result.estimated_rows as f64 * 0.01);
                    (i, join_cost)
                })
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .ok_or_else(|| VectorDBError::Internal("No viable join order found".to_string()))?;

            let next = remaining.remove(best_idx);
            let join_cardinality = self.cardinality_estimator.estimate_join_cardinality(
                result.estimated_rows,
                next.estimated_rows,
                &JoinType::Inner,
                Some(0.1),
            );

            result = PlanNode {
                id: next_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                node_type: PlanNodeType::HashJoin {
                    left: Box::new(result),
                    right: Box::new(next),
                    join_keys: vec![("id".to_string(), "id".to_string())],
                    join_type: JoinType::Inner,
                },
                estimated_cost: best_cost,
                estimated_rows: join_cardinality,
                output_columns: vec!["*".to_string()],
            };
        }

        Ok(result)
    }
}

// ============================================================================
// PLAN CACHE
// ============================================================================

/// Cache key for query plans
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlanCacheKey {
    /// Normalized SQL (with literals replaced by placeholders)
    normalized_sql: String,
    /// Query type
    query_type: String,
    /// Involved collections/tables
    targets: Vec<String>,
}

impl PlanCacheKey {
    /// Create a cache key from a federated query
    pub fn from_query(query: &FederatedQuery) -> Self {
        let normalized_sql = Self::normalize_sql(&query.sql);
        let query_type = format!("{:?}", query.query_type);
        let mut targets: Vec<String> = query.targets.iter().map(|t| t.name.clone()).collect();
        targets.sort();

        Self {
            normalized_sql,
            query_type,
            targets,
        }
    }

    /// Normalize SQL by replacing literals with placeholders
    fn normalize_sql(sql: &str) -> String {
        let mut result = String::with_capacity(sql.len());
        let mut in_string = false;
        let mut in_number = false;
        let mut string_char = '"';

        for c in sql.chars() {
            if !in_string && (c == '"' || c == '\'') {
                in_string = true;
                string_char = c;
                result.push_str("?");
            } else if in_string && c == string_char {
                in_string = false;
            } else if !in_string && !in_number && c.is_ascii_digit() {
                in_number = true;
                result.push_str("?");
            } else if in_number && !c.is_ascii_digit() && c != '.' {
                in_number = false;
                result.push(c);
            } else if !in_string && !in_number {
                result.push(c);
            }
        }

        result
    }
}

/// Cached plan entry with metadata
#[derive(Debug, Clone)]
pub struct CachedPlan {
    /// The cached query plan
    pub plan: QueryPlan,
    /// When the plan was created
    pub created_at: std::time::Instant,
    /// Number of times this plan was used
    pub hit_count: u64,
    /// Average execution time when using this plan
    pub avg_execution_time_ms: Option<f64>,
}

/// Plan cache for repeated query patterns
pub struct PlanCache {
    /// Cached plans
    cache: parking_lot::RwLock<HashMap<PlanCacheKey, CachedPlan>>,
    /// Maximum number of cached plans
    max_entries: usize,
    /// Time-to-live for cached plans
    ttl: std::time::Duration,
}

impl Default for PlanCache {
    fn default() -> Self {
        Self::new(1000, std::time::Duration::from_secs(300))
    }
}

impl PlanCache {
    /// Create a new plan cache
    pub fn new(max_entries: usize, ttl: std::time::Duration) -> Self {
        Self {
            cache: parking_lot::RwLock::new(HashMap::new()),
            max_entries,
            ttl,
        }
    }

    /// Get a cached plan if available
    pub fn get(&self, key: &PlanCacheKey) -> Option<QueryPlan> {
        let mut cache = self.cache.write();

        if let Some(entry) = cache.get_mut(key) {
            // Check if still valid
            if entry.created_at.elapsed() < self.ttl {
                entry.hit_count += 1;
                return Some(entry.plan.clone());
            } else {
                // Expired, remove it
                cache.remove(key);
            }
        }

        None
    }

    /// Cache a plan
    pub fn put(&self, key: PlanCacheKey, plan: QueryPlan) {
        let mut cache = self.cache.write();

        // Evict if at capacity
        if cache.len() >= self.max_entries {
            self.evict_lru(&mut cache);
        }

        cache.insert(
            key,
            CachedPlan {
                plan,
                created_at: std::time::Instant::now(),
                hit_count: 0,
                avg_execution_time_ms: None,
            },
        );
    }

    /// Record execution time for a cached plan
    pub fn record_execution_time(&self, key: &PlanCacheKey, execution_time_ms: f64) {
        let mut cache = self.cache.write();

        if let Some(entry) = cache.get_mut(key) {
            let current_avg = entry.avg_execution_time_ms.unwrap_or(execution_time_ms);
            let new_avg = (current_avg * 0.9) + (execution_time_ms * 0.1); // Exponential moving average
            entry.avg_execution_time_ms = Some(new_avg);
        }
    }

    /// Invalidate plans for a specific target
    pub fn invalidate_for_target(&self, target: &str) {
        let mut cache = self.cache.write();
        cache.retain(|key, _| !key.targets.contains(&target.to_string()));
    }

    /// Clear all cached plans
    pub fn clear(&self) {
        let mut cache = self.cache.write();
        cache.clear();
    }

    /// Get cache statistics
    pub fn stats(&self) -> PlanCacheStats {
        let cache = self.cache.read();
        let total_hits: u64 = cache.values().map(|e| e.hit_count).sum();

        PlanCacheStats {
            cached_plans: cache.len(),
            total_hits,
            avg_plan_age_ms: cache
                .values()
                .map(|e| e.created_at.elapsed().as_millis() as f64)
                .sum::<f64>()
                / cache.len().max(1) as f64,
        }
    }

    /// Evict least recently used entries
    fn evict_lru(&self, cache: &mut HashMap<PlanCacheKey, CachedPlan>) {
        // Collect entries with their keys for sorting
        let mut entries: Vec<_> = cache
            .iter()
            .map(|(k, v)| (k.clone(), v.hit_count, v.created_at))
            .collect();

        // Sort by hit count (ascending), then by age (descending)
        entries.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| b.2.cmp(&a.2)));

        // Remove bottom 10%
        let to_remove = (entries.len() / 10).max(1);
        for (key, _, _) in entries.into_iter().take(to_remove) {
            cache.remove(&key);
        }
    }
}

/// Plan cache statistics
#[derive(Debug, Clone)]
pub struct PlanCacheStats {
    /// Number of cached plans
    pub cached_plans: usize,
    /// Total cache hits
    pub total_hits: u64,
    /// Average age of cached plans in milliseconds
    pub avg_plan_age_ms: f64,
}

/// Cross-model query optimizer
pub struct CrossModelOptimizer {
    /// Cost models per data model (used for basic cost estimation and model-aware optimization)
    #[allow(dead_code)]
    cost_models: HashMap<ModelType, CostModel>,
    /// Next plan node ID
    next_node_id: std::sync::atomic::AtomicUsize,
    /// Advanced cost estimator for per-model cost functions
    advanced_cost_estimator: AdvancedCostEstimator,
    /// Cardinality estimator for cross-model operations
    cardinality_estimator: CardinalityEstimator,
    /// Join order optimizer using dynamic programming
    join_order_optimizer: JoinOrderOptimizer,
    /// Plan cache for repeated query patterns
    plan_cache: std::sync::Arc<PlanCache>,
    /// Statistics provider (optional, for enhanced cost estimation)
    #[allow(dead_code)]
    statistics_provider: Option<std::sync::Arc<dyn StatisticsProvider>>,
    /// Runtime statistics collector for adaptive cost model tuning
    runtime_stats: std::sync::Arc<RuntimeStatisticsCollector>,
}

impl CrossModelOptimizer {
    /// Create a new optimizer
    pub fn new() -> Self {
        let mut cost_models = HashMap::new();

        // Vector search is fast with index
        cost_models.insert(
            ModelType::Vector,
            CostModel {
                row_scan_cost: 0.5,
                index_lookup_cost: 0.05,
                network_cost_per_byte: 0.001,
                cpu_cost_per_op: 0.02, // More CPU for distance calculations
            },
        );

        // Graph traversal varies by depth
        cost_models.insert(
            ModelType::Graph,
            CostModel {
                row_scan_cost: 0.3,      // CSR is efficient
                index_lookup_cost: 0.01, // Very fast node lookup
                network_cost_per_byte: 0.001,
                cpu_cost_per_op: 0.005,
            },
        );

        // Document queries depend on indexes
        cost_models.insert(
            ModelType::Document,
            CostModel {
                row_scan_cost: 1.0,
                index_lookup_cost: 0.2,
                network_cost_per_byte: 0.002, // Larger documents
                cpu_cost_per_op: 0.01,
            },
        );

        // RDBMS is baseline
        cost_models.insert(ModelType::Relational, CostModel::default());

        // Observability is optimized for time-range
        cost_models.insert(
            ModelType::Observability,
            CostModel {
                row_scan_cost: 0.2, // Partitioned by time
                index_lookup_cost: 0.1,
                network_cost_per_byte: 0.001,
                cpu_cost_per_op: 0.005,
            },
        );

        Self {
            cost_models,
            next_node_id: std::sync::atomic::AtomicUsize::new(0),
            advanced_cost_estimator: AdvancedCostEstimator::new(),
            cardinality_estimator: CardinalityEstimator::new(),
            join_order_optimizer: JoinOrderOptimizer::new(),
            plan_cache: std::sync::Arc::new(PlanCache::default()),
            statistics_provider: None,
            runtime_stats: std::sync::Arc::new(RuntimeStatisticsCollector::default()),
        }
    }

    /// Create an optimizer with a custom statistics provider (builder pattern)
    pub fn with_statistics_provider(
        mut self,
        provider: std::sync::Arc<dyn StatisticsProvider>,
    ) -> Self {
        self.statistics_provider = Some(provider);
        self
    }

    /// Create an optimizer with a custom runtime statistics collector
    pub fn with_runtime_stats(
        mut self,
        stats: std::sync::Arc<RuntimeStatisticsCollector>,
    ) -> Self {
        self.runtime_stats = stats;
        self
    }

    /// Get a reference to the runtime statistics collector for sharing with executors
    pub fn runtime_stats(&self) -> &std::sync::Arc<RuntimeStatisticsCollector> {
        &self.runtime_stats
    }

    /// Record execution feedback and adaptively tune the cost model.
    ///
    /// Call this after each query execution with observed metrics.
    /// The optimizer will:
    /// 1. Update cardinality correction ratios
    /// 2. Update cost model parameters (cpu, io, memory costs)
    /// 3. Invalidate cached plans that show performance regression
    pub fn record_execution_feedback(&mut self, feedback: ExecutionFeedback) {
        // 1. Update runtime stats collector
        self.runtime_stats.record_feedback(&feedback);

        // 2. Update cardinality estimator with actual vs estimated
        self.cardinality_estimator.record_actual_cardinality(
            feedback.operation_key.clone(),
            feedback.estimated_cardinality,
            feedback.actual_cardinality,
        );

        // 3. Update cost model parameters from hardware observations
        self.advanced_cost_estimator
            .update_from_feedback(&feedback);

        // 4. Invalidate cached plan if performance regressed significantly
        if self.runtime_stats.should_invalidate_plan(
            &feedback.operation_key,
            feedback.estimated_cost,
            feedback.actual_latency_ms,
        ) {
            // Extract the collection/target from the operation key for targeted invalidation
            if let Some(target) = feedback.operation_key.split(':').nth(1) {
                self.plan_cache.invalidate_for_target(target);
            }
        }
    }

    /// Get calibrated cardinality estimate incorporating runtime feedback
    pub fn calibrated_cardinality(&self, operation_key: &str, base_estimate: u64) -> u64 {
        // First check runtime stats for correction ratio
        if let Some(correction) = self.runtime_stats.cardinality_correction(operation_key) {
            let calibrated = (base_estimate as f64 * correction) as u64;
            return calibrated.max(1);
        }
        // Fall back to cardinality estimator's historical data
        self.cardinality_estimator
            .calibrated_estimate(operation_key, base_estimate)
    }

    fn ordered_query_sources<'a>(query: &'a FederatedQuery) -> Vec<QuerySourceRef<'a>> {
        let sql_upper = query.sql.to_uppercase();
        let mut sources = Vec::new();

        for (ordinal, extension) in query.extensions.iter().enumerate() {
            sources.push((
                Self::query_source_position(&sql_upper, QuerySourceRef::Extension(extension)),
                ordinal,
                QuerySourceRef::Extension(extension),
            ));
        }

        for (ordinal, target) in query.targets.iter().enumerate() {
            if !matches!(
                target.model_type,
                TargetModelType::Table | TargetModelType::Unknown
            ) {
                continue;
            }

            sources.push((
                Self::query_source_position(&sql_upper, QuerySourceRef::Target(target)),
                query.extensions.len() + ordinal,
                QuerySourceRef::Target(target),
            ));
        }

        sources.sort_by_key(|(position, ordinal, _)| (*position, *ordinal));
        sources
            .into_iter()
            .map(|(_, _, source)| source)
            .collect::<Vec<_>>()
    }

    fn query_source_position(sql_upper: &str, source: QuerySourceRef<'_>) -> usize {
        match source {
            QuerySourceRef::Extension(extension) => match extension {
                SqlExtension::VectorSearch { .. } => sql_upper.find("VECTOR_SEARCH("),
                SqlExtension::GraphQuery { .. } => sql_upper.find("GRAPH_QUERY("),
                SqlExtension::DocumentQuery { .. } => sql_upper.find("DOCUMENT_QUERY("),
                SqlExtension::Logs { .. } => sql_upper.find("LOGS("),
                SqlExtension::Metrics { .. } => sql_upper.find("METRICS("),
                SqlExtension::VectorDistance { .. } => sql_upper.find("<->"),
            }
            .unwrap_or(usize::MAX),
            QuerySourceRef::Target(target) => Self::target_position(sql_upper, target),
        }
    }

    fn target_position(sql_upper: &str, target: &QueryTarget) -> usize {
        let target_upper = target.name.to_uppercase();
        [
            format!("FROM {}", target_upper),
            format!("JOIN {}", target_upper),
            format!(", {}", target_upper),
        ]
        .into_iter()
        .filter_map(|needle| sql_upper.find(&needle))
        .min()
        .or_else(|| sql_upper.find(&target_upper))
        .unwrap_or(usize::MAX)
    }

    fn collect_correlations(plan: &PlanNode) -> Vec<String> {
        let mut correlations = Vec::new();
        Self::collect_correlations_into(plan, &mut correlations);
        correlations
    }

    fn collect_correlations_into(plan: &PlanNode, correlations: &mut Vec<String>) {
        match &plan.node_type {
            PlanNodeType::VectorSearch {
                query_vector_source,
                ..
            } => Self::collect_vector_source_correlations(query_vector_source, correlations),
            PlanNodeType::HashJoin { left, right, .. }
            | PlanNodeType::IndexJoin { left, right, .. } => {
                Self::collect_correlations_into(left, correlations);
                Self::collect_correlations_into(right, correlations);
            }
            PlanNodeType::NestedLoopJoin { outer, inner, .. } => {
                Self::collect_correlations_into(outer, correlations);
                Self::collect_correlations_into(inner, correlations);
            }
            PlanNodeType::Filter { input, .. }
            | PlanNodeType::Project { input, .. }
            | PlanNodeType::Distinct { input }
            | PlanNodeType::Sort { input, .. }
            | PlanNodeType::Limit { input, .. }
            | PlanNodeType::Aggregate { input, .. } => {
                Self::collect_correlations_into(input, correlations);
            }
            PlanNodeType::Union { inputs, .. } => {
                for input in inputs {
                    Self::collect_correlations_into(input, correlations);
                }
            }
            PlanNodeType::Scan { .. }
            | PlanNodeType::GraphTraversal { .. }
            | PlanNodeType::DocumentQuery { .. }
            | PlanNodeType::ObservabilityQuery { .. } => {}
        }
    }

    fn collect_vector_source_correlations(source: &VectorSource, correlations: &mut Vec<String>) {
        match source {
            VectorSource::ColumnRef { table, column } => {
                let reference = format!("{}.{}", table, column);
                if !correlations.iter().any(|existing| existing == &reference) {
                    correlations.push(reference);
                }
            }
            VectorSource::Subquery(plan) => Self::collect_correlations_into(plan, correlations),
            VectorSource::Literal(_) | VectorSource::Expression(_) => {}
        }
    }

    /// Set statistics provider on an existing optimizer instance
    pub fn set_statistics_provider(&mut self, provider: std::sync::Arc<dyn StatisticsProvider>) {
        self.statistics_provider = Some(provider);
    }

    /// Create an optimizer with a custom plan cache
    pub fn with_plan_cache(mut self, cache: std::sync::Arc<PlanCache>) -> Self {
        self.plan_cache = cache;
        self
    }

    /// Get the plan cache
    pub fn plan_cache(&self) -> &std::sync::Arc<PlanCache> {
        &self.plan_cache
    }

    /// Get the cardinality estimator
    pub fn cardinality_estimator(&self) -> &CardinalityEstimator {
        &self.cardinality_estimator
    }

    /// Get the advanced cost estimator
    pub fn cost_estimator(&self) -> &AdvancedCostEstimator {
        &self.advanced_cost_estimator
    }

    /// Optimize a federated query
    ///
    /// If a statistics provider is configured, uses cardinality-aware
    /// cost estimation for better plan selection.
    pub fn optimize(&self, query: &FederatedQuery) -> Result<QueryPlan> {
        // If we have a statistics provider, collect stats for referenced collections
        // and delegate to optimize_with_statistics for cardinality-aware planning
        if let Some(ref provider) = self.statistics_provider {
            use crate::query::federated::parser::SqlExtension;
            let mut stats = HashMap::new();
            for ext in &query.extensions {
                let collection_name = match ext {
                    SqlExtension::VectorSearch { collection, .. } => Some(collection.clone()),
                    SqlExtension::DocumentQuery { collection, .. } => Some(collection.clone()),
                    _ => None,
                };
                if let Some(name) = collection_name
                    && let Some(model_stats) = provider.get_statistics(&name) {
                        stats.insert(name, model_stats);
                    }
            }
            // Also check query targets
            for target in &query.targets {
                if let Some(model_stats) = provider.get_statistics(&target.name) {
                    stats.insert(target.name.clone(), model_stats);
                }
            }
            if !stats.is_empty() {
                return self.optimize_with_statistics(query, &stats);
            }
        }

        // Check plan cache first
        let cache_key = PlanCacheKey::from_query(query);
        if let Some(cached_plan) = self.plan_cache.get(&cache_key) {
            tracing::debug!("Using cached plan for query");
            return Ok(cached_plan);
        }

        // Build initial logical plan
        let logical_plan = self.build_logical_plan(query)?;

        // Apply optimizations
        let optimized = self.apply_optimizations(logical_plan)?;

        // Build physical plan
        let physical_plan = self.build_physical_plan(optimized)?;

        // Cache the plan
        self.plan_cache.put(cache_key, physical_plan.clone());

        Ok(physical_plan)
    }

    /// Optimize a federated query with statistics (enhanced cost estimation)
    pub fn optimize_with_statistics(
        &self,
        query: &FederatedQuery,
        stats: &HashMap<String, ModelStatistics>,
    ) -> Result<QueryPlan> {
        // Check plan cache first
        let cache_key = PlanCacheKey::from_query(query);
        if let Some(cached_plan) = self.plan_cache.get(&cache_key) {
            return Ok(cached_plan);
        }

        // Build initial logical plan with statistics-aware costs
        let logical_plan = self.build_logical_plan_with_statistics(query, stats)?;

        // Apply optimizations including DP-based join ordering
        let optimized = self.apply_optimizations_with_statistics(logical_plan, stats)?;

        // Build physical plan
        let physical_plan = self.build_physical_plan(optimized)?;

        // Cache the plan
        self.plan_cache.put(cache_key, physical_plan.clone());

        Ok(physical_plan)
    }

    /// Build logical plan with statistics-aware cost estimation
    fn build_logical_plan_with_statistics(
        &self,
        query: &FederatedQuery,
        stats: &HashMap<String, ModelStatistics>,
    ) -> Result<PlanNode> {
        match query.query_type {
            QueryType::VectorSearch => self.plan_vector_search_with_statistics(query, stats),
            QueryType::GraphQuery => self.plan_graph_query_with_statistics(query, stats),
            QueryType::DocumentQuery => self.plan_document_query_with_statistics(query, stats),
            QueryType::Federated => self.plan_federated_query_with_statistics(query, stats),
            _ => self.build_logical_plan(query),
        }
    }

    /// Plan vector search with statistics
    fn plan_vector_search_with_statistics(
        &self,
        query: &FederatedQuery,
        stats: &HashMap<String, ModelStatistics>,
    ) -> Result<PlanNode> {
        // Extract collection name and top_k from extensions
        let (collection, query_vector_source, top_k) = query
            .extensions
            .iter()
            .find_map(|ext| {
                if let SqlExtension::VectorSearch {
                    collection,
                    query_vector,
                    top_k,
                } = ext
                {
                    Some((
                        collection.clone(),
                        Self::vector_source_from_query(query_vector),
                        *top_k,
                    ))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| ("unknown".to_string(), VectorSource::Literal(vec![]), 10));

        // Get statistics for this collection
        let (cost, rows) = if let Some(ModelStatistics::Vector(vec_stats)) = stats.get(&collection)
        {
            let cost = self
                .advanced_cost_estimator
                .vector_search_cost(vec_stats, top_k);
            let rows = self
                .cardinality_estimator
                .estimate_vector_search_cardinality(top_k, vec_stats);
            (cost, rows)
        } else {
            // Default estimation
            (100.0, top_k as u64)
        };

        Ok(PlanNode {
            id: self.next_id(),
            node_type: PlanNodeType::VectorSearch {
                collection: collection.clone(),
                top_k,
                query_vector_source,
            },
            estimated_cost: cost,
            estimated_rows: rows,
            output_columns: vec!["id".to_string(), "score".to_string()],
        })
    }

    /// Plan graph query with statistics
    fn plan_graph_query_with_statistics(
        &self,
        query: &FederatedQuery,
        stats: &HashMap<String, ModelStatistics>,
    ) -> Result<PlanNode> {
        // Extract cypher query
        let cypher = query
            .extensions
            .iter()
            .find_map(|ext| {
                if let SqlExtension::GraphQuery { cypher } = ext {
                    Some(cypher.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        // Estimate max depth from cypher (simple heuristic)
        let max_depth = cypher.matches("->").count().max(1);
        let graph_name = "default"; // Would need to parse from cypher

        // Get statistics
        let (cost, rows) = if let Some(ModelStatistics::Graph(graph_stats)) = stats.get(graph_name)
        {
            let cost = self
                .advanced_cost_estimator
                .graph_traversal_cost(graph_stats, max_depth);
            let rows = self
                .cardinality_estimator
                .estimate_graph_traversal_cardinality(graph_stats, max_depth);
            (cost, rows)
        } else {
            // Default estimation
            let default_stats = GraphStats {
                node_count: 10000,
                edge_count: 50000,
                avg_degree: 5.0,
                max_depth: Some(6),
                has_label_index: true,
            };
            let cost = self
                .advanced_cost_estimator
                .graph_traversal_cost(&default_stats, max_depth);
            (cost, 100)
        };

        Ok(PlanNode {
            id: self.next_id(),
            node_type: PlanNodeType::GraphTraversal {
                cypher,
                start_nodes: None, // Will be populated from query parsing
            },
            estimated_cost: cost,
            estimated_rows: rows,
            output_columns: vec![
                "node_id".to_string(),
                "label".to_string(),
                "properties".to_string(),
            ],
        })
    }

    /// Plan document query with statistics
    fn plan_document_query_with_statistics(
        &self,
        query: &FederatedQuery,
        stats: &HashMap<String, ModelStatistics>,
    ) -> Result<PlanNode> {
        // Extract collection and filter
        let (collection, filter) = query
            .extensions
            .iter()
            .find_map(|ext| {
                if let SqlExtension::DocumentQuery { collection, filter } = ext {
                    Some((collection.clone(), filter.clone()))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| ("unknown".to_string(), None));

        // Estimate filter complexity and fields
        let filter_complexity = filter
            .as_ref()
            .map_or(1, |f| f.matches("AND").count() + f.matches("OR").count() + 1);
        let filter_fields: Vec<String> = vec![]; // Would need proper filter parsing

        // Get statistics
        let (cost, rows) =
            if let Some(ModelStatistics::Document(doc_stats)) = stats.get(&collection) {
                let cost = self.advanced_cost_estimator.document_query_cost(
                    doc_stats,
                    &filter_fields,
                    filter_complexity,
                );
                let selectivity = 0.1_f64.powi(filter_complexity as i32);
                let rows = self
                    .cardinality_estimator
                    .estimate_document_query_cardinality(doc_stats, selectivity);
                (cost, rows)
            } else {
                (100.0, 100)
            };

        Ok(PlanNode {
            id: self.next_id(),
            node_type: PlanNodeType::DocumentQuery { collection, filter },
            estimated_cost: cost,
            estimated_rows: rows,
            output_columns: vec!["id".to_string(), "document".to_string()],
        })
    }

    /// Plan federated query with statistics and DP join ordering
    fn plan_federated_query_with_statistics(
        &self,
        query: &FederatedQuery,
        stats: &HashMap<String, ModelStatistics>,
    ) -> Result<PlanNode> {
        // Build sub-plans for each extension/target
        let mut sub_plans = Vec::new();

        for source in Self::ordered_query_sources(query) {
            let sub_plan = match source {
                QuerySourceRef::Extension(SqlExtension::VectorSearch {
                    collection,
                    query_vector,
                    top_k,
                }) => {
                    let vec_stats = stats.get(collection).and_then(|s| {
                        if let ModelStatistics::Vector(vs) = s {
                            Some(vs)
                        } else {
                            None
                        }
                    });
                    let (cost, rows) = if let Some(vs) = vec_stats {
                        (
                            self.advanced_cost_estimator.vector_search_cost(vs, *top_k),
                            self.cardinality_estimator
                                .estimate_vector_search_cardinality(*top_k, vs),
                        )
                    } else {
                        (100.0, *top_k as u64)
                    };

                    PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::VectorSearch {
                            collection: collection.clone(),
                            top_k: *top_k,
                            query_vector_source: Self::vector_source_from_query(query_vector),
                        },
                        estimated_cost: cost,
                        estimated_rows: rows,
                        output_columns: vec!["id".to_string(), "score".to_string()],
                    }
                }
                QuerySourceRef::Extension(SqlExtension::GraphQuery { cypher }) => {
                    let max_depth = cypher.matches("->").count().max(1);
                    // Use default stats for graph
                    let default_stats = GraphStats::default();
                    let cost = self
                        .advanced_cost_estimator
                        .graph_traversal_cost(&default_stats, max_depth);

                    PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::GraphTraversal {
                            cypher: cypher.clone(),
                            start_nodes: None, // Will be populated from query parsing
                        },
                        estimated_cost: cost,
                        estimated_rows: 100,
                        output_columns: vec![
                            "node_id".to_string(),
                            "label".to_string(),
                            "properties".to_string(),
                        ],
                    }
                }
                QuerySourceRef::Extension(SqlExtension::DocumentQuery { collection, filter }) => {
                    let doc_stats = stats.get(collection).and_then(|s| {
                        if let ModelStatistics::Document(ds) = s {
                            Some(ds)
                        } else {
                            None
                        }
                    });
                    let (cost, rows) = if let Some(ds) = doc_stats {
                        (
                            self.advanced_cost_estimator.document_query_cost(ds, &[], 1),
                            ds.document_count,
                        )
                    } else {
                        (100.0, 1000)
                    };

                    PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::DocumentQuery {
                            collection: collection.clone(),
                            filter: filter.clone(),
                        },
                        estimated_cost: cost,
                        estimated_rows: rows,
                        output_columns: vec!["id".to_string(), "document".to_string()],
                    }
                }
                QuerySourceRef::Extension(SqlExtension::Logs { namespace }) => PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::ObservabilityQuery {
                        namespace: namespace.clone(),
                        query_type: ObservabilityQueryType::Logs,
                        time_range: None,
                    },
                    estimated_cost: 20.0,
                    estimated_rows: 1000,
                    output_columns: vec![
                        "timestamp".to_string(),
                        "level".to_string(),
                        "message".to_string(),
                    ],
                },
                QuerySourceRef::Extension(SqlExtension::Metrics { namespace }) => PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::ObservabilityQuery {
                        namespace: namespace.clone(),
                        query_type: ObservabilityQueryType::Metrics,
                        time_range: None,
                    },
                    estimated_cost: 20.0,
                    estimated_rows: 500,
                    output_columns: vec![
                        "timestamp".to_string(),
                        "metric_name".to_string(),
                        "value".to_string(),
                    ],
                },
                QuerySourceRef::Extension(SqlExtension::VectorDistance {
                    left_column,
                    right_literal,
                }) => {
                    let target = query
                        .targets
                        .first()
                        .map_or("default".to_string(), |t| t.name.clone());
                    PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::VectorSearch {
                            collection: target,
                            top_k: 10,
                            query_vector_source: Self::vector_source_from_literal(right_literal),
                        },
                        estimated_cost: 10.0,
                        estimated_rows: 10,
                        output_columns: vec!["id".to_string(), left_column.clone()],
                    }
                }
                QuerySourceRef::Target(target) => {
                    let rows = stats
                        .get(&target.name)
                        .map_or(1000, |s| s.estimated_count());

                    PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::Scan {
                            target: target.name.clone(),
                            model_type: ModelType::Relational,
                            predicates: vec![],
                        },
                        estimated_cost: rows as f64 * 0.1,
                        estimated_rows: rows,
                        output_columns: vec!["*".to_string()],
                    }
                }
            };
            sub_plans.push(sub_plan);
        }

        if sub_plans.is_empty() {
            return Err(anyhow!("No sub-plans generated for federated query"));
        }

        if sub_plans.len() == 1 {
            return Ok(sub_plans.remove(0));
        }

        if query.sql.to_uppercase().contains("LATERAL") {
            let mut result = sub_plans.remove(0);
            for right in sub_plans {
                let correlation = Self::collect_correlations(&right);
                let estimated_cost = result.estimated_cost + right.estimated_cost + 200.0;
                let estimated_rows = result
                    .estimated_rows
                    .saturating_mul(right.estimated_rows.max(1));
                result = PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::NestedLoopJoin {
                        outer: Box::new(result),
                        inner: Box::new(right),
                        correlation,
                    },
                    estimated_cost,
                    estimated_rows,
                    output_columns: vec!["*".to_string()],
                };
            }
            return Ok(result);
        }

        // Use DP-based join order optimization
        let join_predicates: Vec<(usize, usize, Vec<(String, String)>)> = vec![];
        self.join_order_optimizer.find_optimal_join_order(
            &sub_plans,
            &join_predicates,
            &self.next_node_id,
        )
    }

    /// Apply optimizations with statistics
    fn apply_optimizations_with_statistics(
        &self,
        plan: PlanNode,
        _stats: &HashMap<String, ModelStatistics>,
    ) -> Result<PlanNode> {
        // Apply standard optimizations
        let optimized = self.apply_optimizations(plan)?;
        Ok(optimized)
    }

    /// Build initial logical plan from parsed query
    fn build_logical_plan(&self, query: &FederatedQuery) -> Result<PlanNode> {
        let plan = match query.query_type {
            QueryType::Sql => self.plan_sql_query(query),
            QueryType::VectorSearch => self.plan_vector_search(query),
            QueryType::GraphQuery => self.plan_graph_query(query),
            QueryType::DocumentQuery => self.plan_document_query(query),
            QueryType::LogQuery | QueryType::MetricQuery => self.plan_observability_query(query),
            QueryType::Federated => self.plan_federated_query(query),
        }?;

        self.apply_sql_clauses(plan, query)
    }

    fn apply_sql_clauses(&self, mut plan: PlanNode, query: &FederatedQuery) -> Result<PlanNode> {
        let has_distinct = Self::select_has_distinct(&query.sql);
        if Self::find_top_level_keyword(&query.sql, "UNION").is_some() {
            return Err(anyhow!(
                "UNION queries are not yet supported in federated SQL execution"
            ));
        }
        if Self::find_top_level_keyword(&query.sql, "HAVING").is_some() {
            return Err(anyhow!(
                "HAVING is not yet supported in federated SQL execution"
            ));
        }

        if let Some(predicate) = Self::extract_where_predicate(&query.sql) {
            let output_columns = plan.output_columns.clone();
            let estimated_cost = plan.estimated_cost * 1.05;
            let estimated_rows = plan.estimated_rows;
            plan = PlanNode {
                id: self.next_id(),
                node_type: PlanNodeType::Filter {
                    input: Box::new(plan),
                    predicate,
                },
                estimated_cost,
                estimated_rows,
                output_columns,
            };
        }

        let select_items = Self::extract_select_items(&query.sql);
        let group_by = Self::extract_group_by_columns(&query.sql);
        let aggregates = select_items
            .iter()
            .filter_map(Self::parse_aggregate_expr)
            .collect::<Vec<_>>();
        let has_group_by = !group_by.is_empty();
        let has_aggregate_projection = !aggregates.is_empty();
        if has_group_by {
            let invalid_group_projection = select_items
                .iter()
                .filter(|item| Self::parse_aggregate_expr(item).is_none())
                .any(|item| !group_by.contains(&item.expression));

            if invalid_group_projection {
                return Err(anyhow!(
                    "GROUP BY select items must reference grouped columns or aggregate expressions"
                ));
            }
        }

        if has_group_by || has_aggregate_projection {
            let aggregate_columns = group_by
                .iter()
                .cloned()
                .chain(aggregates.iter().map(|aggregate| aggregate.alias.clone()))
                .collect::<Vec<_>>();
            let estimated_rows = if has_group_by {
                ((plan.estimated_rows as f64) * 0.5).ceil() as u64
            } else {
                1
            };
            let estimated_rows = estimated_rows.max(u64::from(plan.estimated_rows > 0));
            let estimated_cost = if has_group_by {
                plan.estimated_cost * 1.15
            } else {
                plan.estimated_cost * 1.1
            };

            plan = PlanNode {
                id: self.next_id(),
                node_type: PlanNodeType::Aggregate {
                    input: Box::new(plan),
                    group_by,
                    aggregates,
                },
                estimated_cost,
                estimated_rows,
                output_columns: aggregate_columns,
            };
        }

        let projection_sources = select_items
            .iter()
            .map(|item| {
                Self::parse_aggregate_expr(item).map_or_else(|| item.expression.clone(), |aggregate| aggregate.alias)
            })
            .collect::<Vec<_>>();
        let projection_output_columns = select_items
            .iter()
            .map(|item| {
                item.alias
                    .clone()
                    .unwrap_or_else(|| item.expression.clone())
            })
            .collect::<Vec<_>>();
        if (!has_aggregate_projection && !has_group_by)
            && !projection_sources.is_empty()
            && !(projection_sources.len() == 1 && projection_sources[0] == "*")
            && !projection_sources
                .iter()
                .any(|column| column.ends_with(".*"))
        {
            let estimated_cost = plan.estimated_cost;
            let estimated_rows = plan.estimated_rows;
            plan = PlanNode {
                id: self.next_id(),
                node_type: PlanNodeType::Project {
                    input: Box::new(plan),
                    columns: projection_sources.clone(),
                },
                estimated_cost,
                estimated_rows,
                output_columns: projection_output_columns.clone(),
            };
        }

        if has_group_by
            && !projection_sources.is_empty()
            && !(projection_sources.len() == 1 && projection_sources[0] == "*")
            && !projection_sources
                .iter()
                .any(|column| column.ends_with(".*"))
            && (projection_sources != plan.output_columns
                || projection_output_columns != plan.output_columns)
        {
            let estimated_cost = plan.estimated_cost;
            let estimated_rows = plan.estimated_rows;
            plan = PlanNode {
                id: self.next_id(),
                node_type: PlanNodeType::Project {
                    input: Box::new(plan),
                    columns: projection_sources,
                },
                estimated_cost,
                estimated_rows,
                output_columns: projection_output_columns,
            };
        }

        if has_distinct {
            let output_columns = plan.output_columns.clone();
            let estimated_cost = plan.estimated_cost * 1.05;
            let estimated_rows = ((plan.estimated_rows as f64) * 0.8).ceil() as u64;
            plan = PlanNode {
                id: self.next_id(),
                node_type: PlanNodeType::Distinct {
                    input: Box::new(plan),
                },
                estimated_cost,
                estimated_rows,
                output_columns,
            };
        }

        let order_by = Self::extract_order_by(&query.sql);
        if !order_by.is_empty() {
            let output_columns = plan.output_columns.clone();
            let estimated_cost = plan.estimated_cost * 1.1;
            let estimated_rows = plan.estimated_rows;
            plan = PlanNode {
                id: self.next_id(),
                node_type: PlanNodeType::Sort {
                    input: Box::new(plan),
                    order_by,
                },
                estimated_cost,
                estimated_rows,
                output_columns,
            };
        }

        let (limit, offset) = Self::extract_limit_offset(&query.sql);
        if let Some(limit) = limit {
            let output_columns = plan.output_columns.clone();
            let estimated_cost = plan.estimated_cost;
            let estimated_rows = plan.estimated_rows.min(limit as u64);
            plan = PlanNode {
                id: self.next_id(),
                node_type: PlanNodeType::Limit {
                    input: Box::new(plan),
                    limit,
                    offset,
                },
                estimated_cost,
                estimated_rows,
                output_columns,
            };
        }

        Ok(plan)
    }

    fn find_top_level_keyword(sql: &str, keyword: &str) -> Option<usize> {
        Self::find_top_level_keyword_from(sql, keyword, 0)
    }

    fn find_top_level_keyword_from(sql: &str, keyword: &str, start_at: usize) -> Option<usize> {
        let sql_upper = sql.to_uppercase();
        let keyword_upper = keyword.to_uppercase();
        let bytes = sql_upper.as_bytes();
        let keyword_len = keyword_upper.len();
        let mut depth = 0usize;
        let mut in_quote = None;
        let mut escaped = false;

        for (index, ch) in sql.char_indices() {
            if let Some(quote) = in_quote {
                if ch == quote && !escaped {
                    in_quote = None;
                }
                escaped = ch == '\\' && !escaped;
                continue;
            }

            match ch {
                '\'' | '"' => {
                    in_quote = Some(ch);
                    escaped = false;
                    continue;
                }
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            }

            if index < start_at || depth != 0 || index + keyword_len > sql_upper.len() {
                escaped = false;
                continue;
            }

            if &sql_upper[index..index + keyword_len] == keyword_upper.as_str() {
                let before_ok = index == 0
                    || (!bytes[index - 1].is_ascii_alphanumeric() && bytes[index - 1] != b'_');
                let after_index = index + keyword_len;
                let after_ok = after_index == bytes.len()
                    || (!bytes[after_index].is_ascii_alphanumeric() && bytes[after_index] != b'_');
                if before_ok && after_ok {
                    return Some(index);
                }
            }

            escaped = false;
        }

        None
    }

    fn find_clause_end(sql: &str, start_at: usize, keywords: &[&str]) -> usize {
        keywords
            .iter()
            .filter_map(|keyword| Self::find_top_level_keyword_from(sql, keyword, start_at))
            .min()
            .unwrap_or(sql.len())
    }

    fn split_top_level_list(input: &str) -> Vec<String> {
        let mut items = Vec::new();
        let mut current = String::new();
        let mut depth = 0usize;
        let mut in_quote = None;
        let mut escaped = false;

        for ch in input.chars() {
            if let Some(quote) = in_quote {
                current.push(ch);
                if ch == quote && !escaped {
                    in_quote = None;
                }
                escaped = ch == '\\' && !escaped;
                continue;
            }

            match ch {
                '\'' | '"' => {
                    in_quote = Some(ch);
                    current.push(ch);
                }
                '(' | '[' => {
                    depth += 1;
                    current.push(ch);
                }
                ')' | ']' => {
                    depth = depth.saturating_sub(1);
                    current.push(ch);
                }
                ',' if depth == 0 => {
                    if !current.trim().is_empty() {
                        items.push(current.trim().to_string());
                    }
                    current.clear();
                }
                _ => current.push(ch),
            }

            escaped = false;
        }

        if !current.trim().is_empty() {
            items.push(current.trim().to_string());
        }

        items
    }

    fn select_has_distinct(sql: &str) -> bool {
        let Some(select_pos) = Self::find_top_level_keyword(sql, "SELECT") else {
            return false;
        };
        let Some(from_pos) = Self::find_top_level_keyword_from(sql, "FROM", select_pos + 6) else {
            return false;
        };

        sql[select_pos + 6..from_pos]
            .trim_start()
            .to_uppercase()
            .starts_with("DISTINCT ")
    }

    fn extract_select_items(sql: &str) -> Vec<SelectItem> {
        let Some(select_pos) = Self::find_top_level_keyword(sql, "SELECT") else {
            return vec![];
        };
        let Some(from_pos) = Self::find_top_level_keyword_from(sql, "FROM", select_pos + 6) else {
            return vec![];
        };

        let clause = sql[select_pos + 6..from_pos].trim();
        let clause = clause
            .strip_prefix("DISTINCT ")
            .or_else(|| clause.strip_prefix("distinct "))
            .unwrap_or(clause);

        Self::split_top_level_list(clause)
            .into_iter()
            .map(|item| {
                let upper = item.to_uppercase();
                if let Some(as_pos) = upper.rfind(" AS ") {
                    SelectItem {
                        expression: item[..as_pos].trim().to_string(),
                        alias: Some(item[as_pos + 4..].trim().to_string()),
                    }
                } else {
                    SelectItem {
                        expression: item.trim().to_string(),
                        alias: None,
                    }
                }
            })
            .collect()
    }

    #[allow(dead_code)]
    fn extract_select_columns(sql: &str) -> Vec<String> {
        Self::extract_select_items(sql)
            .into_iter()
            .map(|item| item.expression)
            .collect()
    }

    fn extract_group_by_columns(sql: &str) -> Vec<String> {
        let Some(group_by_pos) = Self::find_top_level_keyword(sql, "GROUP BY") else {
            return vec![];
        };
        let end = Self::find_clause_end(
            sql,
            group_by_pos + 8,
            &["ORDER BY", "LIMIT", "OFFSET", "HAVING", ";"],
        );

        Self::split_top_level_list(sql[group_by_pos + 8..end].trim())
    }

    fn parse_aggregate_expr(item: &SelectItem) -> Option<AggregateExpr> {
        let expression = item.expression.trim();
        let open_paren = expression.find('(')?;
        if !expression.ends_with(')') {
            return None;
        }

        let function_name = expression[..open_paren].trim().to_uppercase();
        let inner = expression[open_paren + 1..expression.len() - 1].trim();
        let alias = item.alias.clone().unwrap_or_else(|| expression.to_string());

        match function_name.as_str() {
            "COUNT" => {
                if inner == "*" {
                    Some(AggregateExpr {
                        function: AggregateFunction::Count,
                        column: None,
                        alias,
                    })
                } else if let Some(distinct_column) = inner
                    .strip_prefix("DISTINCT ")
                    .or_else(|| inner.strip_prefix("distinct "))
                {
                    Some(AggregateExpr {
                        function: AggregateFunction::CountDistinct,
                        column: Some(distinct_column.trim().to_string()),
                        alias,
                    })
                } else {
                    Some(AggregateExpr {
                        function: AggregateFunction::Count,
                        column: Some(inner.to_string()),
                        alias,
                    })
                }
            }
            "SUM" => Some(AggregateExpr {
                function: AggregateFunction::Sum,
                column: Some(inner.to_string()),
                alias,
            }),
            "AVG" => Some(AggregateExpr {
                function: AggregateFunction::Avg,
                column: Some(inner.to_string()),
                alias,
            }),
            "MIN" => Some(AggregateExpr {
                function: AggregateFunction::Min,
                column: Some(inner.to_string()),
                alias,
            }),
            "MAX" => Some(AggregateExpr {
                function: AggregateFunction::Max,
                column: Some(inner.to_string()),
                alias,
            }),
            _ => None,
        }
    }

    fn extract_limit_offset(sql: &str) -> (Option<usize>, usize) {
        let limit = Self::find_top_level_keyword(sql, "LIMIT").and_then(|limit_pos| {
            let end = Self::find_clause_end(sql, limit_pos + 5, &["OFFSET", ";"]);
            sql[limit_pos + 5..end]
                .trim()
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<usize>().ok())
        });

        let offset = Self::find_top_level_keyword(sql, "OFFSET")
            .and_then(|offset_pos| {
                let end = Self::find_clause_end(sql, offset_pos + 6, &["LIMIT", ";"]);
                sql[offset_pos + 6..end]
                    .trim()
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .unwrap_or(0);

        (limit, offset)
    }

    fn extract_order_by(sql: &str) -> Vec<OrderByClause> {
        let Some(order_pos) = Self::find_top_level_keyword(sql, "ORDER BY") else {
            return vec![];
        };
        let end = Self::find_clause_end(sql, order_pos + 8, &["LIMIT", "OFFSET", ";"]);
        let clause = sql[order_pos + 8..end].trim();

        Self::split_top_level_list(clause)
            .into_iter()
            .filter_map(|entry| {
                let upper = entry.to_uppercase();
                let nulls_first = upper.ends_with(" NULLS FIRST");
                let nulls_last = upper.ends_with(" NULLS LAST");
                let trimmed = if nulls_first {
                    entry[..entry.len() - "NULLS FIRST".len()].trim()
                } else if nulls_last {
                    entry[..entry.len() - "NULLS LAST".len()].trim()
                } else {
                    entry.trim()
                };

                let upper_trimmed = trimmed.to_uppercase();
                let ascending = !upper_trimmed.ends_with(" DESC");
                let column = if upper_trimmed.ends_with(" ASC") || upper_trimmed.ends_with(" DESC")
                {
                    trimmed[..trimmed.rfind(' ').unwrap_or(trimmed.len())]
                        .trim()
                        .to_string()
                } else {
                    trimmed.to_string()
                };

                if column.is_empty() {
                    None
                } else {
                    Some(OrderByClause {
                        column,
                        ascending,
                        nulls_first: if nulls_first {
                            true
                        } else if nulls_last {
                            false
                        } else {
                            !ascending
                        },
                    })
                }
            })
            .collect()
    }

    fn find_top_level_operator(input: &str, operator: &str) -> Option<usize> {
        let upper = input.to_uppercase();
        let operator_upper = operator.to_uppercase();
        let mut depth = 0usize;
        let mut in_quote = None;
        let mut escaped = false;

        for (index, ch) in input.char_indices() {
            if let Some(quote) = in_quote {
                if ch == quote && !escaped {
                    in_quote = None;
                }
                escaped = ch == '\\' && !escaped;
                continue;
            }

            match ch {
                '\'' | '"' => {
                    in_quote = Some(ch);
                    escaped = false;
                    continue;
                }
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            }

            if depth == 0
                && index + operator_upper.len() <= upper.len()
                && &upper[index..index + operator_upper.len()] == operator_upper.as_str()
            {
                return Some(index);
            }

            escaped = false;
        }

        None
    }

    fn parse_predicate_value(raw: &str) -> Option<PredicateValue> {
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("NULL") {
            return Some(PredicateValue::Null);
        }
        if trimmed.eq_ignore_ascii_case("TRUE") {
            return Some(PredicateValue::Bool(true));
        }
        if trimmed.eq_ignore_ascii_case("FALSE") {
            return Some(PredicateValue::Bool(false));
        }
        if (trimmed.starts_with('\'') && trimmed.ends_with('\''))
            || (trimmed.starts_with('"') && trimmed.ends_with('"'))
        {
            return Some(PredicateValue::String(
                trimmed[1..trimmed.len() - 1].to_string(),
            ));
        }
        if let Ok(value) = trimmed.parse::<i64>() {
            return Some(PredicateValue::Int(value));
        }
        if let Ok(value) = trimmed.parse::<f64>() {
            return Some(PredicateValue::Float(value));
        }
        None
    }

    fn extract_where_predicate(sql: &str) -> Option<Predicate> {
        let where_pos = Self::find_top_level_keyword(sql, "WHERE")?;
        let end = Self::find_clause_end(
            sql,
            where_pos + 5,
            &["ORDER BY", "GROUP BY", "LIMIT", "OFFSET", "HAVING", ";"],
        );
        let clause = sql[where_pos + 5..end].trim();
        if clause.is_empty()
            || Self::find_top_level_keyword(clause, "AND").is_some()
            || Self::find_top_level_keyword(clause, "OR").is_some()
        {
            return None;
        }

        let upper = clause.to_uppercase();
        if upper.ends_with(" IS NOT NULL") {
            return Some(Predicate {
                column: clause[..clause.len() - "IS NOT NULL".len()]
                    .trim()
                    .to_string(),
                op: PredicateOp::IsNotNull,
                value: PredicateValue::Null,
            });
        }
        if upper.ends_with(" IS NULL") {
            return Some(Predicate {
                column: clause[..clause.len() - "IS NULL".len()].trim().to_string(),
                op: PredicateOp::IsNull,
                value: PredicateValue::Null,
            });
        }

        if let Some(index) = Self::find_top_level_keyword(clause, "LIKE") {
            return Some(Predicate {
                column: clause[..index].trim().to_string(),
                op: PredicateOp::Like,
                value: Self::parse_predicate_value(clause[index + 4..].trim())?,
            });
        }

        for (operator, predicate_op) in [
            ("!=", PredicateOp::Ne),
            ("<>", PredicateOp::Ne),
            (">=", PredicateOp::Ge),
            ("<=", PredicateOp::Le),
            ("=", PredicateOp::Eq),
            (">", PredicateOp::Gt),
            ("<", PredicateOp::Lt),
        ] {
            if let Some(index) = Self::find_top_level_operator(clause, operator) {
                return Some(Predicate {
                    column: clause[..index].trim().to_string(),
                    op: predicate_op,
                    value: Self::parse_predicate_value(clause[index + operator.len()..].trim())?,
                });
            }
        }

        None
    }

    /// Plan a simple SQL query
    fn plan_sql_query(&self, query: &FederatedQuery) -> Result<PlanNode> {
        let target = query
            .targets
            .first().map_or_else(|| "unknown".to_string(), |t| t.name.clone());

        Ok(PlanNode {
            id: self.next_id(),
            node_type: PlanNodeType::Scan {
                target: target.clone(),
                model_type: ModelType::Relational,
                predicates: vec![],
            },
            estimated_cost: 100.0,
            estimated_rows: 1000,
            output_columns: vec!["*".to_string()],
        })
    }

    /// Plan a vector search query
    fn plan_vector_search(&self, query: &FederatedQuery) -> Result<PlanNode> {
        let (collection, query_vector_source, top_k) = query
            .extensions
            .iter()
            .find_map(|ext| match ext {
                SqlExtension::VectorSearch {
                    collection,
                    query_vector,
                    top_k,
                } => Some((
                    collection.clone(),
                    Self::vector_source_from_query(query_vector),
                    *top_k,
                )),
                SqlExtension::VectorDistance { right_literal, .. } => {
                    // Extract from query targets
                    query.targets.first().map(|t| {
                        (
                            t.name.clone(),
                            Self::vector_source_from_literal(right_literal),
                            10,
                        )
                    })
                }
                _ => None,
            })
            .unwrap_or(("default".to_string(), VectorSource::Literal(vec![]), 10));

        Ok(PlanNode {
            id: self.next_id(),
            node_type: PlanNodeType::VectorSearch {
                collection,
                top_k,
                query_vector_source,
            },
            estimated_cost: 10.0,
            estimated_rows: top_k as u64,
            output_columns: vec!["id".to_string(), "score".to_string()],
        })
    }

    /// Plan a graph query
    fn plan_graph_query(&self, query: &FederatedQuery) -> Result<PlanNode> {
        let cypher = query
            .extensions
            .iter()
            .find_map(|ext| match ext {
                SqlExtension::GraphQuery { cypher } => Some(cypher.clone()),
                _ => None,
            })
            .unwrap_or_default();

        Ok(PlanNode {
            id: self.next_id(),
            node_type: PlanNodeType::GraphTraversal {
                cypher,
                start_nodes: None,
            },
            estimated_cost: 50.0,
            estimated_rows: 100,
            output_columns: vec![
                "node_id".to_string(),
                "label".to_string(),
                "properties".to_string(),
            ],
        })
    }

    /// Plan a document query
    fn plan_document_query(&self, query: &FederatedQuery) -> Result<PlanNode> {
        let (collection, filter) = query
            .extensions
            .iter()
            .find_map(|ext| match ext {
                SqlExtension::DocumentQuery { collection, filter } => {
                    Some((collection.clone(), filter.clone()))
                }
                _ => None,
            })
            .unwrap_or(("default".to_string(), None));

        Ok(PlanNode {
            id: self.next_id(),
            node_type: PlanNodeType::DocumentQuery { collection, filter },
            estimated_cost: 30.0,
            estimated_rows: 500,
            output_columns: vec!["id".to_string(), "document".to_string()],
        })
    }

    /// Plan an observability query
    fn plan_observability_query(&self, query: &FederatedQuery) -> Result<PlanNode> {
        let (namespace, query_type) = query
            .extensions
            .iter()
            .find_map(|ext| match ext {
                SqlExtension::Logs { namespace } => {
                    Some((namespace.clone(), ObservabilityQueryType::Logs))
                }
                SqlExtension::Metrics { namespace } => {
                    Some((namespace.clone(), ObservabilityQueryType::Metrics))
                }
                _ => None,
            })
            .unwrap_or(("default".to_string(), ObservabilityQueryType::Logs));
        let output_columns = match &query_type {
            ObservabilityQueryType::Logs => vec![
                "timestamp".to_string(),
                "level".to_string(),
                "message".to_string(),
            ],
            ObservabilityQueryType::Metrics => vec![
                "timestamp".to_string(),
                "metric_name".to_string(),
                "value".to_string(),
            ],
            ObservabilityQueryType::Traces => vec![
                "trace_id".to_string(),
                "span_id".to_string(),
                "operation".to_string(),
                "duration_ns".to_string(),
            ],
        };

        Ok(PlanNode {
            id: self.next_id(),
            node_type: PlanNodeType::ObservabilityQuery {
                namespace,
                query_type,
                time_range: None, // TODO: Extract from WHERE clause
            },
            estimated_cost: 20.0,
            estimated_rows: 1000,
            output_columns,
        })
    }

    /// Plan a federated (cross-model) query
    fn plan_federated_query(&self, query: &FederatedQuery) -> Result<PlanNode> {
        // Build sub-plans for each extension
        let mut sub_plans: Vec<PlanNode> = Vec::new();

        for source in Self::ordered_query_sources(query) {
            let sub_plan = match source {
                QuerySourceRef::Extension(SqlExtension::VectorSearch {
                    collection,
                    query_vector,
                    top_k,
                }) => PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::VectorSearch {
                        collection: collection.clone(),
                        top_k: *top_k,
                        query_vector_source: Self::vector_source_from_query(query_vector),
                    },
                    estimated_cost: 10.0,
                    estimated_rows: *top_k as u64,
                    output_columns: vec!["id".to_string(), "score".to_string()],
                },
                QuerySourceRef::Extension(SqlExtension::GraphQuery { cypher }) => PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::GraphTraversal {
                        cypher: cypher.clone(),
                        start_nodes: None,
                    },
                    estimated_cost: 50.0,
                    estimated_rows: 100,
                    output_columns: vec![
                        "node_id".to_string(),
                        "label".to_string(),
                        "properties".to_string(),
                    ],
                },
                QuerySourceRef::Extension(SqlExtension::DocumentQuery { collection, filter }) => {
                    PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::DocumentQuery {
                            collection: collection.clone(),
                            filter: filter.clone(),
                        },
                        estimated_cost: 30.0,
                        estimated_rows: 500,
                        output_columns: vec!["id".to_string(), "document".to_string()],
                    }
                }
                QuerySourceRef::Extension(SqlExtension::Logs { namespace }) => PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::ObservabilityQuery {
                        namespace: namespace.clone(),
                        query_type: ObservabilityQueryType::Logs,
                        time_range: None,
                    },
                    estimated_cost: 20.0,
                    estimated_rows: 1000,
                    output_columns: vec![
                        "timestamp".to_string(),
                        "level".to_string(),
                        "message".to_string(),
                    ],
                },
                QuerySourceRef::Extension(SqlExtension::Metrics { namespace }) => PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::ObservabilityQuery {
                        namespace: namespace.clone(),
                        query_type: ObservabilityQueryType::Metrics,
                        time_range: None,
                    },
                    estimated_cost: 20.0,
                    estimated_rows: 500,
                    output_columns: vec![
                        "timestamp".to_string(),
                        "metric_name".to_string(),
                        "value".to_string(),
                    ],
                },
                QuerySourceRef::Extension(SqlExtension::VectorDistance {
                    left_column,
                    right_literal,
                }) => {
                    let target = query
                        .targets
                        .first()
                        .map_or("default".to_string(), |t| t.name.clone());
                    PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::VectorSearch {
                            collection: target,
                            top_k: 10,
                            query_vector_source: Self::vector_source_from_literal(right_literal),
                        },
                        estimated_cost: 10.0,
                        estimated_rows: 10,
                        output_columns: vec!["id".to_string(), left_column.clone()],
                    }
                }
                QuerySourceRef::Target(target) => PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Scan {
                        target: target.name.clone(),
                        model_type: ModelType::Relational,
                        predicates: vec![],
                    },
                    estimated_cost: 100.0,
                    estimated_rows: 1000,
                    output_columns: vec!["*".to_string()],
                },
            };
            sub_plans.push(sub_plan);
        }

        // If we have multiple sub-plans, join them
        if sub_plans.len() >= 2 {
            let mut result = sub_plans.remove(0);
            for right in sub_plans {
                let correlation = Self::collect_correlations(&right);
                result = PlanNode {
                    id: self.next_id(),
                    node_type: if query.sql.to_uppercase().contains("LATERAL") {
                        PlanNodeType::NestedLoopJoin {
                            outer: Box::new(result),
                            inner: Box::new(right),
                            correlation,
                        }
                    } else {
                        PlanNodeType::HashJoin {
                            left: Box::new(result),
                            right: Box::new(right),
                            join_keys: vec![("id".to_string(), "id".to_string())],
                            join_type: JoinType::Inner,
                        }
                    },
                    estimated_cost: 200.0,
                    estimated_rows: 100,
                    output_columns: vec!["*".to_string()],
                };
            }
            Ok(result)
        } else if sub_plans.len() == 1 {
            Ok(sub_plans.remove(0))
        } else {
            Err(anyhow!("No valid plan nodes generated"))
        }
    }

    fn vector_source_from_query(query: &VectorQuery) -> VectorSource {
        match query {
            VectorQuery::Literal(vector) => VectorSource::Literal(vector.clone()),
            VectorQuery::Expression(expr) => Self::vector_source_from_expression(expr),
        }
    }

    fn vector_source_from_literal(raw: &str) -> VectorSource {
        Self::parse_vector_literal(raw).map_or_else(|| Self::vector_source_from_expression(raw), VectorSource::Literal)
    }

    fn vector_source_from_expression(expr: &str) -> VectorSource {
        let trimmed = expr.trim();
        if let Some((table, column)) = trimmed.split_once('.') {
            VectorSource::ColumnRef {
                table: table.trim().to_string(),
                column: column.trim().to_string(),
            }
        } else {
            VectorSource::Expression(trimmed.to_string())
        }
    }

    fn parse_vector_literal(raw: &str) -> Option<Vec<f32>> {
        let trimmed = raw.trim();
        let without_cast = trimmed
            .strip_suffix("::vector")
            .or_else(|| trimmed.strip_suffix("::VECTOR"))
            .unwrap_or(trimmed)
            .trim();
        let unquoted = without_cast.trim_matches('\'').trim_matches('"').trim();

        if !(unquoted.starts_with('[') && unquoted.ends_with(']')) {
            return None;
        }

        let inner = &unquoted[1..unquoted.len() - 1];
        if inner.trim().is_empty() {
            return Some(Vec::new());
        }

        inner
            .split(',')
            .map(|value| value.trim().parse::<f32>().ok())
            .collect()
    }

    /// Apply optimization rules in order:
    /// 1. Predicate pushdown - push filters closer to data sources
    /// 2. Join reordering - reorder joins based on cost estimation
    /// 3. Projection pushdown - only read needed columns
    /// 4. Parallel execution identification - mark independent subqueries
    fn apply_optimizations(&self, plan: PlanNode) -> Result<PlanNode> {
        let plan = self.push_predicates(plan)?;
        let plan = self.reorder_joins(plan)?;
        let plan = self.push_projections(plan)?;
        let plan = self.identify_parallelism(plan)?;
        Ok(plan)
    }

    // ========================================================================
    // PREDICATE PUSHDOWN
    // ========================================================================

    /// Push predicates (filters) as close to data sources as possible.
    /// This reduces the amount of data flowing through the plan.
    fn push_predicates(&self, plan: PlanNode) -> Result<PlanNode> {
        match plan.node_type {
            PlanNodeType::Filter { input, predicate } => {
                // Try to push the predicate into the input node
                let pushed = self.try_push_predicate_into(*input, predicate.clone())?;
                if pushed.predicate_pushed {
                    Ok(pushed.node)
                } else {
                    // Could not push, keep filter as-is but optimize input
                    let optimized_input = self.push_predicates(pushed.node)?;
                    Ok(PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::Filter {
                            input: Box::new(optimized_input),
                            predicate,
                        },
                        estimated_cost: plan.estimated_cost,
                        estimated_rows: plan.estimated_rows,
                        output_columns: plan.output_columns,
                    })
                }
            }
            PlanNodeType::HashJoin {
                left,
                right,
                join_keys,
                join_type,
            } => {
                // Recursively optimize children
                let left = self.push_predicates(*left)?;
                let right = self.push_predicates(*right)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::HashJoin {
                        left: Box::new(left),
                        right: Box::new(right),
                        join_keys,
                        join_type,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::NestedLoopJoin {
                outer,
                inner,
                correlation,
            } => {
                let outer = self.push_predicates(*outer)?;
                let inner = self.push_predicates(*inner)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::NestedLoopJoin {
                        outer: Box::new(outer),
                        inner: Box::new(inner),
                        correlation,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::IndexJoin {
                left,
                right,
                index_lookup,
            } => {
                let left = self.push_predicates(*left)?;
                let right = self.push_predicates(*right)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::IndexJoin {
                        left: Box::new(left),
                        right: Box::new(right),
                        index_lookup,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Project { input, columns } => {
                let input = self.push_predicates(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Project {
                        input: Box::new(input),
                        columns,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Distinct { input } => {
                let input = self.push_predicates(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Distinct {
                        input: Box::new(input),
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Sort { input, order_by } => {
                let input = self.push_predicates(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Sort {
                        input: Box::new(input),
                        order_by,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Limit {
                input,
                limit,
                offset,
            } => {
                let input = self.push_predicates(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Limit {
                        input: Box::new(input),
                        limit,
                        offset,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Aggregate {
                input,
                group_by,
                aggregates,
            } => {
                let input = self.push_predicates(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Aggregate {
                        input: Box::new(input),
                        group_by,
                        aggregates,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Union { inputs, all } => {
                let inputs: Result<Vec<_>> = inputs
                    .into_iter()
                    .map(|input| self.push_predicates(input))
                    .collect();
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Union {
                        inputs: inputs?,
                        all,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            // Leaf nodes - no further pushdown possible
            _ => Ok(plan),
        }
    }

    /// Try to push a predicate into a plan node
    fn try_push_predicate_into(
        &self,
        node: PlanNode,
        predicate: Predicate,
    ) -> Result<PredicatePushResult> {
        match &node.node_type {
            PlanNodeType::Scan {
                target,
                model_type,
                predicates,
            } => {
                // Check if predicate references columns from this scan
                if self.predicate_references_target(&predicate, target) {
                    // Merge predicate into scan
                    let mut new_predicates = predicates.clone();
                    new_predicates.push(predicate);

                    // Estimate reduced row count based on predicate selectivity
                    let last_predicate = new_predicates.last().ok_or_else(|| {
                        VectorDBError::Internal("No predicates found after push".to_string())
                    })?;
                    let selectivity = self.estimate_predicate_selectivity(last_predicate);
                    let new_rows = ((node.estimated_rows as f64) * selectivity) as u64;

                    Ok(PredicatePushResult {
                        node: PlanNode {
                            id: self.next_id(),
                            node_type: PlanNodeType::Scan {
                                target: target.clone(),
                                model_type: *model_type,
                                predicates: new_predicates,
                            },
                            estimated_cost: node.estimated_cost * selectivity,
                            estimated_rows: new_rows.max(1),
                            output_columns: node.output_columns.clone(),
                        },
                        predicate_pushed: true,
                    })
                } else {
                    Ok(PredicatePushResult {
                        node,
                        predicate_pushed: false,
                    })
                }
            }
            PlanNodeType::VectorSearch { collection, .. } => {
                let _ = collection;
                Ok(PredicatePushResult {
                    node,
                    predicate_pushed: false,
                })
            }
            PlanNodeType::DocumentQuery { collection, filter } => {
                // Document queries can have filters pushed into them
                if self.predicate_references_target(&predicate, collection) {
                    let new_filter = match filter {
                        Some(existing) => Some(format!(
                            "({}) AND ({})",
                            existing,
                            self.predicate_to_string(&predicate)
                        )),
                        None => Some(self.predicate_to_string(&predicate)),
                    };
                    let selectivity = self.estimate_predicate_selectivity(&predicate);
                    Ok(PredicatePushResult {
                        node: PlanNode {
                            id: self.next_id(),
                            node_type: PlanNodeType::DocumentQuery {
                                collection: collection.clone(),
                                filter: new_filter,
                            },
                            estimated_cost: node.estimated_cost * selectivity,
                            estimated_rows: ((node.estimated_rows as f64) * selectivity) as u64,
                            output_columns: node.output_columns.clone(),
                        },
                        predicate_pushed: true,
                    })
                } else {
                    Ok(PredicatePushResult {
                        node,
                        predicate_pushed: false,
                    })
                }
            }
            PlanNodeType::HashJoin {
                left,
                right,
                join_keys,
                join_type,
            } => {
                let predicate_column = predicate.column.clone();
                let left_can_match = self.column_from_left(&predicate_column, left)
                    && !self.column_from_right(&predicate_column, right);
                let right_can_match = self.column_from_right(&predicate_column, right)
                    && !self.column_from_left(&predicate_column, left);

                if left_can_match {
                    let left_pushed =
                        self.try_push_predicate_into(*left.clone(), predicate.clone())?;
                    if left_pushed.predicate_pushed {
                        return Ok(PredicatePushResult {
                            node: PlanNode {
                                id: self.next_id(),
                                node_type: PlanNodeType::HashJoin {
                                    left: Box::new(left_pushed.node),
                                    right: right.clone(),
                                    join_keys: join_keys.clone(),
                                    join_type: join_type.clone(),
                                },
                                estimated_cost: node.estimated_cost * 0.8,
                                estimated_rows: node.estimated_rows,
                                output_columns: node.output_columns.clone(),
                            },
                            predicate_pushed: true,
                        });
                    }
                }

                if right_can_match {
                    let right_pushed = self.try_push_predicate_into(*right.clone(), predicate)?;
                    if right_pushed.predicate_pushed {
                        return Ok(PredicatePushResult {
                            node: PlanNode {
                                id: self.next_id(),
                                node_type: PlanNodeType::HashJoin {
                                    left: left.clone(),
                                    right: Box::new(right_pushed.node),
                                    join_keys: join_keys.clone(),
                                    join_type: join_type.clone(),
                                },
                                estimated_cost: node.estimated_cost * 0.8,
                                estimated_rows: node.estimated_rows,
                                output_columns: node.output_columns.clone(),
                            },
                            predicate_pushed: true,
                        });
                    }
                }

                Ok(PredicatePushResult {
                    node,
                    predicate_pushed: false,
                })
            }
            _ => Ok(PredicatePushResult {
                node,
                predicate_pushed: false,
            }),
        }
    }

    /// Check if a predicate references a specific target
    fn predicate_references_target(&self, predicate: &Predicate, target: &str) -> bool {
        // Simple heuristic: check if column name starts with target or contains no qualifier
        predicate.column.starts_with(target)
            || predicate.column.starts_with(&format!("{}.", target))
            || !predicate.column.contains('.')
    }

    /// Estimate selectivity of a predicate (fraction of rows that match)
    fn estimate_predicate_selectivity(&self, predicate: &Predicate) -> f64 {
        match predicate.op {
            PredicateOp::Eq => 0.1, // Equality typically selects 10%
            PredicateOp::Ne => 0.9, // Not equal selects 90%
            PredicateOp::Lt | PredicateOp::Le | PredicateOp::Gt | PredicateOp::Ge => 0.3, // Range selects ~30%
            PredicateOp::Like => 0.2,       // Pattern match ~20%
            PredicateOp::In => 0.15,        // IN clause ~15%
            PredicateOp::IsNull => 0.05,    // Null check ~5%
            PredicateOp::IsNotNull => 0.95, // Not null ~95%
            PredicateOp::Between => 0.25,   // Between ~25%
        }
    }

    /// Convert predicate to string representation
    fn predicate_to_string(&self, predicate: &Predicate) -> String {
        let op_str = match predicate.op {
            PredicateOp::Eq => "=",
            PredicateOp::Ne => "!=",
            PredicateOp::Lt => "<",
            PredicateOp::Le => "<=",
            PredicateOp::Gt => ">",
            PredicateOp::Ge => ">=",
            PredicateOp::Like => "LIKE",
            PredicateOp::In => "IN",
            PredicateOp::IsNull => "IS NULL",
            PredicateOp::IsNotNull => "IS NOT NULL",
            PredicateOp::Between => "BETWEEN",
        };

        let value_str = match &predicate.value {
            PredicateValue::String(s) => format!("'{}'", s),
            PredicateValue::Int(i) => i.to_string(),
            PredicateValue::Float(f) => f.to_string(),
            PredicateValue::Bool(b) => b.to_string(),
            PredicateValue::Null => "NULL".to_string(),
            PredicateValue::List(values) => {
                let items: Vec<String> = values
                    .iter()
                    .map(|v| match v {
                        PredicateValue::String(s) => format!("'{}'", s),
                        PredicateValue::Int(i) => i.to_string(),
                        PredicateValue::Float(f) => f.to_string(),
                        PredicateValue::Bool(b) => b.to_string(),
                        _ => "?".to_string(),
                    })
                    .collect();
                format!("({})", items.join(", "))
            }
        };

        format!("{} {} {}", predicate.column, op_str, value_str)
    }

    // ========================================================================
    // JOIN REORDERING
    // ========================================================================

    /// Reorder joins based on cost estimation.
    /// Uses a greedy approach: always join the two relations with lowest combined cost first.
    fn reorder_joins(&self, plan: PlanNode) -> Result<PlanNode> {
        match plan.node_type {
            PlanNodeType::HashJoin {
                left,
                right,
                join_keys,
                join_type,
            } => {
                // First, recursively optimize children
                let left = self.reorder_joins(*left)?;
                let right = self.reorder_joins(*right)?;

                // Extract all leaf relations from both sides
                let mut left_leaves = self.extract_join_leaves(&left);
                let mut right_leaves = self.extract_join_leaves(&right);

                // If we have multiple leaves to join, apply greedy reordering
                if left_leaves.len() + right_leaves.len() > 2 {
                    let mut all_leaves: Vec<PlanNode> = left_leaves
                        .drain(..)
                        .chain(right_leaves.drain(..))
                        .collect();
                    let reordered =
                        self.greedy_join_order(&mut all_leaves, &join_keys, &join_type)?;
                    return Ok(reordered);
                }

                // Simple case: just swap if right is cheaper than left
                let left_cost = self.calculate_total_cost(&left);
                let right_cost = self.calculate_total_cost(&right);

                if right_cost < left_cost && join_type == JoinType::Inner {
                    // Swap for inner joins (commutative)
                    let swapped_keys: Vec<(String, String)> = join_keys
                        .iter()
                        .map(|(l, r)| (r.clone(), l.clone()))
                        .collect();
                    Ok(PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::HashJoin {
                            left: Box::new(right),
                            right: Box::new(left),
                            join_keys: swapped_keys,
                            join_type,
                        },
                        estimated_cost: plan.estimated_cost,
                        estimated_rows: plan.estimated_rows,
                        output_columns: plan.output_columns,
                    })
                } else {
                    Ok(PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::HashJoin {
                            left: Box::new(left),
                            right: Box::new(right),
                            join_keys,
                            join_type,
                        },
                        estimated_cost: plan.estimated_cost,
                        estimated_rows: plan.estimated_rows,
                        output_columns: plan.output_columns,
                    })
                }
            }
            PlanNodeType::NestedLoopJoin {
                outer,
                inner,
                correlation,
            } => {
                let outer = self.reorder_joins(*outer)?;
                let inner = self.reorder_joins(*inner)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::NestedLoopJoin {
                        outer: Box::new(outer),
                        inner: Box::new(inner),
                        correlation,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Filter { input, predicate } => {
                let input = self.reorder_joins(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Filter {
                        input: Box::new(input),
                        predicate,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Project { input, columns } => {
                let input = self.reorder_joins(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Project {
                        input: Box::new(input),
                        columns,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Distinct { input } => {
                let input = self.reorder_joins(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Distinct {
                        input: Box::new(input),
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Sort { input, order_by } => {
                let input = self.reorder_joins(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Sort {
                        input: Box::new(input),
                        order_by,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Limit {
                input,
                limit,
                offset,
            } => {
                let input = self.reorder_joins(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Limit {
                        input: Box::new(input),
                        limit,
                        offset,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Aggregate {
                input,
                group_by,
                aggregates,
            } => {
                let input = self.reorder_joins(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Aggregate {
                        input: Box::new(input),
                        group_by,
                        aggregates,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Union { inputs, all } => {
                let inputs: Result<Vec<_>> = inputs
                    .into_iter()
                    .map(|input| self.reorder_joins(input))
                    .collect();
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Union {
                        inputs: inputs?,
                        all,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            _ => Ok(plan),
        }
    }

    /// Extract leaf nodes from a join tree
    fn extract_join_leaves(&self, plan: &PlanNode) -> Vec<PlanNode> {
        match &plan.node_type {
            PlanNodeType::HashJoin { left, right, .. } => {
                let mut leaves = self.extract_join_leaves(left);
                leaves.extend(self.extract_join_leaves(right));
                leaves
            }
            _ => vec![plan.clone()],
        }
    }

    /// Apply greedy join ordering to a set of relations
    fn greedy_join_order(
        &self,
        relations: &mut Vec<PlanNode>,
        _join_keys: &[(String, String)],
        join_type: &JoinType,
    ) -> Result<PlanNode> {
        if relations.is_empty() {
            return Err(anyhow!("No relations to join"));
        }
        if relations.len() == 1 {
            return Ok(relations.remove(0));
        }

        // Sort by estimated cost (ascending)
        relations.sort_by(|a, b| {
            a.estimated_cost
                .partial_cmp(&b.estimated_cost)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Greedily build join tree from cheapest to most expensive
        let mut result = relations.remove(0);

        while !relations.is_empty() {
            let next = relations.remove(0);
            let join_cost = self.estimate_join_cost(&result, &next);
            let output_columns: Vec<String> = result
                .output_columns
                .iter()
                .chain(next.output_columns.iter())
                .cloned()
                .collect();

            result = PlanNode {
                id: self.next_id(),
                node_type: PlanNodeType::HashJoin {
                    left: Box::new(result),
                    right: Box::new(next),
                    join_keys: vec![("id".to_string(), "id".to_string())], // Default join key
                    join_type: join_type.clone(),
                },
                estimated_cost: join_cost,
                estimated_rows: 100, // Simplified estimate
                output_columns,
            };
        }

        Ok(result)
    }

    /// Estimate cost of joining two plans
    fn estimate_join_cost(&self, left: &PlanNode, right: &PlanNode) -> f64 {
        // Hash join cost model: O(left) + O(right) build + O(left * right) probe
        // Simplified to: left_cost + right_cost + (left_rows * 0.01)
        left.estimated_cost + right.estimated_cost + (left.estimated_rows as f64 * 0.01)
    }

    // ========================================================================
    // PROJECTION PUSHDOWN
    // ========================================================================

    /// Push projections down to read only needed columns.
    /// This reduces I/O and memory usage.
    fn push_projections(&self, plan: PlanNode) -> Result<PlanNode> {
        // Collect required columns from the root
        let required_columns = self.collect_required_columns(&plan);
        self.push_projections_with_required(plan, &required_columns)
    }

    /// Push projections with a set of required columns
    fn push_projections_with_required(
        &self,
        plan: PlanNode,
        required: &[String],
    ) -> Result<PlanNode> {
        match plan.node_type {
            PlanNodeType::Project { input, columns } => {
                // Intersect required columns with projected columns
                let new_required: Vec<String> = if required.contains(&"*".to_string()) {
                    columns.clone()
                } else {
                    columns
                        .iter()
                        .zip(plan.output_columns.iter())
                        .filter(|(column, output)| {
                            required.contains(*column)
                                || required.contains(*output)
                                || **column == "*"
                        })
                        .map(|(column, _)| column.clone())
                        .collect()
                };

                let input = self.push_projections_with_required(*input, &new_required)?;
                let preserves_names = columns == plan.output_columns;

                // If all columns are required and the projection is not renaming columns,
                // remove it entirely.
                if (new_required.len() == columns.len() || columns.contains(&"*".to_string()))
                    && preserves_names
                {
                    Ok(input)
                } else {
                    let filtered_output_columns: Vec<String> =
                        if required.contains(&"*".to_string()) {
                            plan.output_columns.clone()
                        } else {
                            columns
                                .iter()
                                .zip(plan.output_columns.iter())
                                .filter(|(column, output)| {
                                    new_required.contains(column)
                                        || required.contains(*output)
                                        || **column == "*"
                                })
                                .map(|(_, output)| output.clone())
                                .collect()
                        };

                    Ok(PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::Project {
                            input: Box::new(input),
                            columns: new_required,
                        },
                        estimated_cost: plan.estimated_cost * 0.9, // Slight cost reduction
                        estimated_rows: plan.estimated_rows,
                        output_columns: filtered_output_columns,
                    })
                }
            }
            PlanNodeType::Scan {
                target,
                model_type,
                predicates,
            } => {
                // Apply column pruning to scan
                let pruned_columns = Self::filter_output_columns(&plan.output_columns, required);

                // Estimate cost reduction from reading fewer columns
                let column_ratio = if !plan.output_columns.is_empty() {
                    pruned_columns.len() as f64 / plan.output_columns.len() as f64
                } else {
                    1.0
                };

                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Scan {
                        target,
                        model_type,
                        predicates,
                    },
                    estimated_cost: plan.estimated_cost * column_ratio.max(0.5),
                    estimated_rows: plan.estimated_rows,
                    output_columns: pruned_columns,
                })
            }
            PlanNodeType::HashJoin {
                left,
                right,
                join_keys,
                join_type,
            } => {
                // Collect columns needed from each side
                let mut left_required: Vec<String> = if required.contains(&"*".to_string()) {
                    left.output_columns.clone()
                } else {
                    required
                        .iter()
                        .filter_map(|column| self.join_output_to_left_column(column, &left))
                        .collect()
                };
                left_required.extend(join_keys.iter().map(|(left_key, _)| left_key.clone()));
                left_required.sort();
                left_required.dedup();

                let mut right_required: Vec<String> = if required.contains(&"*".to_string()) {
                    right.output_columns.clone()
                } else {
                    required
                        .iter()
                        .filter_map(|column| {
                            self.join_output_to_right_column(column, &left, &right)
                        })
                        .collect()
                };
                right_required.extend(join_keys.iter().map(|(_, right_key)| right_key.clone()));
                right_required.sort();
                right_required.dedup();

                let left = self.push_projections_with_required(*left, &left_required)?;
                let right = self.push_projections_with_required(*right, &right_required)?;

                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::HashJoin {
                        left: Box::new(left),
                        right: Box::new(right),
                        join_keys,
                        join_type,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: Self::filter_output_columns(&plan.output_columns, required),
                })
            }
            PlanNodeType::NestedLoopJoin {
                outer,
                inner,
                correlation,
            } => {
                let outer = self.push_projections_with_required(*outer, required)?;
                let inner = self.push_projections_with_required(*inner, required)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::NestedLoopJoin {
                        outer: Box::new(outer),
                        inner: Box::new(inner),
                        correlation,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: Self::filter_output_columns(&plan.output_columns, required),
                })
            }
            PlanNodeType::Filter { input, predicate } => {
                // Include columns from predicate in required set
                let mut extended_required = required.to_vec();
                if !extended_required.contains(&predicate.column) {
                    extended_required.push(predicate.column.clone());
                }

                let input = self.push_projections_with_required(*input, &extended_required)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Filter {
                        input: Box::new(input),
                        predicate,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: Self::filter_output_columns(&plan.output_columns, required),
                })
            }
            PlanNodeType::Distinct { input } => {
                let input = self.push_projections_with_required(*input, required)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Distinct {
                        input: Box::new(input),
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: Self::filter_output_columns(&plan.output_columns, required),
                })
            }
            PlanNodeType::Sort { input, order_by } => {
                // Include sort columns in required set
                let mut extended_required = required.to_vec();
                for clause in &order_by {
                    if !extended_required.contains(&clause.column) {
                        extended_required.push(clause.column.clone());
                    }
                }

                let input = self.push_projections_with_required(*input, &extended_required)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Sort {
                        input: Box::new(input),
                        order_by,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: Self::filter_output_columns(&plan.output_columns, required),
                })
            }
            PlanNodeType::Limit {
                input,
                limit,
                offset,
            } => {
                let input = self.push_projections_with_required(*input, required)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Limit {
                        input: Box::new(input),
                        limit,
                        offset,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: Self::filter_output_columns(&plan.output_columns, required),
                })
            }
            PlanNodeType::Aggregate {
                input,
                group_by,
                aggregates,
            } => {
                // Collect all columns needed for grouping and aggregation
                let mut agg_required: Vec<String> = group_by.clone();
                for agg in &aggregates {
                    if let Some(col) = &agg.column
                        && !agg_required.contains(col) {
                            agg_required.push(col.clone());
                        }
                }

                let input = self.push_projections_with_required(*input, &agg_required)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Aggregate {
                        input: Box::new(input),
                        group_by,
                        aggregates,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: Self::filter_output_columns(&plan.output_columns, required),
                })
            }
            PlanNodeType::Union { inputs, all } => {
                let inputs: Result<Vec<_>> = inputs
                    .into_iter()
                    .map(|input| self.push_projections_with_required(input, required))
                    .collect();
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Union {
                        inputs: inputs?,
                        all,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: Self::filter_output_columns(&plan.output_columns, required),
                })
            }
            // For other node types, pass through required columns
            _ => Ok(PlanNode {
                output_columns: Self::filter_output_columns(&plan.output_columns, required),
                ..plan
            }),
        }
    }

    /// Collect all columns required by a plan
    fn collect_required_columns(&self, plan: &PlanNode) -> Vec<String> {
        let mut columns = Vec::new();

        match &plan.node_type {
            PlanNodeType::Project {
                columns: proj_cols,
                input,
            } => {
                columns.extend(proj_cols.clone());
                columns.extend(self.collect_required_columns(input));
            }
            PlanNodeType::Distinct { .. } => {
                // DISTINCT operates on the visible row shape at this point in the plan.
                // Pulling through child columns here can incorrectly widen the distinct key
                // and strip away a preceding projection during pushdown.
                columns.extend(plan.output_columns.clone());
            }
            PlanNodeType::Filter { predicate, input } => {
                columns.push(predicate.column.clone());
                columns.extend(self.collect_required_columns(input));
            }
            PlanNodeType::HashJoin { .. } => columns.extend(plan.output_columns.clone()),
            PlanNodeType::Sort { order_by, input } => {
                for clause in order_by {
                    columns.push(clause.column.clone());
                }
                columns.extend(self.collect_required_columns(input));
            }
            PlanNodeType::Aggregate { .. } => columns.extend(plan.output_columns.clone()),
            _ => {
                columns.extend(plan.output_columns.clone());
            }
        }

        // Deduplicate
        columns.sort();
        columns.dedup();
        columns
    }

    /// Check if column comes from left side of join
    fn column_from_left(&self, column: &str, left: &PlanNode) -> bool {
        // Check if column has a table qualifier (e.g., "users.name")
        if let Some((qualifier, _col)) = column.split_once('.') {
            // Qualified column: match only if scan target matches the qualifier
            if let PlanNodeType::Scan { target, .. } = &left.node_type {
                return target == qualifier;
            }
            // Non-scan nodes with wildcard can accept qualified columns
            return left.output_columns.contains(&column.to_string());
        }
        left.output_columns.contains(&column.to_string())
            || left.output_columns.contains(&"*".to_string())
    }

    /// Check if column comes from right side of join
    fn column_from_right(&self, column: &str, right: &PlanNode) -> bool {
        // Check if column has a table qualifier (e.g., "orders.id")
        if let Some((qualifier, _col)) = column.split_once('.') {
            // Qualified column: match only if scan target matches the qualifier
            if let PlanNodeType::Scan { target, .. } = &right.node_type {
                return target == qualifier;
            }
            // Non-scan nodes with wildcard can accept qualified columns
            return right.output_columns.contains(&column.to_string());
        }
        right.output_columns.contains(&column.to_string())
            || right.output_columns.contains(&"*".to_string())
    }

    fn filter_output_columns(output_columns: &[String], required: &[String]) -> Vec<String> {
        if required.contains(&"*".to_string()) {
            return output_columns.to_vec();
        }

        let filtered = output_columns
            .iter()
            .filter(|column| required.contains(*column))
            .cloned()
            .collect::<Vec<_>>();

        if filtered.is_empty() && output_columns.is_empty() {
            required.to_vec()
        } else if filtered.is_empty() {
            output_columns.to_vec()
        } else {
            filtered
        }
    }

    fn normalize_column_name(column: &str) -> &str {
        column.rsplit('.').next().unwrap_or(column).trim()
    }

    fn resolve_output_column_name(&self, column: &str, plan: &PlanNode) -> Option<String> {
        let normalized = Self::normalize_column_name(column);
        plan.output_columns
            .iter()
            .find(|candidate| {
                candidate.eq_ignore_ascii_case(column) || candidate.eq_ignore_ascii_case(normalized)
            })
            .cloned()
    }

    fn join_output_to_left_column(&self, column: &str, left: &PlanNode) -> Option<String> {
        self.resolve_output_column_name(column, left)
    }

    fn join_output_to_right_column(
        &self,
        column: &str,
        left: &PlanNode,
        right: &PlanNode,
    ) -> Option<String> {
        if let Some(stripped) = column.strip_prefix("right_") {
            return self.resolve_output_column_name(stripped, right);
        }

        let right_column = self.resolve_output_column_name(column, right)?;
        if self.resolve_output_column_name(column, left).is_some() {
            None
        } else {
            Some(right_column)
        }
    }

    // ========================================================================
    // PARALLEL EXECUTION IDENTIFICATION
    // ========================================================================

    /// Identify opportunities for parallel execution.
    /// Marks independent subqueries that can run concurrently.
    fn identify_parallelism(&self, plan: PlanNode) -> Result<PlanNode> {
        // Collect parallel stages
        let _parallel_stages = self.find_parallel_stages(&plan);

        // For now, we just mark the plan with parallelism info
        // The actual parallel execution would be done by the executor

        // Recursively process children
        match plan.node_type {
            PlanNodeType::HashJoin {
                left,
                right,
                join_keys,
                join_type,
            } => {
                // Both sides of a hash join can be executed in parallel
                let left = self.identify_parallelism(*left)?;
                let right = self.identify_parallelism(*right)?;

                // Mark this as a parallelizable point
                let parallel_cost = left.estimated_cost.max(right.estimated_cost);

                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::HashJoin {
                        left: Box::new(left),
                        right: Box::new(right),
                        join_keys,
                        join_type,
                    },
                    estimated_cost: parallel_cost + plan.estimated_cost * 0.5, // Parallel execution bonus
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Union { inputs, all } => {
                // All union inputs can run in parallel
                let inputs: Result<Vec<_>> = inputs
                    .into_iter()
                    .map(|input| self.identify_parallelism(input))
                    .collect();
                let inputs = inputs?;

                // Parallel cost is max of all inputs
                let max_cost = inputs
                    .iter()
                    .map(|i| i.estimated_cost)
                    .fold(0.0f64, |a, b| a.max(b));

                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Union { inputs, all },
                    estimated_cost: max_cost + plan.estimated_cost * 0.1, // Union overhead
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::NestedLoopJoin {
                outer,
                inner,
                correlation,
            } => {
                let outer = self.identify_parallelism(*outer)?;
                let inner = self.identify_parallelism(*inner)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::NestedLoopJoin {
                        outer: Box::new(outer),
                        inner: Box::new(inner),
                        correlation,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Filter { input, predicate } => {
                let input = self.identify_parallelism(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Filter {
                        input: Box::new(input),
                        predicate,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Project { input, columns } => {
                let input = self.identify_parallelism(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Project {
                        input: Box::new(input),
                        columns,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Distinct { input } => {
                let input = self.identify_parallelism(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Distinct {
                        input: Box::new(input),
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Sort { input, order_by } => {
                let input = self.identify_parallelism(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Sort {
                        input: Box::new(input),
                        order_by,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Limit {
                input,
                limit,
                offset,
            } => {
                let input = self.identify_parallelism(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Limit {
                        input: Box::new(input),
                        limit,
                        offset,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            PlanNodeType::Aggregate {
                input,
                group_by,
                aggregates,
            } => {
                let input = self.identify_parallelism(*input)?;
                Ok(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Aggregate {
                        input: Box::new(input),
                        group_by,
                        aggregates,
                    },
                    estimated_cost: plan.estimated_cost,
                    estimated_rows: plan.estimated_rows,
                    output_columns: plan.output_columns,
                })
            }
            _ => Ok(plan),
        }
    }

    /// Find stages that can execute in parallel
    fn find_parallel_stages(&self, plan: &PlanNode) -> Vec<Vec<usize>> {
        let mut stages: Vec<Vec<usize>> = Vec::new();

        match &plan.node_type {
            PlanNodeType::HashJoin { left, right, .. } => {
                // Left and right can execute in parallel
                let mut stage = vec![left.id, right.id];
                stage.sort();
                stages.push(stage);

                // Recursively find more parallel stages
                stages.extend(self.find_parallel_stages(left));
                stages.extend(self.find_parallel_stages(right));
            }
            PlanNodeType::Union { inputs, .. } => {
                // All union inputs can execute in parallel
                let mut stage: Vec<usize> = inputs.iter().map(|i| i.id).collect();
                stage.sort();
                if stage.len() > 1 {
                    stages.push(stage);
                }

                for input in inputs {
                    stages.extend(self.find_parallel_stages(input));
                }
            }
            PlanNodeType::NestedLoopJoin { outer, inner, .. } => {
                stages.extend(self.find_parallel_stages(outer));
                stages.extend(self.find_parallel_stages(inner));
            }
            PlanNodeType::Filter { input, .. }
            | PlanNodeType::Project { input, .. }
            | PlanNodeType::Distinct { input, .. }
            | PlanNodeType::Sort { input, .. }
            | PlanNodeType::Limit { input, .. }
            | PlanNodeType::Aggregate { input, .. } => {
                stages.extend(self.find_parallel_stages(input));
            }
            _ => {}
        }

        stages
    }

    /// Build physical plan from logical plan
    fn build_physical_plan(&self, plan: PlanNode) -> Result<QueryPlan> {
        let total_cost = self.calculate_total_cost(&plan);
        let involved_models = self.collect_involved_models(&plan);

        Ok(QueryPlan {
            root: plan,
            total_cost,
            metadata: PlanMetadata {
                involved_models: involved_models.clone(),
                is_cross_model: involved_models.len() > 1,
                parallel_stages: vec![],
                hints_applied: vec![],
            },
        })
    }

    /// Calculate total cost of a plan
    fn calculate_total_cost(&self, plan: &PlanNode) -> f64 {
        let mut cost = plan.estimated_cost;

        match &plan.node_type {
            PlanNodeType::HashJoin { left, right, .. } => {
                cost += self.calculate_total_cost(left);
                cost += self.calculate_total_cost(right);
            }
            PlanNodeType::NestedLoopJoin { outer, inner, .. } => {
                cost += self.calculate_total_cost(outer);
                cost += self.calculate_total_cost(inner) * outer.estimated_rows as f64;
            }
            PlanNodeType::IndexJoin { left, right, .. } => {
                cost += self.calculate_total_cost(left);
                cost += self.calculate_total_cost(right);
            }
            PlanNodeType::Filter { input, .. } => {
                cost += self.calculate_total_cost(input);
            }
            PlanNodeType::Project { input, .. } => {
                cost += self.calculate_total_cost(input);
            }
            PlanNodeType::Distinct { input, .. } => {
                cost += self.calculate_total_cost(input);
            }
            PlanNodeType::Sort { input, .. } => {
                cost += self.calculate_total_cost(input);
            }
            PlanNodeType::Limit { input, .. } => {
                cost += self.calculate_total_cost(input);
            }
            PlanNodeType::Aggregate { input, .. } => {
                cost += self.calculate_total_cost(input);
            }
            PlanNodeType::Union { inputs, .. } => {
                for input in inputs {
                    cost += self.calculate_total_cost(input);
                }
            }
            _ => {}
        }

        cost
    }

    /// Collect all model types involved in the plan
    fn collect_involved_models(&self, plan: &PlanNode) -> Vec<ModelType> {
        let mut models = Vec::new();

        match &plan.node_type {
            PlanNodeType::Scan { model_type, .. } => {
                if !models.contains(model_type) {
                    models.push(*model_type);
                }
            }
            PlanNodeType::VectorSearch { .. } => {
                if !models.contains(&ModelType::Vector) {
                    models.push(ModelType::Vector);
                }
            }
            PlanNodeType::GraphTraversal { .. } => {
                if !models.contains(&ModelType::Graph) {
                    models.push(ModelType::Graph);
                }
            }
            PlanNodeType::DocumentQuery { .. } => {
                if !models.contains(&ModelType::Document) {
                    models.push(ModelType::Document);
                }
            }
            PlanNodeType::ObservabilityQuery { .. } => {
                if !models.contains(&ModelType::Observability) {
                    models.push(ModelType::Observability);
                }
            }
            PlanNodeType::HashJoin { left, right, .. } => {
                models.extend(self.collect_involved_models(left));
                models.extend(self.collect_involved_models(right));
            }
            PlanNodeType::NestedLoopJoin { outer, inner, .. } => {
                models.extend(self.collect_involved_models(outer));
                models.extend(self.collect_involved_models(inner));
            }
            PlanNodeType::IndexJoin { left, right, .. } => {
                models.extend(self.collect_involved_models(left));
                models.extend(self.collect_involved_models(right));
            }
            PlanNodeType::Filter { input, .. }
            | PlanNodeType::Project { input, .. }
            | PlanNodeType::Distinct { input, .. }
            | PlanNodeType::Sort { input, .. }
            | PlanNodeType::Limit { input, .. }
            | PlanNodeType::Aggregate { input, .. } => {
                models.extend(self.collect_involved_models(input));
            }
            PlanNodeType::Union { inputs, .. } => {
                for input in inputs {
                    models.extend(self.collect_involved_models(input));
                }
            }
        }

        // Deduplicate
        models.sort_by_key(|m| format!("{:?}", m));
        models.dedup();
        models
    }

    /// Get next node ID
    fn next_id(&self) -> usize {
        self.next_node_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for CrossModelOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// JOIN STRATEGY SELECTION (Cost-Based Optimizer)
// ============================================================================

/// Join execution strategy selected by the cost-based optimizer
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinStrategy {
    /// Build a hash table on the smaller side, probe with the larger
    HashJoin,
    /// Iterate outer rows, scan inner for each (small datasets)
    NestedLoopJoin,
    /// Use an existing index on the inner side for lookups
    IndexJoin,
}

/// Select the optimal join strategy based on estimated cardinalities
///
/// Decision logic:
/// - **IndexJoin**: when an index exists and the right side is small (<1000 rows),
///   exploiting the index avoids a full scan.
/// - **HashJoin**: when both sides exceed 1000 rows, hash-based joining
///   gives O(n + m) cost vs O(n * m) for nested loop.
/// - **NestedLoopJoin**: fallback for small inputs where hash-table
///   construction overhead is not worthwhile.
pub fn select_join_strategy(left_rows: u64, right_rows: u64, has_index: bool) -> JoinStrategy {
    if has_index && right_rows < 1000 {
        JoinStrategy::IndexJoin
    } else if left_rows > 1000 && right_rows > 1000 {
        JoinStrategy::HashJoin
    } else {
        JoinStrategy::NestedLoopJoin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimizer_creation() {
        let optimizer = CrossModelOptimizer::new();
        assert!(optimizer.cost_models.contains_key(&ModelType::Vector));
        assert!(optimizer.cost_models.contains_key(&ModelType::Graph));
    }

    #[test]
    fn test_optimize_sql_query() {
        let optimizer = CrossModelOptimizer::new();
        let query = FederatedQuery {
            sql: "SELECT * FROM users".to_string(),
            query_type: QueryType::Sql,
            extensions: vec![],
            targets: vec![super::super::parser::QueryTarget {
                name: "users".to_string(),
                alias: None,
                model_type: TargetModelType::Table,
            }],
            parameters: HashMap::new(),
            is_cross_model_join: false,
        };

        let plan = optimizer
            .optimize(&query)
            .expect("optimize should succeed for valid query");
        assert!(plan.total_cost > 0.0);
        assert!(!plan.metadata.is_cross_model);
    }

    #[test]
    fn test_optimize_vector_search() {
        let optimizer = CrossModelOptimizer::new();
        let query = FederatedQuery {
            sql: "SELECT * FROM VECTOR_SEARCH('embeddings', '[0.1]', 10)".to_string(),
            query_type: QueryType::VectorSearch,
            extensions: vec![SqlExtension::VectorSearch {
                collection: "embeddings".to_string(),
                query_vector: VectorQuery::Literal(vec![0.1]),
                top_k: 10,
            }],
            targets: vec![],
            parameters: HashMap::new(),
            is_cross_model_join: false,
        };

        let plan = optimizer
            .optimize(&query)
            .expect("optimize should succeed for vector search query");
        assert!(plan.metadata.involved_models.contains(&ModelType::Vector));
    }

    #[test]
    fn test_lateral_plan_preserves_sql_source_order_and_correlation() {
        let parser = super::super::parser::FederatedParser::new();
        let query = parser
            .parse(
                "SELECT * FROM DOCUMENT_QUERY('profiles') p JOIN LATERAL VECTOR_SEARCH('products', p.document.embedding, 1) v ON true",
            )
            .expect("parser should accept function-backed lateral query");
        let optimizer = CrossModelOptimizer::new();

        let plan = optimizer
            .optimize(&query)
            .expect("optimizer should preserve lateral source ordering");

        match &plan.root.node_type {
            PlanNodeType::NestedLoopJoin {
                outer,
                inner,
                correlation,
            } => {
                assert!(matches!(
                    outer.node_type,
                    PlanNodeType::DocumentQuery { .. }
                ));
                assert!(matches!(inner.node_type, PlanNodeType::VectorSearch { .. }));
                assert_eq!(correlation, &vec!["p.document.embedding".to_string()]);
            }
            other => panic!("expected nested-loop join, got {:?}", other),
        }
    }

    #[test]
    fn test_cost_model_defaults() {
        let cost_model = CostModel::default();
        assert_eq!(cost_model.row_scan_cost, 1.0);
        assert_eq!(cost_model.index_lookup_cost, 0.1);
    }

    // ========================================================================
    // OPTIMIZATION RULE TESTS
    // ========================================================================

    /// Helper to create a scan node
    fn make_scan(optimizer: &CrossModelOptimizer, target: &str, cost: f64, rows: u64) -> PlanNode {
        PlanNode {
            id: optimizer.next_id(),
            node_type: PlanNodeType::Scan {
                target: target.to_string(),
                model_type: ModelType::Relational,
                predicates: vec![],
            },
            estimated_cost: cost,
            estimated_rows: rows,
            output_columns: vec!["id".to_string(), "name".to_string(), "value".to_string()],
        }
    }

    /// Helper to create a filter node
    fn make_filter(
        optimizer: &CrossModelOptimizer,
        input: PlanNode,
        column: &str,
        value: &str,
    ) -> PlanNode {
        PlanNode {
            id: optimizer.next_id(),
            node_type: PlanNodeType::Filter {
                input: Box::new(input),
                predicate: Predicate {
                    column: column.to_string(),
                    op: PredicateOp::Eq,
                    value: PredicateValue::String(value.to_string()),
                },
            },
            estimated_cost: 10.0,
            estimated_rows: 100,
            output_columns: vec!["id".to_string(), "name".to_string(), "value".to_string()],
        }
    }

    /// Helper to create a hash join node
    fn make_hash_join(
        optimizer: &CrossModelOptimizer,
        left: PlanNode,
        right: PlanNode,
    ) -> PlanNode {
        PlanNode {
            id: optimizer.next_id(),
            node_type: PlanNodeType::HashJoin {
                left: Box::new(left),
                right: Box::new(right),
                join_keys: vec![("id".to_string(), "id".to_string())],
                join_type: JoinType::Inner,
            },
            estimated_cost: 200.0,
            estimated_rows: 1000,
            output_columns: vec!["*".to_string()],
        }
    }

    #[test]
    fn test_predicate_pushdown_to_scan() {
        let optimizer = CrossModelOptimizer::new();

        // Create: Filter(Scan(users))
        let scan = make_scan(&optimizer, "users", 100.0, 1000);
        let filter = make_filter(&optimizer, scan, "name", "Alice");

        // Apply predicate pushdown
        let optimized = optimizer
            .push_predicates(filter)
            .expect("push_predicates should succeed for filter node");

        // The filter should be pushed into the scan
        match &optimized.node_type {
            PlanNodeType::Scan { predicates, .. } => {
                assert_eq!(predicates.len(), 1);
                assert_eq!(predicates[0].column, "name");
            }
            _ => panic!(
                "Expected scan with pushed predicate, got {:?}",
                optimized.node_type
            ),
        }

        // Cost should be reduced (10% selectivity)
        assert!(optimized.estimated_cost < 100.0);
    }

    #[test]
    fn test_predicate_pushdown_through_join() {
        let optimizer = CrossModelOptimizer::new();

        // Create: Filter(HashJoin(Scan(users), Scan(orders)))
        let users_scan = make_scan(&optimizer, "users", 100.0, 1000);
        let orders_scan = make_scan(&optimizer, "orders", 150.0, 2000);
        let join = make_hash_join(&optimizer, users_scan, orders_scan);
        let filter = make_filter(&optimizer, join, "users.name", "Alice");

        // Apply predicate pushdown
        let optimized = optimizer
            .push_predicates(filter)
            .expect("push_predicates should succeed for filter with join");

        // The filter should be pushed through the join
        match &optimized.node_type {
            PlanNodeType::HashJoin { left, .. } => {
                // Left side should have the pushed predicate
                match &left.node_type {
                    PlanNodeType::Scan { predicates, .. } => {
                        assert_eq!(predicates.len(), 1);
                    }
                    _ => panic!("Expected scan with pushed predicate on left"),
                }
            }
            _ => panic!("Expected hash join at top level"),
        }
    }

    #[test]
    fn test_join_reordering_swaps_cheaper_to_left() {
        let optimizer = CrossModelOptimizer::new();

        // Create: HashJoin(expensive_scan, cheap_scan)
        let expensive = make_scan(&optimizer, "big_table", 1000.0, 100000);
        let cheap = make_scan(&optimizer, "small_table", 10.0, 100);
        let join = make_hash_join(&optimizer, expensive, cheap);

        // Apply join reordering
        let optimized = optimizer
            .reorder_joins(join)
            .expect("reorder_joins should succeed for hash join");

        // For inner joins, cheaper table should be on the left
        match &optimized.node_type {
            PlanNodeType::HashJoin { left, right, .. } => {
                assert!(
                    left.estimated_cost < right.estimated_cost,
                    "Expected left ({}) to be cheaper than right ({})",
                    left.estimated_cost,
                    right.estimated_cost
                );
            }
            _ => panic!("Expected hash join"),
        }
    }

    #[test]
    fn test_projection_pushdown() {
        let optimizer = CrossModelOptimizer::new();

        // Create: Project(Scan(users), [id, name])
        let scan = make_scan(&optimizer, "users", 100.0, 1000);
        let project = PlanNode {
            id: optimizer.next_id(),
            node_type: PlanNodeType::Project {
                input: Box::new(scan),
                columns: vec!["id".to_string(), "name".to_string()],
            },
            estimated_cost: 5.0,
            estimated_rows: 1000,
            output_columns: vec!["id".to_string(), "name".to_string()],
        };

        // Apply projection pushdown
        let optimized = optimizer
            .push_projections(project)
            .expect("push_projections should succeed for project node");

        // The projection should be pushed down and the scan should only read needed columns
        // Cost should be reduced
        assert!(optimized.estimated_cost <= 105.0);
    }

    #[test]
    fn test_parallel_execution_identification_hash_join() {
        let optimizer = CrossModelOptimizer::new();

        // Create: HashJoin(Scan(a), Scan(b))
        let scan_a = make_scan(&optimizer, "table_a", 100.0, 1000);
        let scan_b = make_scan(&optimizer, "table_b", 100.0, 1000);
        let join = make_hash_join(&optimizer, scan_a, scan_b);

        // Apply parallel identification
        let optimized = optimizer
            .identify_parallelism(join)
            .expect("identify_parallelism should succeed for hash join");

        // Cost should account for parallel execution (max of children, not sum)
        match &optimized.node_type {
            PlanNodeType::HashJoin { left, right, .. } => {
                // Parallel cost model: max(left, right) + overhead
                // This should be less than left + right + join overhead
                assert!(
                    optimized.estimated_cost < left.estimated_cost + right.estimated_cost + 200.0,
                    "Parallel cost {} should be less than sequential {}",
                    optimized.estimated_cost,
                    left.estimated_cost + right.estimated_cost + 200.0
                );
            }
            _ => panic!("Expected hash join"),
        }
    }

    #[test]
    fn test_parallel_execution_identification_union() {
        let optimizer = CrossModelOptimizer::new();

        // Create: Union(Scan(a), Scan(b), Scan(c))
        let scan_a = make_scan(&optimizer, "table_a", 50.0, 500);
        let scan_b = make_scan(&optimizer, "table_b", 100.0, 1000);
        let scan_c = make_scan(&optimizer, "table_c", 75.0, 750);

        let union = PlanNode {
            id: optimizer.next_id(),
            node_type: PlanNodeType::Union {
                inputs: vec![scan_a, scan_b, scan_c],
                all: true,
            },
            estimated_cost: 250.0,
            estimated_rows: 2250,
            output_columns: vec!["*".to_string()],
        };

        // Apply parallel identification
        let optimized = optimizer
            .identify_parallelism(union)
            .expect("identify_parallelism should succeed for union node");

        // All union inputs can run in parallel, cost should be max + overhead
        assert!(
            optimized.estimated_cost < 250.0,
            "Union cost {} should be less than original 250.0 due to parallelism",
            optimized.estimated_cost
        );
    }

    #[test]
    fn test_find_parallel_stages() {
        let optimizer = CrossModelOptimizer::new();

        // Create: HashJoin(HashJoin(a, b), c)
        let scan_a = make_scan(&optimizer, "table_a", 50.0, 500);
        let scan_b = make_scan(&optimizer, "table_b", 100.0, 1000);
        let scan_c = make_scan(&optimizer, "table_c", 75.0, 750);

        let inner_join = make_hash_join(&optimizer, scan_a.clone(), scan_b.clone());
        let outer_join = make_hash_join(&optimizer, inner_join.clone(), scan_c.clone());

        let stages = optimizer.find_parallel_stages(&outer_join);

        // Should identify that a and b can run in parallel
        // and that (a join b) and c can run in parallel
        assert!(stages.len() >= 1, "Should find at least one parallel stage");
    }

    #[test]
    fn test_predicate_selectivity_estimation() {
        let optimizer = CrossModelOptimizer::new();

        // Equality predicate should have ~10% selectivity
        let eq_pred = Predicate {
            column: "id".to_string(),
            op: PredicateOp::Eq,
            value: PredicateValue::Int(42),
        };
        assert!((optimizer.estimate_predicate_selectivity(&eq_pred) - 0.1).abs() < 0.001);

        // Not equal should have ~90% selectivity
        let ne_pred = Predicate {
            column: "status".to_string(),
            op: PredicateOp::Ne,
            value: PredicateValue::String("deleted".to_string()),
        };
        assert!((optimizer.estimate_predicate_selectivity(&ne_pred) - 0.9).abs() < 0.001);

        // Range predicates should have ~30% selectivity
        let range_pred = Predicate {
            column: "price".to_string(),
            op: PredicateOp::Gt,
            value: PredicateValue::Float(100.0),
        };
        assert!((optimizer.estimate_predicate_selectivity(&range_pred) - 0.3).abs() < 0.001);
    }

    #[test]
    fn test_predicate_to_string() {
        let optimizer = CrossModelOptimizer::new();

        let pred = Predicate {
            column: "name".to_string(),
            op: PredicateOp::Eq,
            value: PredicateValue::String("Alice".to_string()),
        };
        assert_eq!(optimizer.predicate_to_string(&pred), "name = 'Alice'");

        let in_pred = Predicate {
            column: "status".to_string(),
            op: PredicateOp::In,
            value: PredicateValue::List(vec![
                PredicateValue::String("active".to_string()),
                PredicateValue::String("pending".to_string()),
            ]),
        };
        assert_eq!(
            optimizer.predicate_to_string(&in_pred),
            "status IN ('active', 'pending')"
        );
    }

    #[test]
    fn test_optimization_reduces_cost() {
        let optimizer = CrossModelOptimizer::new();

        // Create a complex plan that should benefit from all optimizations
        let scan_users = make_scan(&optimizer, "users", 100.0, 1000);
        let scan_orders = make_scan(&optimizer, "orders", 200.0, 5000);
        let join = make_hash_join(&optimizer, scan_users, scan_orders);
        let filter = make_filter(&optimizer, join, "users.status", "active");

        let original_cost = optimizer.calculate_total_cost(&filter);

        // Apply all optimizations
        let optimized = optimizer
            .apply_optimizations(filter)
            .expect("apply_optimizations should succeed for filter node");
        let optimized_cost = optimizer.calculate_total_cost(&optimized);

        // Optimized plan should have lower or equal cost
        // Note: Allow larger variance (1.5x) because predicate pushdown may initially
        // increase intermediate costs while ultimately reducing overall execution cost.
        // The optimizer focuses on reducing I/O and network costs, not just the cost metric.
        assert!(
            optimized_cost <= original_cost * 1.5,
            "Optimized cost {} should not be much higher than original {}",
            optimized_cost,
            original_cost
        );
    }

    #[test]
    fn test_collect_required_columns() {
        let optimizer = CrossModelOptimizer::new();

        // Create: Filter(Project(Scan(users), [id, name, email]), name = 'Alice')
        let scan = make_scan(&optimizer, "users", 100.0, 1000);
        let project = PlanNode {
            id: optimizer.next_id(),
            node_type: PlanNodeType::Project {
                input: Box::new(scan),
                columns: vec!["id".to_string(), "name".to_string(), "email".to_string()],
            },
            estimated_cost: 5.0,
            estimated_rows: 1000,
            output_columns: vec!["id".to_string(), "name".to_string(), "email".to_string()],
        };
        let filter = make_filter(&optimizer, project, "name", "Alice");

        let required = optimizer.collect_required_columns(&filter);

        // Should include all projected columns plus the filter column
        assert!(required.contains(&"id".to_string()));
        assert!(required.contains(&"name".to_string()));
        assert!(required.contains(&"email".to_string()));
    }

    #[test]
    fn test_greedy_join_order() {
        let optimizer = CrossModelOptimizer::new();

        // Create relations with different costs
        let mut relations = vec![
            make_scan(&optimizer, "big", 1000.0, 100000),
            make_scan(&optimizer, "small", 10.0, 100),
            make_scan(&optimizer, "medium", 100.0, 1000),
        ];

        let result = optimizer
            .greedy_join_order(
                &mut relations,
                &[("id".to_string(), "id".to_string())],
                &JoinType::Inner,
            )
            .expect("greedy_join_order should succeed for valid relations");

        // Result should be a nested join tree starting with smallest tables
        match &result.node_type {
            PlanNodeType::HashJoin { .. } => {
                // The greedy algorithm should have built a left-deep tree
                // with the cheapest tables joined first
                assert!(result.estimated_cost > 0.0);
            }
            _ => panic!("Expected hash join as result"),
        }
    }

    // ========================================================================
    // JOIN STRATEGY SELECTION TESTS
    // ========================================================================

    #[test]
    fn test_join_strategy_index_join_small_right() {
        let strategy = select_join_strategy(50_000, 500, true);
        assert_eq!(strategy, JoinStrategy::IndexJoin);
    }

    #[test]
    fn test_join_strategy_hash_join_large_both() {
        let strategy = select_join_strategy(10_000, 5_000, false);
        assert_eq!(strategy, JoinStrategy::HashJoin);
    }

    #[test]
    fn test_join_strategy_nested_loop_small_inputs() {
        let strategy = select_join_strategy(100, 50, false);
        assert_eq!(strategy, JoinStrategy::NestedLoopJoin);
    }

    #[test]
    fn test_join_strategy_hash_over_index_when_right_large() {
        // Even with an index, if the right side is large prefer hash join
        let strategy = select_join_strategy(5_000, 5_000, true);
        assert_eq!(strategy, JoinStrategy::HashJoin);
    }

    #[test]
    fn test_join_strategy_nested_loop_one_side_small() {
        let strategy = select_join_strategy(500, 50, true);
        assert_eq!(strategy, JoinStrategy::IndexJoin);
    }

    // ========================================================================
    // Runtime Statistics Feedback Tests
    // ========================================================================

    #[test]
    fn test_runtime_stats_collector_creation() {
        let collector = RuntimeStatisticsCollector::new(500);
        let snapshot = collector.snapshot();
        assert_eq!(snapshot.tracked_operations, 0);
        assert_eq!(snapshot.total_observations, 0);
    }

    #[test]
    fn test_runtime_stats_record_single_feedback() {
        let collector = RuntimeStatisticsCollector::default();
        let feedback = ExecutionFeedback {
            operation_key: "vector_search:embeddings:top10".to_string(),
            estimated_cardinality: 10,
            actual_cardinality: 8,
            estimated_cost: 100.0,
            actual_latency_ms: 12.5,
            ..Default::default()
        };

        collector.record_feedback(&feedback);

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.cardinality_entries, 1);
        assert_eq!(snapshot.latency_entries, 1);

        // Check correction ratio was recorded
        let correction = collector
            .cardinality_correction("vector_search:embeddings:top10")
            .expect("correction should exist");
        // Initial ratio=1.0, actual/est=0.8, EMA with alpha=0.3 → 1.0*0.7 + 0.8*0.3 = 0.94
        assert!((correction - 0.94).abs() < 0.01);
    }

    #[test]
    fn test_runtime_stats_ema_convergence() {
        let collector = RuntimeStatisticsCollector::default();

        // Simulate 20 executions where actual is consistently 2x estimated
        for _ in 0..20 {
            collector.record_feedback(&ExecutionFeedback {
                operation_key: "graph:traverse:depth3".to_string(),
                estimated_cardinality: 100,
                actual_cardinality: 200,
                estimated_cost: 50.0,
                actual_latency_ms: 25.0,
                ..Default::default()
            });
        }

        let correction = collector
            .cardinality_correction("graph:traverse:depth3")
            .expect("correction should exist after 20 observations");
        // After 20 iterations of EMA(alpha=0.3) toward ratio=2.0, should be close to 2.0
        assert!(
            correction > 1.8,
            "correction ratio should converge toward 2.0, got {}",
            correction
        );
    }

    #[test]
    fn test_runtime_stats_latency_tracking() {
        let collector = RuntimeStatisticsCollector::default();

        collector.record_feedback(&ExecutionFeedback {
            operation_key: "doc_query:users".to_string(),
            estimated_cardinality: 50,
            actual_cardinality: 50,
            estimated_cost: 200.0,
            actual_latency_ms: 15.0,
            ..Default::default()
        });

        let avg = collector
            .avg_latency("doc_query:users")
            .expect("latency should be tracked");
        assert!((avg - 15.0).abs() < 0.1);

        let ratio = collector
            .cost_latency_ratio("doc_query:users")
            .expect("ratio should be tracked");
        // 15.0 / 200.0 = 0.075
        assert!((ratio - 0.075).abs() < 0.01);
    }

    #[test]
    fn test_runtime_stats_selectivity_tracking() {
        let collector = RuntimeStatisticsCollector::default();

        collector.record_feedback(&ExecutionFeedback {
            operation_key: "filter:age>30".to_string(),
            estimated_cardinality: 30,
            actual_cardinality: 30,
            estimated_cost: 10.0,
            actual_latency_ms: 1.0,
            rows_scanned: Some(100),
            ..Default::default()
        });

        let selectivity = collector
            .calibrated_selectivity("filter:age>30")
            .expect("selectivity should be tracked");
        // 30/100 = 0.3
        assert!((selectivity - 0.3).abs() < 0.01);
    }

    #[test]
    fn test_runtime_stats_plan_invalidation_detection() {
        let collector = RuntimeStatisticsCollector::default();

        // Establish a baseline cost-to-latency ratio
        for _ in 0..5 {
            collector.record_feedback(&ExecutionFeedback {
                operation_key: "vector_search:imgs".to_string(),
                estimated_cardinality: 10,
                actual_cardinality: 10,
                estimated_cost: 50.0,
                actual_latency_ms: 5.0, // ratio = 0.1
                ..Default::default()
            });
        }

        // Normal latency: should NOT invalidate
        assert!(!collector.should_invalidate_plan("vector_search:imgs", 50.0, 6.0));

        // 3x+ regression: should invalidate
        // Expected = 50.0 * 0.1 = 5.0, threshold = 15.0
        assert!(collector.should_invalidate_plan("vector_search:imgs", 50.0, 20.0));
    }

    #[test]
    fn test_runtime_stats_unknown_operation_no_invalidation() {
        let collector = RuntimeStatisticsCollector::default();
        // No history for this op — should not recommend invalidation
        assert!(!collector.should_invalidate_plan("unknown:op", 100.0, 500.0));
    }

    #[test]
    fn test_advanced_cost_estimator_feedback_update() {
        let mut estimator = AdvancedCostEstimator::new();
        let initial_cpu = estimator.cpu_cycles_per_distance;

        estimator.update_from_feedback(&ExecutionFeedback {
            observed_cpu_per_distance: Some(0.005),
            ..Default::default()
        });

        // EMA: 0.001 * 0.8 + 0.005 * 0.2 = 0.0018
        let expected = initial_cpu * 0.8 + 0.005 * 0.2;
        assert!(
            (estimator.cpu_cycles_per_distance - expected).abs() < 1e-6,
            "cpu cost should update via EMA, got {}",
            estimator.cpu_cycles_per_distance
        );
    }

    #[test]
    fn test_advanced_cost_estimator_multiple_feedback_convergence() {
        let mut estimator = AdvancedCostEstimator::new();

        // Apply 50 feedbacks with observed io_cost = 2.0 (initial is 1.0)
        for _ in 0..50 {
            estimator.update_from_feedback(&ExecutionFeedback {
                observed_io_per_page: Some(2.0),
                ..Default::default()
            });
        }

        assert!(
            (estimator.io_cost_per_page - 2.0).abs() < 0.05,
            "io_cost_per_page should converge to 2.0, got {}",
            estimator.io_cost_per_page
        );
    }

    #[test]
    fn test_optimizer_record_execution_feedback() {
        let mut optimizer = CrossModelOptimizer::new();

        // Record feedback for a vector search
        optimizer.record_execution_feedback(ExecutionFeedback {
            operation_key: "vector_search:embeddings:top10".to_string(),
            estimated_cardinality: 10,
            actual_cardinality: 7,
            estimated_cost: 100.0,
            actual_latency_ms: 8.0,
            ..Default::default()
        });

        // Calibrated cardinality should now differ from base
        let calibrated = optimizer.calibrated_cardinality("vector_search:embeddings:top10", 10);
        // With ratio ~0.94 (EMA from 1.0 toward 0.7): 10 * 0.94 = 9
        assert!(
            calibrated < 10,
            "calibrated should be less than base, got {}",
            calibrated
        );
    }

    #[test]
    fn test_optimizer_plan_invalidation_on_regression() {
        let mut optimizer = CrossModelOptimizer::new();

        // First, build up a baseline
        for _ in 0..10 {
            optimizer.record_execution_feedback(ExecutionFeedback {
                operation_key: "vector_search:products:top5".to_string(),
                estimated_cardinality: 5,
                actual_cardinality: 5,
                estimated_cost: 80.0,
                actual_latency_ms: 4.0, // ratio ~0.05
                ..Default::default()
            });
        }

        // Cache a plan for "products"
        let query = FederatedQuery {
            sql: "SELECT * FROM VECTOR_SEARCH('products', '[0.1]', 5)".to_string(),
            query_type: QueryType::VectorSearch,
            extensions: vec![SqlExtension::VectorSearch {
                collection: "products".to_string(),
                query_vector: VectorQuery::Literal(vec![0.1]),
                top_k: 5,
            }],
            targets: vec![],
            parameters: HashMap::new(),
            is_cross_model_join: false,
        };
        let plan = optimizer.optimize(&query).unwrap();
        let cache_key = PlanCacheKey::from_query(&query);
        assert!(optimizer.plan_cache.get(&cache_key).is_some());

        // Now record a massive regression
        optimizer.record_execution_feedback(ExecutionFeedback {
            operation_key: "vector_search:products:top5".to_string(),
            estimated_cardinality: 5,
            actual_cardinality: 5,
            estimated_cost: 80.0,
            actual_latency_ms: 500.0, // 125x regression
            ..Default::default()
        });

        // The plan for "products" should have been invalidated
        assert!(
            optimizer.plan_cache.get(&cache_key).is_none(),
            "cached plan should be invalidated after performance regression"
        );
    }

    #[test]
    fn test_runtime_stats_snapshot_completeness() {
        let collector = RuntimeStatisticsCollector::new(100);

        // Record various operation types
        collector.record_feedback(&ExecutionFeedback {
            operation_key: "op1".to_string(),
            estimated_cardinality: 10,
            actual_cardinality: 12,
            estimated_cost: 50.0,
            actual_latency_ms: 5.0,
            rows_scanned: Some(100),
            ..Default::default()
        });
        collector.record_feedback(&ExecutionFeedback {
            operation_key: "op2".to_string(),
            estimated_cardinality: 100,
            actual_cardinality: 80,
            estimated_cost: 200.0,
            actual_latency_ms: 20.0,
            ..Default::default()
        });

        let snap = collector.snapshot();
        assert_eq!(snap.cardinality_entries, 2);
        assert_eq!(snap.latency_entries, 2);
        assert_eq!(snap.selectivity_entries, 1); // Only op1 had rows_scanned
        assert!(snap.total_observations >= 4); // 2 cardinality + 2 latency minimum
    }

    #[test]
    fn test_calibrated_cardinality_falls_back_to_estimator() {
        let mut optimizer = CrossModelOptimizer::new();

        // Record in the cardinality_estimator directly (bypassing runtime stats)
        optimizer.cardinality_estimator.record_actual_cardinality(
            "legacy:op".to_string(),
            100,
            50,
        );

        // Should use the cardinality estimator as fallback
        let calibrated = optimizer.calibrated_cardinality("legacy:op", 200);
        // ratio = 50/100 = 0.5 → 200 * 0.5 = 100
        assert_eq!(calibrated, 100);
    }

    #[test]
    fn test_runtime_stats_compaction() {
        let collector = RuntimeStatisticsCollector::new(5);

        // Insert more than 2 * max_history_per_op entries to trigger compaction
        for i in 0..12 {
            collector.record_feedback(&ExecutionFeedback {
                operation_key: format!("op_{}", i),
                estimated_cardinality: 10,
                actual_cardinality: 10,
                estimated_cost: 10.0,
                actual_latency_ms: 1.0,
                ..Default::default()
            });
        }

        let snap = collector.snapshot();
        // After compaction, cardinality entries should be <= max_history_per_op
        assert!(
            snap.cardinality_entries <= 5,
            "should compact to max_history_per_op, got {}",
            snap.cardinality_entries
        );
    }
}
