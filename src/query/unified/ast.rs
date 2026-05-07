//! Multi-Model Query AST
//!
//! Defines the abstract syntax tree for cross-model queries that combine
//! vector search, document queries, and graph traversal.

use std::collections::HashMap;

use super::fusion::FusionStrategy;

/// A multi-model query that can span vectors, documents, and graphs
#[derive(Debug, Clone)]
pub struct MultiModelQuery {
    /// Query components (one per data model involved)
    pub components: Vec<QueryComponent>,
    /// How to combine results from different models
    pub fusion_strategy: FusionStrategy,
    /// Global limit on results
    pub limit: Option<u32>,
    /// Global offset for pagination
    pub offset: Option<u32>,
    /// Output projection (fields to return)
    pub projection: Vec<String>,
    /// Sort order for final results
    pub order_by: Option<OrderBy>,
}

impl MultiModelQuery {
    /// Create a new empty multi-model query
    pub fn new() -> Self {
        Self {
            components: Vec::new(),
            fusion_strategy: FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: Vec::new(),
            order_by: None,
        }
    }

    /// Add a vector search component
    pub fn with_vector_search(mut self, search: VectorSearchExpr) -> Self {
        self.components.push(QueryComponent {
            model: DataModel::Vector,
            operation: ModelOperation::VectorSearch(search),
            filters: Vec::new(),
            dependencies: Vec::new(),
        });
        self
    }

    /// Add a document query component
    pub fn with_document_query(mut self, query: DocumentQueryExpr) -> Self {
        self.components.push(QueryComponent {
            model: DataModel::Document,
            operation: ModelOperation::DocumentQuery(query),
            filters: Vec::new(),
            dependencies: Vec::new(),
        });
        self
    }

    /// Add a graph traversal component
    pub fn with_graph_traversal(mut self, traversal: GraphTraversalExpr) -> Self {
        self.components.push(QueryComponent {
            model: DataModel::Graph,
            operation: ModelOperation::GraphTraversal(traversal),
            filters: Vec::new(),
            dependencies: Vec::new(),
        });
        self
    }

    /// Add a declarative graph query component
    pub fn with_graph_query(mut self, query: GraphQueryExpr) -> Self {
        self.components.push(QueryComponent {
            model: DataModel::Graph,
            operation: ModelOperation::GraphQuery(query),
            filters: Vec::new(),
            dependencies: Vec::new(),
        });
        self
    }

    /// Set the fusion strategy
    pub fn with_fusion(mut self, strategy: FusionStrategy) -> Self {
        self.fusion_strategy = strategy;
        self
    }

    /// Set the limit
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }
}

impl Default for MultiModelQuery {
    fn default() -> Self {
        Self::new()
    }
}

/// A single query component targeting one data model
#[derive(Debug, Clone)]
pub struct QueryComponent {
    /// Target data model
    pub model: DataModel,
    /// The operation to perform
    pub operation: ModelOperation,
    /// Additional filters to apply
    pub filters: Vec<Filter>,
    /// Dependencies on other components (for joins)
    pub dependencies: Vec<ComponentDependency>,
}

impl QueryComponent {
    /// Check if this component can run in parallel with others
    pub fn is_parallelizable(&self) -> bool {
        self.dependencies.is_empty()
    }

    /// Get the collection/namespace this component targets
    pub fn target_collection(&self) -> Option<&str> {
        match &self.operation {
            ModelOperation::VectorSearch(v) => Some(&v.collection),
            ModelOperation::DocumentQuery(d) => Some(&d.collection),
            ModelOperation::GraphQuery(g) => Some(&g.graph_name),
            ModelOperation::GraphTraversal(g) => Some(&g.graph_name),
            ModelOperation::LogQuery(l) => Some(&l.namespace),
            ModelOperation::MetricQuery(m) => Some(&m.namespace),
        }
    }

    /// Alias for target_collection() for distributed query planning
    pub fn collection_name(&self) -> Option<String> {
        self.target_collection().map(String::from)
    }
}

/// Dependency between query components
#[derive(Debug, Clone)]
pub struct ComponentDependency {
    /// Index of the dependent component
    pub component_index: usize,
    /// Field to join on from dependent component
    pub join_field: String,
    /// Join type
    pub join_type: JoinType,
}

