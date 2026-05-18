use proximadb_document_query::DocumentQueryExpr;
use proximadb_graph_query::declarative::LoweredGraphQuery as GraphQueryExpr;
use proximadb_graph_query::traversal::GraphTraversalExpr;
use proximadb_observability_query::{LogQueryExpr, MetricQueryExpr};
use proximadb_query_clauses::{Filter, OrderBy};
use proximadb_query_fusion::FusionStrategy;
use proximadb_vector_query::VectorSearchExpr;

/// Data models supported by ProximaDB.
pub type DataModel = proximadb_data_model::DataModel;

/// A multi-model query that can span vectors, documents, and graphs.
#[derive(Debug, Clone)]
pub struct MultiModelQuery {
    /// Query components (one per data model involved).
    pub components: Vec<QueryComponent>,
    /// How to combine results from different models.
    pub fusion_strategy: FusionStrategy,
    /// Global limit on results.
    pub limit: Option<u32>,
    /// Global offset for pagination.
    pub offset: Option<u32>,
    /// Output projection (fields to return).
    pub projection: Vec<String>,
    /// Sort order for final results.
    pub order_by: Option<OrderBy>,
}

impl MultiModelQuery {
    /// Create a new empty multi-model query.
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

    /// Add a vector search component.
    pub fn with_vector_search(mut self, search: VectorSearchExpr) -> Self {
        self.components.push(QueryComponent {
            model: DataModel::Vector,
            operation: ModelOperation::VectorSearch(search),
            filters: Vec::new(),
            dependencies: Vec::new(),
        });
        self
    }

    /// Add a document query component.
    pub fn with_document_query(mut self, query: DocumentQueryExpr) -> Self {
        self.components.push(QueryComponent {
            model: DataModel::Document,
            operation: ModelOperation::DocumentQuery(query),
            filters: Vec::new(),
            dependencies: Vec::new(),
        });
        self
    }

    /// Add a graph traversal component.
    pub fn with_graph_traversal(mut self, traversal: GraphTraversalExpr) -> Self {
        self.components.push(QueryComponent {
            model: DataModel::Graph,
            operation: ModelOperation::GraphTraversal(traversal),
            filters: Vec::new(),
            dependencies: Vec::new(),
        });
        self
    }

    /// Add a declarative graph query component.
    pub fn with_graph_query(mut self, query: GraphQueryExpr) -> Self {
        self.components.push(QueryComponent {
            model: DataModel::Graph,
            operation: ModelOperation::GraphQuery(query),
            filters: Vec::new(),
            dependencies: Vec::new(),
        });
        self
    }

    /// Set the fusion strategy.
    pub fn with_fusion(mut self, strategy: FusionStrategy) -> Self {
        self.fusion_strategy = strategy;
        self
    }

    /// Set the limit.
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Validate query-level structure before planning or execution.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.components.is_empty() {
            return Err("query must contain at least one component");
        }
        if matches!(self.limit, Some(0)) {
            return Err("query limit must be > 0");
        }
        self.fusion_strategy.validate()?;

        for (component_index, component) in self.components.iter().enumerate() {
            component.validate(component_index)?;

            for dependency in &component.dependencies {
                dependency.validate(component_index)?;
                if dependency.component_index >= self.components.len() {
                    return Err("component dependency index out of range");
                }
                if dependency.component_index == component_index {
                    return Err("component cannot depend on itself");
                }
            }
        }

        Ok(())
    }
}

impl Default for MultiModelQuery {
    fn default() -> Self {
        Self::new()
    }
}

/// A single query component targeting one data model.
#[derive(Debug, Clone)]
pub struct QueryComponent {
    /// Target data model.
    pub model: DataModel,
    /// The operation to perform.
    pub operation: ModelOperation,
    /// Additional filters to apply.
    pub filters: Vec<Filter>,
    /// Dependencies on other components (for joins).
    pub dependencies: Vec<ComponentDependency>,
}

