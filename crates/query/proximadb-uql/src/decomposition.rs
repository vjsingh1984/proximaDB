//! Query Decomposition
//!
//! Parses unified queries and decomposes them into model-specific sub-queries
//! that can be executed independently or with dependencies.

use anyhow::{Result, anyhow};
use regex::Regex;
use std::collections::HashMap;
use tracing::{debug, error, warn};

use super::ast::*;
use super::fusion::FusionStrategy;

/// Query decomposer that parses and breaks down multi-model queries
pub struct QueryDecomposer {
    /// Regex patterns for query parsing
    patterns: QueryPatterns,
}

/// Compiled regex patterns for query parsing
struct QueryPatterns {
    vector_similar: Option<Regex>,
    vector_distance: Option<Regex>,
    #[allow(dead_code)]
    json_path: Option<Regex>,
    graph_traverse: Option<Regex>,
    graph_connected: Option<Regex>,
    log_query: Option<Regex>,
    metric_query: Option<Regex>,
}

fn compile_pattern(pattern_name: &str, pattern: &str) -> Option<Regex> {
    match Regex::new(pattern) {
        Ok(compiled) => Some(compiled),
        Err(regex_error) => {
            error!(
                pattern_name,
                error = %regex_error,
                "Unified query regex failed to compile; related capability disabled"
            );
            None
        }
    }
}

fn current_time_nanos() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i64,
        Err(clock_error) => {
            warn!(
                error = %clock_error,
                "System clock moved backwards; using epoch for observability time bounds"
            );
            0
        }
    }
}

impl QueryDecomposer {
    /// Create a new query decomposer
    pub fn new() -> Self {
        Self {
            patterns: QueryPatterns {
                vector_similar: compile_pattern(
                    "VECTOR_SIMILAR",
                    r"(?i)VECTOR_SIMILAR\s*\(\s*([^,]+)\s*,\s*([^,]+)\s*(?:,\s*([0-9.]+))?\s*\)",
                ),
                vector_distance: compile_pattern(
                    "VECTOR_DISTANCE",
                    r"(?i)VECTOR_DISTANCE\s*\(\s*([^,]+)\s*,\s*([^)]+)\s*\)",
                ),
                json_path: compile_pattern(
                    "JSON_PATH",
                    r"\$\.[\w.]+\s*(?:=|!=|>|>=|<|<=|LIKE|IN|CONTAINS)\s*[^AND OR]+",
                ),
                graph_traverse: compile_pattern(
                    "GRAPH_TRAVERSE",
                    r"(?i)GRAPH_TRAVERSE\s*\(\s*([^,]+)\s*,\s*'([^']+)'\s*(?:,\s*(\d+))?\s*\)",
                ),
                graph_connected: compile_pattern(
                    "GRAPH_CONNECTED",
                    r"(?i)GRAPH_CONNECTED\s*\(\s*([^,]+)\s*,\s*'([^']+)'\s*,\s*([^)]+)\s*\)",
                ),
                log_query: compile_pattern(
                    "LOG_QUERY",
                    r"(?i)LOG_QUERY\s*\(\s*'([^']+)'\s*(?:,\s*([^,]+)\s*,\s*([^,]+))?\s*\)",
                ),
                metric_query: compile_pattern(
                    "METRIC_QUERY",
                    r"(?i)METRIC_(?:AGG|QUERY)\s*\(\s*'([^']+)'\s*,\s*'([^']+)'\s*(?:,\s*'([^']+)')?\s*\)",
                ),
            },
        }
    }

    /// Decompose a query string into a multi-model query
    pub fn decompose(&self, query: &str) -> Result<MultiModelQuery> {
        debug!("Decomposing query: {}", query);

        let mut multi_query = MultiModelQuery::new();
        let mut component_index = 0;

        // Detect and extract vector search components
        if let Some(vector_component) = self.extract_vector_component(query)? {
            debug!("Found vector search component");
            multi_query.components.push(vector_component);
            component_index += 1;
        }

        // Detect and extract document query components
        if let Some(doc_component) = self.extract_document_component(query)? {
            debug!("Found document query component");
            multi_query.components.push(doc_component);
            component_index += 1;
        }

        // Detect and extract graph traversal components
        if let Some(graph_component) = self.extract_graph_component(query, component_index)? {
            debug!("Found graph traversal component");
            multi_query.components.push(graph_component);
            let _component_index = component_index + 1;
        }

        // Detect and extract observability components
        if let Some(obs_component) = self.extract_observability_component(query)? {
            debug!("Found observability component");
            multi_query.components.push(obs_component);
        }

        // Determine fusion strategy based on query structure
        multi_query.fusion_strategy = self.infer_fusion_strategy(query);

        // Extract global LIMIT
        multi_query.limit = self.extract_limit(query);

        // Extract ORDER BY
        multi_query.order_by = self.extract_order_by(query);

        if multi_query.components.is_empty() {
            return Err(anyhow!(
                "No recognizable query components found in: {}",
                query
            ));
        }

        debug!(
            "Decomposed into {} components with {:?} fusion",
            multi_query.components.len(),
            multi_query.fusion_strategy
        );

        multi_query
            .validate()
            .map_err(|reason| anyhow!("Invalid decomposed multi-model query: {}", reason))?;

        Ok(multi_query)
    }