/// Join types for cross-model queries
#[derive(Debug, Clone, PartialEq)]
pub enum JoinType {
    /// Inner join - only matching records
    Inner,
    /// Left outer join - all from left, matching from right
    LeftOuter,
    /// Semi join - exists check
    Semi,
    /// Anti join - not exists check
    Anti,
    /// Semantic join - matches based on a [`SemanticJoinMode`].
    ///
    /// The `mode` field selects between cheap cosine-similarity matching
    /// (the default, always available) and LLM-driven block-batched
    /// matching (arXiv:2510.08489 Trummer 2025, gated behind the
    /// `llm-joins` feature). See [`SemanticJoinMode`] for details.
    Semantic {
        /// Similarity threshold for the [`SemanticJoinMode::Cosine`] mode
        /// (0.0 to 1.0, higher is stricter). Currently unused by
        /// [`SemanticJoinMode::LlmBlockBatch`] which uses LLM judgement
        /// rather than a numeric threshold.
        threshold: f32,
        /// Maximum number of right-side matches per left record.
        top_k: u32,
        /// Matching strategy. Defaults to [`SemanticJoinMode::Cosine`]
        /// for backward compatibility with callers constructed before
        /// TD-049 (the field is `#[serde(default)]` so JSON omitting
        /// it is accepted unchanged).
        mode: SemanticJoinMode,
    },
}

/// Strategies for evaluating a [`JoinType::Semantic`] join.
///
/// `Cosine` is the simple, dependency-free path: extract a vector
/// from each row, compute cosine similarity, accept matches above
/// `threshold`. Always available regardless of feature flags.
///
/// `LlmBlockBatch` is the paper-faithful path
/// (arXiv:2510.08489 Trummer 2025, Cornell): pack batches of rows
/// from *both* sides into a single LLM prompt and ask the model to
/// identify all matching pairs in one shot. Includes formulas for
/// optimal batch size given the LLM context window. Significant
/// cost reduction vs. per-pair LLM calls. Gated behind the
/// `llm-joins` Cargo feature so production builds without an LLM
/// integration produce clear errors at dispatch time rather than
/// silent fallbacks.
#[derive(Debug, Clone, PartialEq)]
pub enum SemanticJoinMode {
    /// Cosine similarity over an extracted vector field. Cheapest
    /// option, always available. The default.
    Cosine,
    /// LLM-driven block-batched matching (TD-049, arXiv:2510.08489).
    LlmBlockBatch(BlockBatchConfig),
}

impl Default for SemanticJoinMode {
    fn default() -> Self {
        Self::Cosine
    }
}

/// Configuration for [`SemanticJoinMode::LlmBlockBatch`].
///
/// Batch sizes follow the paper's optimal-batching formula given the
/// LLM context window. Defaults are conservative — tune for your
/// model's context size and the row width of your tables.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockBatchConfig {
    /// Number of left-side rows packed into each prompt.
    pub batch_size_left: u32,
    /// Number of right-side rows packed into each prompt.
    pub batch_size_right: u32,
    /// Hard cap on LLM calls per query. Once exceeded, the join
    /// returns whatever it has matched so far. Mandatory because
    /// LLM costs blow up fast on accidental Cartesian products.
    pub max_calls: u32,
}

impl Default for BlockBatchConfig {
    fn default() -> Self {
        // Conservative defaults tuned for an 8K-token context model
        // and ~256-token row width. Halve `batch_size_*` for 4K
        // models, double for 32K+. `max_calls` is intentionally
        // small so a misconfigured query fails fast and visibly.
        Self {
            batch_size_left: 16,
            batch_size_right: 16,
            max_calls: 64,
        }
    }
}

impl BlockBatchConfig {
    /// Validate the config. All sizes must be > 0 -- a zero batch
    /// size is unrecoverable (no work would happen) and a zero
    /// `max_calls` would prevent any LLM dispatch.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.batch_size_left == 0 {
            return Err("batch_size_left must be > 0");
        }
        if self.batch_size_right == 0 {
            return Err("batch_size_right must be > 0");
        }
        if self.max_calls == 0 {
            return Err("max_calls must be > 0");
        }
        Ok(())
    }
}

/// Data models supported by ProximaDB.
///
/// Re-exported from `crate::query::multimodel_router::StoreType` -- the single
/// source of truth for all data model type discrimination across the codebase.
/// The `DataModel` alias is maintained for backward compatibility with existing
/// code that imports `DataModel` from this module.
pub type DataModel = crate::query::multimodel_router::StoreType;