/// Named agentic view produced by natural-language or AQL decomposition.
///
/// Agentic views are logical-plan fragments over the shared multi-model IR; they
/// do not own storage, catalog state, or a separate type system. Persisted views
/// must be registered through xCatalog and replay into this shape.
#[derive(Debug, Clone)]
pub struct AgenticView {
    /// Stable view name within a query decomposition.
    pub name: String,
    /// Human-readable objective for explain plans and agent traces.
    pub objective: String,
    /// Multi-model subplan executed to materialize this view.
    pub query: MultiModelQuery,
    /// Other view names that must be available before this view can run.
    pub depends_on: Vec<String>,
    /// Materialization policy for this logical view.
    pub materialization: ViewMaterialization,
}

impl AgenticView {
    /// Create a logical view over a shared multi-model query.
    pub fn new(
        name: impl Into<String>,
        objective: impl Into<String>,
        query: MultiModelQuery,
    ) -> Self {
        Self {
            name: name.into(),
            objective: objective.into(),
            query,
            depends_on: Vec::new(),
            materialization: ViewMaterialization::Ephemeral,
        }
    }

    /// Declare a dependency on another decomposed view.
    pub fn depends_on(mut self, view_name: impl Into<String>) -> Self {
        self.depends_on.push(view_name.into());
        self
    }

    /// Mark this view as eligible for xCatalog-managed rebuildable materialization.
    pub fn materializable(mut self, projection_name: impl Into<String>) -> Self {
        self.materialization = ViewMaterialization::RebuildableProjection {
            projection_name: projection_name.into(),
            freshness: ProjectionFreshness::default(),
        };
        self
    }

    /// Attach freshness metadata to a rebuildable materialized view.
    pub fn with_freshness(mut self, freshness: ProjectionFreshness) -> Self {
        if let ViewMaterialization::RebuildableProjection {
            freshness: current, ..
        } = &mut self.materialization
        {
            *current = freshness;
        }
        self
    }
}

/// Materialization mode for an agentic view.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewMaterialization {
    /// Per-query logical view only.
    Ephemeral,
    /// Rebuildable xCatalog projection. This is never a separate durable truth.
    RebuildableProjection {
        /// xCatalog projection name.
        projection_name: String,
        /// Freshness/rebuild contract.
        freshness: ProjectionFreshness,
    },
}

impl ViewMaterialization {
    /// True when the view may be persisted as a rebuildable projection.
    pub fn is_materializable(&self) -> bool {
        matches!(self, Self::RebuildableProjection { .. })
    }
}

/// Freshness contract for a rebuildable projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionFreshness {
    /// Maximum acceptable lag behind canonical records.
    pub max_lag_ms: u64,
    /// Canonical repair source, usually a record table/collection name.
    pub repair_source: String,
}

impl Default for ProjectionFreshness {
    fn default() -> Self {
        Self {
            max_lag_ms: 0,
            repair_source: String::new(),
        }
    }
}

impl ProjectionFreshness {
    /// Validate freshness metadata before a view is cataloged.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.repair_source.is_empty() {
            return Err("projection repair_source must not be empty");
        }
        Ok(())
    }
}

/// Ordered decomposition of an agentic query into inspectable logical views.
#[derive(Debug, Clone, Default)]
pub struct AgenticViewPlan {
    /// Original user/query text, when available.
    pub source_query: Option<String>,
    /// Ordered view definitions.
    pub views: Vec<AgenticView>,
    /// Final view returned to the caller.
    pub output_view: Option<String>,
}

impl AgenticViewPlan {
    /// Create an empty view plan.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach original query text for explainability.
    pub fn with_source_query(mut self, source_query: impl Into<String>) -> Self {
        self.source_query = Some(source_query.into());
        self
    }

    /// Add a decomposed view.
    pub fn add_view(mut self, view: AgenticView) -> Self {
        self.views.push(view);
        self
    }