    /// Placeholder vector used when a UQL query references a bound vector
    /// parameter (`?`). Protocol handlers must replace this before execution.
    fn bound_parameter_vector_placeholder(&self) -> Vec<f32> {
        vec![0.0]
    }

    /// Extract vector search component from query
    fn extract_vector_component(&self, query: &str) -> Result<Option<QueryComponent>> {
        // Check for VECTOR_SIMILAR
        if let Some(vector_similar) = self.patterns.vector_similar.as_ref()
            && let Some(caps) = vector_similar.captures(query)
        {
            let _field = caps.get(1).map_or("embedding", |m| m.as_str().trim());
            let threshold = caps.get(3).and_then(|m| m.as_str().parse::<f32>().ok());

            // Extract collection from FROM clause
            let collection = self
                .extract_collection(query)
                // TD-007: unwrap_or with safe default - "default" collection for unspecified queries
                .unwrap_or("default".to_string());

            // Extract top_k from LIMIT
            // TD-007: unwrap_or with safe default - 10 is reasonable default for result limit
            let top_k = self.extract_limit(query).unwrap_or(10);

            return Ok(Some(QueryComponent {
                model: DataModel::Vector,
                operation: ModelOperation::VectorSearch(VectorSearchExpr {
                    collection,
                    query_vector: self.bound_parameter_vector_placeholder(),
                    top_k,
                    threshold,
                    metric: self.infer_distance_metric(query),
                    params: VectorSearchParams::default(),
                }),
                filters: vec![],
                dependencies: vec![],
            }));
        }

        // Check for ORDER BY VECTOR_DISTANCE
        if self
            .patterns
            .vector_distance
            .as_ref()
            .is_some_and(|pattern| pattern.is_match(query))
        {
            let collection = self
                .extract_collection(query)
                // TD-007: unwrap_or with safe default - "default" collection for unspecified queries
                .unwrap_or("default".to_string());
            // TD-007: unwrap_or with safe default - 10 is reasonable default for result limit
            let top_k = self.extract_limit(query).unwrap_or(10);

            return Ok(Some(QueryComponent {
                model: DataModel::Vector,
                operation: ModelOperation::VectorSearch(VectorSearchExpr {
                    collection,
                    query_vector: self.bound_parameter_vector_placeholder(),
                    top_k,
                    threshold: None,
                    metric: self.infer_distance_metric(query),
                    params: VectorSearchParams::default(),
                }),
                filters: vec![],
                dependencies: vec![],
            }));
        }

        Ok(None)
    }

    /// Extract document query component from query
    fn extract_document_component(&self, query: &str) -> Result<Option<QueryComponent>> {
        // Check for JSON path expressions ($.field)
        let path_filters = self.extract_path_filters(query);

        if path_filters.is_empty() {
            return Ok(None);
        }

        let collection = self
            .extract_collection(query)
            // TD-007: unwrap_or with safe default - "documents" collection for document queries
            .unwrap_or("documents".to_string());

        Ok(Some(QueryComponent {
            model: DataModel::Document,
            operation: ModelOperation::DocumentQuery(DocumentQueryExpr {
                collection,
                path_filters,
                text_search: self.extract_text_search(query),
                projection: self.extract_projection(query),
                sort: self.extract_document_sort(query),
                limit: self.extract_limit(query),
            }),
            filters: vec![],
            dependencies: vec![],
        }))
    }