/// Operations that can be performed on each data model
#[derive(Debug, Clone)]
pub enum ModelOperation {
    /// Vector similarity search
    VectorSearch(VectorSearchExpr),
    /// Document query with JSON path filters
    DocumentQuery(DocumentQueryExpr),
    /// Declarative graph query lowered from the supported graph subset
    GraphQuery(GraphQueryExpr),
    /// Graph traversal
    GraphTraversal(GraphTraversalExpr),
    /// Log query
    LogQuery(LogQueryExpr),
    /// Metric query
    MetricQuery(MetricQueryExpr),
}

/// Vector search expression
#[derive(Debug, Clone)]
pub struct VectorSearchExpr {
    /// Collection to search
    pub collection: String,
    /// Query vector
    pub query_vector: Vec<f32>,
    /// Number of results to return
    pub top_k: u32,
    /// Similarity threshold (0.0 to 1.0)
    pub threshold: Option<f32>,
    /// Distance metric
    pub metric: DistanceMetric,
    /// Search parameters
    pub params: VectorSearchParams,
}

/// Vector search parameters
#[derive(Debug, Clone, Default)]
pub struct VectorSearchParams {
    /// Search mode (exact, approximate, adaptive)
    pub mode: Option<String>,
    /// EF search parameter for HNSW
    pub ef_search: Option<u32>,
    /// Number of probes for IVF
    pub n_probes: Option<u32>,
}

/// Distance metrics for vector search
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DistanceMetric {
    /// Euclidean distance (L2)
    Euclidean,
    /// Cosine similarity
    #[default]
    Cosine,
    /// Dot product
    DotProduct,
    /// Manhattan distance (L1)
    Manhattan,
}

/// Document query expression
#[derive(Debug, Clone)]
pub struct DocumentQueryExpr {
    /// Collection to query
    pub collection: String,
    /// JSON path filters
    pub path_filters: Vec<PathFilter>,
    /// Full-text search query
    pub text_search: Option<String>,
    /// Projection (fields to return)
    pub projection: Vec<String>,
    /// Sort order
    pub sort: Option<DocumentSort>,
    /// Limit
    pub limit: Option<u32>,
}

/// JSON path filter
#[derive(Debug, Clone)]
pub struct PathFilter {
    /// JSON path (e.g., "$.user.name")
    pub path: String,
    /// Comparison operator
    pub operator: FilterOperator,
    /// Value to compare against
    pub value: FilterValue,
}

/// Filter operators
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterOperator {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
    NotIn,
    Contains,
    StartsWith,
    EndsWith,
    Exists,
    Type,
}

/// Filter values
#[derive(Debug, Clone)]
pub enum FilterValue {
    String(String),
    Number(f64),
    Bool(bool),
    Null,
    Array(Vec<FilterValue>),
}

/// Document sort specification
#[derive(Debug, Clone)]
pub struct DocumentSort {
    /// Path to sort by
    pub path: String,
    /// Ascending or descending
    pub ascending: bool,
}

/// Declarative graph query expression backed by the shared graph subset
#[derive(Debug, Clone)]
pub struct GraphQueryExpr {
    /// Graph name
    pub graph_name: String,
    /// Normalized query text without an inline FROM clause
    pub normalized_query: String,
    /// Projected output columns produced by the shared graph subset
    pub output_columns: Vec<String>,
    /// Whether whole-node projections should be materialized into legacy
    /// `node_id` / `label` / `properties` row shape
    pub uses_legacy_node_rows: bool,
    /// Maximum traversal depth implied by the query pattern
    pub max_depth: u32,
}

/// Graph traversal expression
#[derive(Debug, Clone)]
pub struct GraphTraversalExpr {
    /// Graph name
    pub graph_name: String,
    /// Start node(s)
    pub start_nodes: StartNodeSpec,
    /// Edge type(s) to traverse
    pub edge_types: Vec<String>,
    /// Traversal direction
    pub direction: TraversalDirection,
    /// Maximum depth
    pub max_depth: u32,
    /// Minimum depth
    pub min_depth: u32,
    /// Node filters
    pub node_filters: Vec<NodeFilter>,
    /// Edge filters
    pub edge_filters: Vec<EdgeFilter>,
    /// Return paths or just nodes
    pub return_paths: bool,
}

/// Start node specification
#[derive(Debug, Clone)]
pub enum StartNodeSpec {
    /// Specific node IDs
    Ids(Vec<String>),
    /// Nodes matching a label
    Label(String),
    /// Nodes matching a filter
    Filter(NodeFilter),
    /// From another query component
    FromComponent(usize),
}