    /// Set the final output view.
    pub fn with_output_view(mut self, view_name: impl Into<String>) -> Self {
        self.output_view = Some(view_name.into());
        self
    }

    /// Validate ordering and output references.
    pub fn validate(&self) -> Result<(), &'static str> {
        let mut seen = std::collections::HashSet::new();

        for view in &self.views {
            if view.name.is_empty() {
                return Err("view name must not be empty");
            }
            if view.objective.is_empty() {
                return Err("view objective must not be empty");
            }
            if view.query.components.is_empty() {
                return Err("view query must contain at least one component");
            }
            view.query.validate()?;
            if !seen.insert(view.name.as_str()) {
                return Err("view names must be unique");
            }
            if view
                .depends_on
                .iter()
                .any(|dependency| !seen.contains(dependency.as_str()))
            {
                return Err("view dependencies must reference earlier views");
            }
            if let ViewMaterialization::RebuildableProjection {
                projection_name,
                freshness,
            } = &view.materialization
            {
                if projection_name.is_empty() {
                    return Err("projection_name must not be empty");
                }
                freshness.validate()?;
            }
        }

        if let Some(output_view) = &self.output_view
            && !seen.contains(output_view.as_str())
        {
            return Err("output view must reference a declared view");
        }

        Ok(())
    }
}

impl QueryComponent {
    /// Validate component-local fields.
    pub fn validate(&self, component_index: usize) -> Result<(), &'static str> {
        if self.target_collection().is_none_or(str::is_empty) {
            return Err("component target collection must not be empty");
        }

        match &self.operation {
            ModelOperation::VectorSearch(search) => {
                if search.query_vector.is_empty() {
                    return Err("vector query_vector must not be empty");
                }
                if search.top_k == 0 {
                    return Err("vector top_k must be > 0");
                }
            }
            ModelOperation::GraphTraversal(traversal) => {
                if traversal.max_depth == 0 {
                    return Err("graph traversal max_depth must be > 0");
                }
                if traversal.min_depth > traversal.max_depth {
                    return Err("graph traversal min_depth must be <= max_depth");
                }
            }
            ModelOperation::DocumentQuery(_)
            | ModelOperation::GraphQuery(_)
            | ModelOperation::LogQuery(_)
            | ModelOperation::MetricQuery(_) => {}
        }

        for dependency in &self.dependencies {
            dependency.validate(component_index)?;
        }

        Ok(())
    }

    /// Check if this component can run in parallel with others.
    pub fn is_parallelizable(&self) -> bool {
        self.dependencies.is_empty()
    }

    /// Get the collection or namespace this component targets.
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

    /// Alias for `target_collection()` for distributed query planning.
    pub fn collection_name(&self) -> Option<String> {
        self.target_collection().map(String::from)
    }
}

/// Dependency between query components.
#[derive(Debug, Clone)]
pub struct ComponentDependency {
    /// Index of the dependent component.
    pub component_index: usize,
    /// Field to join on from dependent component.
    pub join_field: String,
    /// Join type.
    pub join_type: JoinType,
}

impl ComponentDependency {
    /// Validate dependency-local fields.
    pub fn validate(&self, component_index: usize) -> Result<(), &'static str> {
        if self.component_index == component_index {
            return Err("component cannot depend on itself");
        }
        if self.join_field.is_empty() {
            return Err("dependency join_field must not be empty");
        }
        self.join_type.validate()
    }
}

/// Join types for cross-model queries.
#[derive(Debug, Clone, PartialEq)]
pub enum JoinType {
    /// Inner join - only matching records.
    Inner,
    /// Left outer join - all from left, matching from right.
    LeftOuter,
    /// Semi join - exists check.
    Semi,
    /// Anti join - not exists check.
    Anti,
    /// Semantic join - matches based on a [`SemanticJoinMode`].
    Semantic {
        /// Similarity threshold for the [`SemanticJoinMode::Cosine`] mode.
        threshold: f32,
        /// Maximum number of right-side matches per left record.
        top_k: u32,
        /// Matching strategy.
        mode: SemanticJoinMode,
    },
}