    /// Extract graph traversal component from query
    fn extract_graph_component(
        &self,
        query: &str,
        prev_component_count: usize,
    ) -> Result<Option<QueryComponent>> {
        // Check for GRAPH_TRAVERSE
        if let Some(graph_traverse) = self.patterns.graph_traverse.as_ref()
            && let Some(caps) = graph_traverse.captures(query)
        {
            let graph_name = caps
                .get(1)
                .map_or("default".to_string(), |m| m.as_str().trim().to_string());
            let edge_type = caps
                .get(2)
                .map_or("*".to_string(), |m| m.as_str().to_string());
            let max_depth = caps
                .get(3)
                .and_then(|m| m.as_str().parse::<u32>().ok())
                // TD-007: unwrap_or with safe default - depth 2 is reasonable for graph traversal
                .unwrap_or(2);

            // Check if this depends on a previous component
            let dependencies = if prev_component_count > 0 && query.contains("JOIN") {
                vec![ComponentDependency {
                    component_index: prev_component_count - 1,
                    join_field: "id".to_string(),
                    join_type: JoinType::Inner,
                }]
            } else {
                vec![]
            };

            return Ok(Some(QueryComponent {
                model: DataModel::Graph,
                operation: ModelOperation::GraphTraversal(GraphTraversalExpr {
                    graph_name,
                    start_nodes: StartNodeSpec::Label("*".to_string()),
                    edge_types: vec![edge_type],
                    direction: TraversalDirection::Outgoing,
                    max_depth,
                    min_depth: 1,
                    node_filters: vec![],
                    edge_filters: vec![],
                    return_paths: query.to_lowercase().contains("path"),
                }),
                filters: vec![],
                dependencies,
            }));
        }

        // Check for GRAPH_CONNECTED
        if let Some(graph_connected) = self.patterns.graph_connected.as_ref()
            && let Some(caps) = graph_connected.captures(query)
        {
            let graph_name = "default".to_string(); // Would need to infer from context
            let edge_type = caps
                .get(2)
                .map_or("*".to_string(), |m| m.as_str().to_string());

            return Ok(Some(QueryComponent {
                model: DataModel::Graph,
                operation: ModelOperation::GraphTraversal(GraphTraversalExpr {
                    graph_name,
                    start_nodes: StartNodeSpec::Label("*".to_string()),
                    edge_types: vec![edge_type],
                    direction: TraversalDirection::Both,
                    max_depth: 1,
                    min_depth: 1,
                    node_filters: vec![],
                    edge_filters: vec![],
                    return_paths: false,
                }),
                filters: vec![],
                dependencies: vec![],
            }));
        }

        Ok(None)
    }

    /// Extract observability component from query
    fn extract_observability_component(&self, query: &str) -> Result<Option<QueryComponent>> {
        // Check for LOG_QUERY
        if let Some(log_query) = self.patterns.log_query.as_ref()
            && let Some(caps) = log_query.captures(query)
        {
            let namespace = caps
                .get(1)
                .map_or("default".to_string(), |m| m.as_str().to_string());

            let now = current_time_nanos();
            let hour_ago = now - 3_600_000_000_000; // 1 hour in nanoseconds

            return Ok(Some(QueryComponent {
                model: DataModel::Observability,
                operation: ModelOperation::LogQuery(LogQueryExpr {
                    namespace,
                    start_time_ns: hour_ago,
                    end_time_ns: now,
                    query: None,
                    severities: vec![],
                    services: vec![],
                    // TD-007: unwrap_or with safe default - 100 is reasonable for log query limit
                    limit: self.extract_limit(query).unwrap_or(100),
                }),
                filters: vec![],
                dependencies: vec![],
            }));
        }

        // Check for METRIC_AGG
        if let Some(metric_query) = self.patterns.metric_query.as_ref()
            && let Some(caps) = metric_query.captures(query)
        {
            let namespace = caps
                .get(1)
                .map_or("default".to_string(), |m| m.as_str().to_string());
            let metric_name = caps
                .get(2)
                .map_or("*".to_string(), |m| m.as_str().to_string());
            let agg_type = caps
                .get(3)
                .map_or("AVG".to_string(), |m| m.as_str().to_uppercase());

            let now = current_time_nanos();
            let hour_ago = now - 3_600_000_000_000;

            let aggregation = match agg_type.as_str() {
                "SUM" => MetricAggregation::Sum,
                "AVG" => MetricAggregation::Avg,
                "MIN" => MetricAggregation::Min,
                "MAX" => MetricAggregation::Max,
                "COUNT" => MetricAggregation::Count,
                "P50" => MetricAggregation::P50,
                "P90" => MetricAggregation::P90,
                "P95" => MetricAggregation::P95,
                "P99" => MetricAggregation::P99,
                "RATE" => MetricAggregation::Rate,
                _ => MetricAggregation::Avg,
            };

            return Ok(Some(QueryComponent {
                model: DataModel::Observability,
                operation: ModelOperation::MetricQuery(MetricQueryExpr {
                    namespace,
                    metric_name,
                    start_time_ns: hour_ago,
                    end_time_ns: now,
                    aggregation,
                    group_by: vec![],
                    label_filters: HashMap::new(),
                }),
                filters: vec![],
                dependencies: vec![],
            }));
        }

        Ok(None)
    }