/// Traversal direction
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TraversalDirection {
    #[default]
    Outgoing,
    Incoming,
    Both,
}

/// Node filter for graph queries
#[derive(Debug, Clone)]
pub struct NodeFilter {
    /// Label to match (optional)
    pub label: Option<String>,
    /// Property filters
    pub properties: Vec<PropertyFilter>,
}

/// Edge filter for graph queries
#[derive(Debug, Clone)]
pub struct EdgeFilter {
    /// Edge type to match (optional)
    pub edge_type: Option<String>,
    /// Property filters
    pub properties: Vec<PropertyFilter>,
    /// Weight range (min, max)
    pub weight_range: Option<(f64, f64)>,
}

/// Property filter
#[derive(Debug, Clone)]
pub struct PropertyFilter {
    /// Property name
    pub name: String,
    /// Operator
    pub operator: FilterOperator,
    /// Value
    pub value: FilterValue,
}

/// Log query expression
#[derive(Debug, Clone)]
pub struct LogQueryExpr {
    /// Namespace to query
    pub namespace: String,
    /// Start time (nanoseconds)
    pub start_time_ns: i64,
    /// End time (nanoseconds)
    pub end_time_ns: i64,
    /// Search query
    pub query: Option<String>,
    /// Severity filter
    pub severities: Vec<String>,
    /// Service filter
    pub services: Vec<String>,
    /// Limit
    pub limit: u32,
}

/// Metric query expression
#[derive(Debug, Clone)]
pub struct MetricQueryExpr {
    /// Namespace to query
    pub namespace: String,
    /// Metric name
    pub metric_name: String,
    /// Start time (nanoseconds)
    pub start_time_ns: i64,
    /// End time (nanoseconds)
    pub end_time_ns: i64,
    /// Aggregation function
    pub aggregation: MetricAggregation,
    /// Group by labels
    pub group_by: Vec<String>,
    /// Label filters
    pub label_filters: HashMap<String, String>,
}

/// Metric aggregation functions
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MetricAggregation {
    Sum,
    #[default]
    Avg,
    Min,
    Max,
    Count,
    P50,
    P90,
    P95,
    P99,
    Rate,
}

/// Generic filter that can apply to any record
#[derive(Debug, Clone)]
pub struct Filter {
    /// Field path
    pub field: String,
    /// Operator
    pub operator: FilterOperator,
    /// Value
    pub value: FilterValue,
}

/// Order by specification
#[derive(Debug, Clone)]
pub struct OrderBy {
    /// Field to order by
    pub field: String,
    /// Ascending order
    pub ascending: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_vector_query() {
        let query = MultiModelQuery::new()
            .with_vector_search(VectorSearchExpr {
                collection: "embeddings".to_string(),
                query_vector: vec![0.1, 0.2, 0.3],
                top_k: 10,
                threshold: Some(0.8),
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            })
            .with_limit(10);

        assert_eq!(query.components.len(), 1);
        assert_eq!(query.limit, Some(10));
    }

    #[test]
    fn test_build_hybrid_query() {
        let query = MultiModelQuery::new()
            .with_document_query(DocumentQueryExpr {
                collection: "products".to_string(),
                path_filters: vec![PathFilter {
                    path: "$.category".to_string(),
                    operator: FilterOperator::Eq,
                    value: FilterValue::String("electronics".to_string()),
                }],
                text_search: None,
                projection: vec!["id".to_string(), "name".to_string()],
                sort: None,
                limit: Some(100),
            })
            .with_vector_search(VectorSearchExpr {
                collection: "products".to_string(),
                query_vector: vec![0.1; 128],
                top_k: 50,
                threshold: Some(0.7),
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            })
            .with_fusion(FusionStrategy::Intersection);

        assert_eq!(query.components.len(), 2);
        assert!(matches!(
            query.fusion_strategy,
            FusionStrategy::Intersection
        ));
    }