impl JoinType {
    /// Validate join configuration.
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Self::Semantic {
            threshold,
            top_k,
            mode,
        } = self
        {
            if !threshold.is_finite() || !(0.0..=1.0).contains(threshold) {
                return Err("semantic join threshold must be between 0 and 1");
            }
            if *top_k == 0 {
                return Err("semantic join top_k must be > 0");
            }
            if let SemanticJoinMode::LlmBlockBatch(config) = mode {
                config.validate()?;
            }
        }
        Ok(())
    }
}

/// Strategies for evaluating a [`JoinType::Semantic`] join.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum SemanticJoinMode {
    /// Cosine similarity over an extracted vector field.
    #[default]
    Cosine,
    /// LLM-driven block-batched matching.
    LlmBlockBatch(BlockBatchConfig),
}

/// Configuration for [`SemanticJoinMode::LlmBlockBatch`].
#[derive(Debug, Clone, PartialEq)]
pub struct BlockBatchConfig {
    /// Number of left-side rows packed into each prompt.
    pub batch_size_left: u32,
    /// Number of right-side rows packed into each prompt.
    pub batch_size_right: u32,
    /// Hard cap on LLM calls per query.
    pub max_calls: u32,
}

impl Default for BlockBatchConfig {
    fn default() -> Self {
        Self {
            batch_size_left: 16,
            batch_size_right: 16,
            max_calls: 64,
        }
    }
}

impl BlockBatchConfig {
    /// Validate the config. All sizes must be greater than zero.
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

/// Operations that can be performed on each data model.
#[derive(Debug, Clone)]
pub enum ModelOperation {
    /// Vector similarity search.
    VectorSearch(VectorSearchExpr),
    /// Document query with JSON path filters.
    DocumentQuery(DocumentQueryExpr),
    /// Declarative graph query lowered from the supported graph subset.
    GraphQuery(GraphQueryExpr),
    /// Graph traversal.
    GraphTraversal(GraphTraversalExpr),
    /// Log query.
    LogQuery(LogQueryExpr),
    /// Metric query.
    MetricQuery(MetricQueryExpr),
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_graph_query::traversal::{
        EdgeFilter, GraphTraversalExpr, NodeFilter, PropertyFilter, StartNodeSpec,
        TraversalDirection,
    };
    use proximadb_query_filter::{FilterOperator, FilterValue};
    use proximadb_vector_query::{DistanceMetric, VectorSearchParams};

    #[test]
    fn build_vector_query_sets_component_and_limit() {
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
        assert!(query.validate().is_ok());
    }