    /// Extract collection name from FROM clause
    fn extract_collection(&self, query: &str) -> Option<String> {
        let from_pattern = Regex::new(r"(?i)FROM\s+(\w+(?:\.\w+)?)").ok()?;
        from_pattern
            .captures(query)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }

    /// Extract JSON path filters from query
    fn extract_path_filters(&self, query: &str) -> Vec<PathFilter> {
        let mut filters = Vec::new();

        // Match patterns like $.field = 'value' or $.field > 10
        let path_filter_pattern = match Regex::new(
            r"\$\.(\w+(?:\.\w+)*)\s*(=|!=|>|>=|<|<=|LIKE|IN|CONTAINS)\s*('[^']*'|\d+(?:\.\d+)?|\btrue\b|\bfalse\b|\bnull\b)",
        ) {
            Ok(pattern) => pattern,
            Err(regex_error) => {
                error!(
                    error = %regex_error,
                    "PATH_FILTER regex failed to compile; document path filters disabled"
                );
                return filters;
            }
        };

        for caps in path_filter_pattern.captures_iter(query) {
            // TD-007: unwrap_or with safe default - empty path if not captured
            let path = format!("$.{}", caps.get(1).map_or("", |m| m.as_str()));
            let op_str = caps
                .get(2)
                .map(|m| m.as_str().to_uppercase())
                .unwrap_or_default();
            // TD-007: unwrap_or with safe default - empty value if not captured
            let value_str = caps.get(3).map_or("", |m| m.as_str());

            let operator = match op_str.as_str() {
                "=" => FilterOperator::Eq,
                "!=" => FilterOperator::Ne,
                ">" => FilterOperator::Gt,
                ">=" => FilterOperator::Gte,
                "<" => FilterOperator::Lt,
                "<=" => FilterOperator::Lte,
                "LIKE" => FilterOperator::Contains,
                "IN" => FilterOperator::In,
                "CONTAINS" => FilterOperator::Contains,
                _ => FilterOperator::Eq,
            };

            let value = if value_str.starts_with('\'') && value_str.ends_with('\'') {
                FilterValue::String(value_str[1..value_str.len() - 1].to_string())
            } else if value_str == "true" {
                FilterValue::Bool(true)
            } else if value_str == "false" {
                FilterValue::Bool(false)
            } else if value_str == "null" {
                FilterValue::Null
            } else if let Ok(num) = value_str.parse::<f64>() {
                FilterValue::Number(num)
            } else {
                FilterValue::String(value_str.to_string())
            };

            filters.push(PathFilter {
                path,
                operator,
                value,
            });
        }

        filters
    }

    /// Extract text search from query
    fn extract_text_search(&self, query: &str) -> Option<String> {
        let text_pattern = Regex::new(r"(?i)TEXT_SEARCH\s*\(\s*'([^']+)'\s*\)").ok()?;
        text_pattern
            .captures(query)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }

    /// Extract projection (SELECT fields)
    fn extract_projection(&self, query: &str) -> Vec<String> {
        let select_pattern = Regex::new(r"(?i)SELECT\s+(.+?)\s+FROM").ok();
        if let Some(pattern) = select_pattern
            && let Some(caps) = pattern.captures(query)
            && let Some(fields) = caps.get(1)
        {
            let fields_str = fields.as_str().trim();
            if fields_str == "*" {
                return vec![];
            }
            return fields_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        vec![]
    }

    /// Extract document sort from ORDER BY clause
    fn extract_document_sort(&self, query: &str) -> Option<DocumentSort> {
        let order_pattern =
            Regex::new(r"(?i)ORDER\s+BY\s+\$\.(\w+(?:\.\w+)*)\s*(ASC|DESC)?").ok()?;
        order_pattern.captures(query).map(|caps| {
            // TD-007: unwrap_or with safe default - empty path if not captured
            let path = format!("$.{}", caps.get(1).map_or("", |m| m.as_str()));
            let ascending = caps
                .get(2)
                // TD-007: unwrap_or with safe default - true means ascending order
                .is_none_or(|m| m.as_str().to_uppercase() != "DESC");
            DocumentSort { path, ascending }
        })
    }

    /// Extract LIMIT value
    fn extract_limit(&self, query: &str) -> Option<u32> {
        let limit_pattern = Regex::new(r"(?i)LIMIT\s+(\d+)").ok()?;
        limit_pattern
            .captures(query)
            .and_then(|caps| caps.get(1))
            .and_then(|m| m.as_str().parse::<u32>().ok())
    }

    /// Extract ORDER BY clause
    fn extract_order_by(&self, query: &str) -> Option<OrderBy> {
        let order_pattern = Regex::new(r"(?i)ORDER\s+BY\s+(\w+)\s*(ASC|DESC)?").ok()?;
        order_pattern.captures(query).map(|caps| {
            let field = caps
                .get(1)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let ascending = caps
                .get(2)
                // TD-007: unwrap_or with safe default - true means ascending order
                .is_none_or(|m| m.as_str().to_uppercase() != "DESC");
            OrderBy { field, ascending }
        })
    }

    /// Infer distance metric from query
    fn infer_distance_metric(&self, query: &str) -> DistanceMetric {
        let query_lower = query.to_lowercase();
        if query_lower.contains("cosine") {
            DistanceMetric::Cosine
        } else if query_lower.contains("euclidean") || query_lower.contains("l2") {
            DistanceMetric::L2
        } else if query_lower.contains("dot") || query_lower.contains("inner") {
            DistanceMetric::InnerProduct
        } else if query_lower.contains("manhattan") || query_lower.contains("l1") {
            DistanceMetric::L1
        } else {
            DistanceMetric::Cosine // Default
        }
    }

    /// Infer fusion strategy from query structure
    fn infer_fusion_strategy(&self, query: &str) -> FusionStrategy {
        let query_lower = query.to_lowercase();

        if query_lower.contains(" or ") {
            FusionStrategy::Union
        } else if query_lower.contains("rank") || query_lower.contains("score") {
            FusionStrategy::RankedFusion {
                weights: HashMap::new(),
                normalize: true,
            }
        } else {
            // Default to intersection for AND conditions
            FusionStrategy::Intersection
        }
    }
}

impl Default for QueryDecomposer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_collection() {
        let decomposer = QueryDecomposer::new();

        let query = "SELECT * FROM products WHERE $.category = 'electronics'";
        assert_eq!(
            decomposer.extract_collection(query),
            Some("products".to_string())
        );

        let query = "SELECT * FROM documents.users LIMIT 10";
        assert_eq!(
            decomposer.extract_collection(query),
            Some("documents.users".to_string())
        );
    }

    #[test]
    fn test_extract_path_filters() {
        let decomposer = QueryDecomposer::new();

        let query = "SELECT * FROM products WHERE $.category = 'electronics' AND $.price > 100";
        let filters = decomposer.extract_path_filters(query);

        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].path, "$.category");
        assert!(matches!(filters[0].operator, FilterOperator::Eq));
        assert!(matches!(filters[1].operator, FilterOperator::Gt));
    }

    #[test]
    fn test_extract_limit() {
        let decomposer = QueryDecomposer::new();

        assert_eq!(decomposer.extract_limit("SELECT * LIMIT 10"), Some(10));
        assert_eq!(decomposer.extract_limit("SELECT * LIMIT 100"), Some(100));
        assert_eq!(decomposer.extract_limit("SELECT * FROM table"), None);
    }

    #[test]
    fn test_decompose_vector_query() {
        let decomposer = QueryDecomposer::new();

        let query = "SELECT * FROM embeddings WHERE VECTOR_SIMILAR(embedding, ?, 0.8) LIMIT 10";
        let result = decomposer.decompose(query).unwrap();

        assert_eq!(result.components.len(), 1);
        assert!(matches!(result.components[0].model, DataModel::Vector));
    }

    #[test]
    fn test_decompose_hybrid_query() {
        let decomposer = QueryDecomposer::new();

        let query = "SELECT * FROM products WHERE $.category = 'electronics' AND VECTOR_SIMILAR($.embedding, ?, 0.8) LIMIT 20";
        let result = decomposer.decompose(query).unwrap();

        assert_eq!(result.components.len(), 2);
    }

    #[test]
    fn test_infer_fusion_strategy() {
        let decomposer = QueryDecomposer::new();

        let and_query = "SELECT * WHERE $.a = 1 AND $.b = 2";
        assert!(matches!(
            decomposer.infer_fusion_strategy(and_query),
            FusionStrategy::Intersection
        ));

        let or_query = "SELECT * WHERE $.a = 1 OR $.b = 2";
        assert!(matches!(
            decomposer.infer_fusion_strategy(or_query),
            FusionStrategy::Union
        ));
    }
}