    #[test]
    fn test_component_parallelizable() {
        let component = QueryComponent {
            model: DataModel::Vector,
            operation: ModelOperation::VectorSearch(VectorSearchExpr {
                collection: "test".to_string(),
                query_vector: vec![0.1],
                top_k: 10,
                threshold: None,
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            }),
            filters: vec![],
            dependencies: vec![],
        };

        assert!(component.is_parallelizable());

        let dependent_component = QueryComponent {
            model: DataModel::Graph,
            operation: ModelOperation::GraphTraversal(GraphTraversalExpr {
                graph_name: "test".to_string(),
                start_nodes: StartNodeSpec::FromComponent(0),
                edge_types: vec!["RELATED".to_string()],
                direction: TraversalDirection::Outgoing,
                max_depth: 2,
                min_depth: 1,
                node_filters: vec![],
                edge_filters: vec![],
                return_paths: false,
            }),
            filters: vec![],
            dependencies: vec![ComponentDependency {
                component_index: 0,
                join_field: "id".to_string(),
                join_type: JoinType::Inner,
            }],
        };

        assert!(!dependent_component.is_parallelizable());
    }

    // ==========================================================
    // TD-049 SemanticJoinMode + BlockBatchConfig
    //
    // Pin the contracts that other code paths rely on:
    //   - Default mode is Cosine (backward compat).
    //   - JoinType::Semantic carries `mode` and survives PartialEq.
    //   - BlockBatchConfig::default() yields valid config.
    //   - validate() rejects every zero-sized field with a clear msg.
    // ==========================================================

    #[test]
    fn semantic_join_mode_defaults_to_cosine() {
        // Critical for backward compat: callers constructing
        // JoinType::Semantic without specifying mode must get
        // cosine semantics.
        let mode: SemanticJoinMode = Default::default();
        assert_eq!(mode, SemanticJoinMode::Cosine);
    }

    #[test]
    fn semantic_join_carries_mode_and_compares_by_value() {
        let a = JoinType::Semantic {
            threshold: 0.8,
            top_k: 5,
            mode: SemanticJoinMode::Cosine,
        };
        let b = JoinType::Semantic {
            threshold: 0.8,
            top_k: 5,
            mode: SemanticJoinMode::Cosine,
        };
        assert_eq!(a, b, "same fields including mode -> structurally equal");

        let c = JoinType::Semantic {
            threshold: 0.8,
            top_k: 5,
            mode: SemanticJoinMode::LlmBlockBatch(BlockBatchConfig::default()),
        };
        assert_ne!(
            a, c,
            "different mode -> not equal even with same threshold/top_k"
        );
    }

    #[test]
    fn block_batch_config_default_is_valid() {
        // Our shipped defaults must pass validation -- a zero-sized
        // default would be a footgun for anyone constructing the
        // mode without overriding fields.
        BlockBatchConfig::default()
            .validate()
            .expect("default must validate");
    }

    #[test]
    fn block_batch_config_default_values_are_conservative() {
        let cfg = BlockBatchConfig::default();
        // These are the documented conservative defaults; a change
        // here is API-visible and warrants reviewing the rustdoc.
        assert_eq!(cfg.batch_size_left, 16);
        assert_eq!(cfg.batch_size_right, 16);
        assert_eq!(cfg.max_calls, 64);
    }

    #[test]
    fn block_batch_config_rejects_zero_left_batch() {
        let mut cfg = BlockBatchConfig::default();
        cfg.batch_size_left = 0;
        match cfg.validate() {
            Err(msg) => assert!(msg.contains("batch_size_left")),
            Ok(()) => panic!("zero left batch must be rejected"),
        }
    }

    #[test]
    fn block_batch_config_rejects_zero_right_batch() {
        let mut cfg = BlockBatchConfig::default();
        cfg.batch_size_right = 0;
        match cfg.validate() {
            Err(msg) => assert!(msg.contains("batch_size_right")),
            Ok(()) => panic!("zero right batch must be rejected"),
        }
    }

    #[test]
    fn block_batch_config_rejects_zero_max_calls() {
        let mut cfg = BlockBatchConfig::default();
        cfg.max_calls = 0;
        match cfg.validate() {
            Err(msg) => assert!(msg.contains("max_calls")),
            Ok(()) => panic!("zero max_calls must be rejected"),
        }
    }

    #[test]
    fn block_batch_config_validates_first_failure() {
        // When multiple fields are zero, validate returns ONE error
        // at a time (the first encountered) so callers can fix
        // problems sequentially. Pin the order: left -> right -> max.
        let cfg = BlockBatchConfig {
            batch_size_left: 0,
            batch_size_right: 0,
            max_calls: 0,
        };
        match cfg.validate() {
            Err(msg) => assert!(
                msg.contains("batch_size_left"),
                "first error should be batch_size_left, got: {}",
                msg
            ),
            Ok(()) => panic!("all-zero must be rejected"),
        }
    }
}