    #[test]
    fn build_hybrid_query_keeps_requested_fusion() {
        let query = MultiModelQuery::new()
            .with_document_query(DocumentQueryExpr {
                collection: "products".to_string(),
                path_filters: vec![proximadb_document_query::PathFilter {
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
        assert!(query.validate().is_ok());
    }

    #[test]
    fn query_validation_rejects_empty_query_and_zero_limit() {
        assert_eq!(
            MultiModelQuery::new().validate(),
            Err("query must contain at least one component")
        );

        let query = MultiModelQuery::new()
            .with_vector_search(VectorSearchExpr {
                collection: "docs".to_string(),
                query_vector: vec![0.1],
                top_k: 1,
                threshold: None,
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            })
            .with_limit(0);

        assert_eq!(query.validate(), Err("query limit must be > 0"));
    }

    #[test]
    fn query_validation_rejects_bad_dependencies() {
        let component = QueryComponent {
            model: DataModel::Graph,
            operation: ModelOperation::GraphTraversal(GraphTraversalExpr {
                graph_name: "knowledge".to_string(),
                start_nodes: StartNodeSpec::FromComponent(0),
                edge_types: vec!["RELATED".to_string()],
                direction: TraversalDirection::Outgoing,
                max_depth: 2,
                min_depth: 1,
                node_filters: Vec::<NodeFilter>::new(),
                edge_filters: Vec::<EdgeFilter>::new(),
                return_paths: false,
            }),
            filters: vec![],
            dependencies: vec![ComponentDependency {
                component_index: 0,
                join_field: "id".to_string(),
                join_type: JoinType::Inner,
            }],
        };

        let query = MultiModelQuery {
            components: vec![component],
            fusion_strategy: FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: Vec::new(),
            order_by: None,
        };

        assert_eq!(query.validate(), Err("component cannot depend on itself"));
    }

    #[test]
    fn component_parallelizability_tracks_dependencies() {
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
                node_filters: Vec::<NodeFilter>::new(),
                edge_filters: Vec::<EdgeFilter>::new(),
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

    #[test]
    fn agentic_view_plan_validates_ordered_dependencies() {
        let vector_view = AgenticView::new(
            "candidate_docs",
            "Find semantically relevant documents",
            MultiModelQuery::new().with_vector_search(VectorSearchExpr {
                collection: "docs".to_string(),
                query_vector: vec![0.1, 0.2],
                top_k: 20,
                threshold: None,
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            }),
        );
        let graph_view = AgenticView::new(
            "expanded_context",
            "Expand through related graph neighborhoods",
            MultiModelQuery::new().with_graph_traversal(GraphTraversalExpr {
                graph_name: "knowledge".to_string(),
                start_nodes: StartNodeSpec::FromComponent(0),
                edge_types: vec!["RELATED".to_string()],
                direction: TraversalDirection::Outgoing,
                max_depth: 2,
                min_depth: 1,
                node_filters: Vec::<NodeFilter>::new(),
                edge_filters: Vec::<EdgeFilter>::new(),
                return_paths: false,
            }),
        )
        .depends_on("candidate_docs")
        .materializable("expanded_context_projection")
        .with_freshness(ProjectionFreshness {
            max_lag_ms: 1_000,
            repair_source: "knowledge".to_string(),
        });

        let plan = AgenticViewPlan::new()
            .with_source_query("Find related incident context")
            .add_view(vector_view)
            .add_view(graph_view)
            .with_output_view("expanded_context");

        assert!(plan.validate().is_ok());
        assert!(plan.views[1].materialization.is_materializable());
    }

    #[test]
    fn agentic_view_plan_rejects_forward_dependency() {
        let view = AgenticView::new(
            "late",
            "Invalid dependency",
            MultiModelQuery::new().with_vector_search(VectorSearchExpr {
                collection: "docs".to_string(),
                query_vector: vec![0.1],
                top_k: 1,
                threshold: None,
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            }),
        )
        .depends_on("future");
        let plan = AgenticViewPlan::new().add_view(view);

        assert_eq!(
            plan.validate(),
            Err("view dependencies must reference earlier views")
        );
    }

    #[test]
    fn agentic_view_plan_rejects_uncataloged_materialization() {
        let view = AgenticView::new(
            "candidate_docs",
            "Find semantically relevant documents",
            MultiModelQuery::new().with_vector_search(VectorSearchExpr {
                collection: "docs".to_string(),
                query_vector: vec![0.1],
                top_k: 1,
                threshold: None,
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            }),
        )
        .materializable("candidate_docs_projection");

        let plan = AgenticViewPlan::new().add_view(view);

        assert_eq!(
            plan.validate(),
            Err("projection repair_source must not be empty")
        );
    }

    #[test]
    fn semantic_join_mode_defaults_to_cosine() {
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
        assert_eq!(a, b);

        let c = JoinType::Semantic {
            threshold: 0.8,
            top_k: 5,
            mode: SemanticJoinMode::LlmBlockBatch(BlockBatchConfig::default()),
        };
        assert_ne!(a, c);
        assert!(a.validate().is_ok());
    }

    #[test]
    fn semantic_join_rejects_invalid_bounds() {
        let join = JoinType::Semantic {
            threshold: 1.5,
            top_k: 0,
            mode: SemanticJoinMode::Cosine,
        };

        assert_eq!(
            join.validate(),
            Err("semantic join threshold must be between 0 and 1")
        );
    }

    #[test]
    fn block_batch_config_default_is_valid() {
        BlockBatchConfig::default()
            .validate()
            .expect("default must validate");
    }

    #[test]
    fn block_batch_config_default_values_are_conservative() {
        let cfg = BlockBatchConfig::default();
        assert_eq!(cfg.batch_size_left, 16);
        assert_eq!(cfg.batch_size_right, 16);
        assert_eq!(cfg.max_calls, 64);
    }

    #[test]
    fn block_batch_config_rejects_zero_fields() {
        let mut left = BlockBatchConfig::default();
        left.batch_size_left = 0;
        assert!(matches!(left.validate(), Err(msg) if msg.contains("batch_size_left")));

        let mut right = BlockBatchConfig::default();
        right.batch_size_right = 0;
        assert!(matches!(right.validate(), Err(msg) if msg.contains("batch_size_right")));

        let mut max_calls = BlockBatchConfig::default();
        max_calls.max_calls = 0;
        assert!(matches!(max_calls.validate(), Err(msg) if msg.contains("max_calls")));
    }

    #[test]
    fn block_batch_config_reports_first_failure() {
        let cfg = BlockBatchConfig {
            batch_size_left: 0,
            batch_size_right: 0,
            max_calls: 0,
        };
        assert!(matches!(cfg.validate(), Err(msg) if msg.contains("batch_size_left")));
    }

    #[test]
    fn target_collection_tracks_model_operation_namespace() {
        let graph_component = QueryComponent {
            model: DataModel::Graph,
            operation: ModelOperation::GraphQuery(GraphQueryExpr {
                graph_name: "knowledge".to_string(),
                normalized_query: "MATCH (n) RETURN n".to_string(),
                output_columns: vec!["n".to_string()],
                uses_legacy_node_rows: false,
                max_depth: 0,
            }),
            filters: vec![],
            dependencies: vec![],
        };

        let metric_component = QueryComponent {
            model: DataModel::Observability,
            operation: ModelOperation::MetricQuery(MetricQueryExpr {
                namespace: "metrics".to_string(),
                metric_name: "latency_ms".to_string(),
                start_time_ns: 0,
                end_time_ns: 1,
                aggregation: proximadb_observability_query::MetricAggregation::Avg,
                group_by: vec![],
                label_filters: std::collections::HashMap::new(),
            }),
            filters: vec![],
            dependencies: vec![],
        };

        assert_eq!(graph_component.target_collection(), Some("knowledge"));
        assert_eq!(
            metric_component.collection_name().as_deref(),
            Some("metrics")
        );
    }

    #[test]
    fn graph_traversal_filters_accept_property_filters() {
        let traversal = GraphTraversalExpr {
            graph_name: "code".to_string(),
            start_nodes: StartNodeSpec::Filter(NodeFilter {
                label: Some("Symbol".to_string()),
                properties: vec![PropertyFilter {
                    name: "name".to_string(),
                    operator: FilterOperator::Eq,
                    value: FilterValue::String("main".to_string()),
                }],
            }),
            edge_types: vec!["CALLS".to_string()],
            direction: TraversalDirection::Outgoing,
            max_depth: 3,
            min_depth: 1,
            node_filters: vec![],
            edge_filters: vec![],
            return_paths: true,
        };

        assert!(matches!(traversal.start_nodes, StartNodeSpec::Filter(_)));
        assert!(traversal.return_paths);
    }
}
