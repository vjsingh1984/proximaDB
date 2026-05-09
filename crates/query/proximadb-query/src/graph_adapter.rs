//! Pure graph-query adaptation helpers shared across query surfaces.

use std::collections::HashMap;

use anyhow::Result;
use proximadb_data_model::DataModel;
use proximadb_graph_query::declarative::graph_query_row_id;
use proximadb_graph_query::service::{
    GraphQueryReadService, GraphQueryService, GraphQueryTraversalService,
};
use proximadb_graph_query::traversal::{
    GraphTraversalExpr, NodeFilter, PropertyFilter as UnifiedPropertyFilter, StartNodeSpec,
    TraversalDirection,
};
use proximadb_proto::proximadb_v1::{
    Node, NodeQuery, PropertyFilter, PropertyFilterOperator, PropertyValue, TraversalAlgorithm,
    TraversalRequest, property_value,
};
use proximadb_query_filter::FilterValue;
use tracing::{debug, info, warn};

use crate::{SubQueryResult, UnifiedRecord, resolve_component_record_ids};

/// Convert a unified graph property filter into the protobuf contract.
pub fn unified_property_filter_to_graph_property_filter(
    filter: &UnifiedPropertyFilter,
) -> Option<PropertyFilter> {
    let value = filter_value_to_graph_property_value(&filter.value)?;
    Some(PropertyFilter {
        key: filter.name.clone(),
        operator: PropertyFilterOperator::Equals as i32,
        value: Some(value),
    })
}

/// Convert a query-runtime filter value into a protobuf graph property value.
pub fn filter_value_to_graph_property_value(value: &FilterValue) -> Option<PropertyValue> {
    let value = match value {
        FilterValue::String(value) => Some(property_value::Value::StringValue(value.clone())),
        FilterValue::Number(value) => {
            if value.fract() == 0.0 && *value >= i64::MIN as f64 && *value <= i64::MAX as f64 {
                Some(property_value::Value::IntValue(*value as i64))
            } else {
                Some(property_value::Value::DoubleValue(*value))
            }
        }
        FilterValue::Bool(value) => Some(property_value::Value::BoolValue(*value)),
        FilterValue::Null => None,
        FilterValue::Array(_) => None,
    }?;

    Some(PropertyValue { value: Some(value) })
}

/// Extract node labels from traversal filters.
pub fn extract_node_labels(filters: &[NodeFilter]) -> Vec<String> {
    filters.iter().filter_map(|f| f.label.clone()).collect()
}

/// Build a protobuf traversal request from a graph traversal expression.
pub fn build_traversal_request(
    expr: &GraphTraversalExpr,
    start_node_id: String,
    node_labels: Vec<String>,
) -> TraversalRequest {
    TraversalRequest {
        graph_id: expr.graph_name.clone(),
        start_node_id,
        max_depth: expr.max_depth,
        edge_types: expr.edge_types.clone(),
        node_labels,
        filters: Vec::new(),
        algorithm: traversal_direction_to_algorithm(&expr.direction) as i32,
        limit: None,
        timeout_ms: None,
        max_frontier: None,
    }
}

/// Build a unified record from a declarative graph query row.
pub fn build_graph_query_record(row: serde_json::Value, index: usize) -> UnifiedRecord {
    UnifiedRecord {
        id: graph_query_row_id(&row, index),
        source_model: DataModel::Graph,
        data: row,
        score: None,
        metadata: HashMap::new(),
    }
}

/// Build a unified record from a traversal node result.
pub fn build_graph_traversal_record(node: &Node, start_node: Option<&str>) -> UnifiedRecord {
    let mut data = serde_json::Map::new();
    data.insert("id".to_string(), serde_json::json!(node.id));
    data.insert("labels".to_string(), serde_json::json!(node.labels));
    data.insert(
        "properties".to_string(),
        serde_json::json!(format!("{:?}", node.properties)),
    );
    if let Some(start_node_id) = start_node {
        data.insert("start_node".to_string(), serde_json::json!(start_node_id));
    }

    UnifiedRecord {
        id: node.id.clone(),
        source_model: DataModel::Graph,
        data: serde_json::Value::Object(data),
        score: None,
        metadata: HashMap::new(),
    }
}

/// Resolve a traversal start-node spec to concrete node IDs.
pub async fn resolve_start_nodes<G>(
    spec: &StartNodeSpec,
    graph_name: &str,
    graph_service: &G,
    component_context: Option<&HashMap<usize, &SubQueryResult>>,
) -> Result<Vec<String>>
where
    G: GraphQueryReadService + ?Sized,
{
    match spec {
        StartNodeSpec::Ids(ids) => {
            debug!("StartNodeSpec::Ids - using {} direct IDs", ids.len());
            Ok(ids.clone())
        }
        StartNodeSpec::Label(label) => {
            debug!(
                "StartNodeSpec::Label - querying nodes with label '{}'",
                label
            );
            resolve_nodes_by_label(graph_name, label, graph_service).await
        }
        StartNodeSpec::Filter(filter) => {
            debug!("StartNodeSpec::Filter - querying nodes matching filter");
            resolve_nodes_by_filter(graph_name, filter, graph_service).await
        }
        StartNodeSpec::FromComponent(component_idx) => {
            debug!(
                "StartNodeSpec::FromComponent - resolving from component {}",
                component_idx
            );
            Ok(resolve_nodes_from_component(
                *component_idx,
                component_context,
            ))
        }
    }
}

/// Execute a graph traversal from a start-node specification using the extracted
/// graph query contracts.
pub async fn execute_graph_traversal_with_service<G>(
    expr: &GraphTraversalExpr,
    graph_service: &G,
    context: Option<&HashMap<usize, &SubQueryResult>>,
) -> Result<SubQueryResult>
where
    G: GraphQueryService + ?Sized,
{
    let start_node_ids =
        resolve_start_nodes(&expr.start_nodes, &expr.graph_name, graph_service, context).await?;

    if start_node_ids.is_empty() {
        debug!("No start nodes resolved for graph traversal");
        return Ok(SubQueryResult::empty(DataModel::Graph));
    }

    info!(
        "Graph traversal starting from {} nodes on graph '{}'",
        start_node_ids.len(),
        expr.graph_name
    );

    let mut all_records = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    for start_id in start_node_ids {
        let traversal_request = build_traversal_request(
            expr,
            start_id.clone(),
            extract_node_labels(&expr.node_filters),
        );

        match graph_service
            .traverse(&expr.graph_name, traversal_request)
            .await
        {
            Ok(response) => {
                for node in response.nodes {
                    if seen_ids.insert(node.id.clone()) {
                        all_records.push(build_graph_traversal_record(&node, Some(&start_id)));
                    }
                }
            }
            Err(error) => {
                warn!("Graph traversal from '{}' failed: {}", start_id, error);
            }
        }
    }

    let count = all_records.len() as u64;
    info!("Graph traversal returned {} unique nodes", count);

    Ok(SubQueryResult {
        source_model: DataModel::Graph,
        records_returned: count,
        records: all_records,
        total_count: Some(count),
        execution_time_us: 0,
        records_scanned: count,
    })
}

/// Execute a graph traversal from explicit input node IDs.
pub async fn execute_graph_traversal_with_input_service<G>(
    expr: &GraphTraversalExpr,
    input_ids: Option<Vec<String>>,
    graph_service: &G,
) -> Result<SubQueryResult>
where
    G: GraphQueryTraversalService + ?Sized,
{
    let start_nodes = input_ids.unwrap_or_else(|| match &expr.start_nodes {
        StartNodeSpec::Ids(ids) => ids.clone(),
        _ => Vec::new(),
    });

    if start_nodes.is_empty() {
        return Ok(SubQueryResult::empty(DataModel::Graph));
    }

    let mut all_records = Vec::new();

    for start_id in start_nodes {
        let traversal_request = build_traversal_request(expr, start_id, Vec::new());

        if let Ok(response) = graph_service
            .traverse(&expr.graph_name, traversal_request)
            .await
        {
            for node in response.nodes {
                all_records.push(build_graph_traversal_record(&node, None));
            }
        }
    }

    let count = all_records.len() as u64;
    Ok(SubQueryResult {
        source_model: DataModel::Graph,
        records_returned: count,
        records: all_records,
        total_count: Some(count),
        execution_time_us: 0,
        records_scanned: count,
    })
}

fn traversal_direction_to_algorithm(_direction: &TraversalDirection) -> TraversalAlgorithm {
    TraversalAlgorithm::Bfs
}

async fn resolve_nodes_by_label<G>(
    graph_name: &str,
    label: &str,
    graph_service: &G,
) -> Result<Vec<String>>
where
    G: GraphQueryReadService + ?Sized,
{
    let query = NodeQuery {
        graph_id: graph_name.to_string(),
        labels: vec![label.to_string()],
        filters: Vec::new(),
        limit: None,
        offset: None,
        continuation_token: None,
    };

    match graph_service.query_nodes(graph_name, query).await {
        Ok(nodes) => {
            let ids: Vec<String> = nodes.into_iter().map(|node| node.id.clone()).collect();
            info!(
                "Resolved {} nodes with label '{}' in graph '{}'",
                ids.len(),
                label,
                graph_name
            );
            Ok(ids)
        }
        Err(error) => {
            warn!("Failed to query nodes by label '{}': {}", label, error);
            Ok(Vec::new())
        }
    }
}

async fn resolve_nodes_by_filter<G>(
    graph_name: &str,
    filter: &NodeFilter,
    graph_service: &G,
) -> Result<Vec<String>>
where
    G: GraphQueryReadService + ?Sized,
{
    let query = NodeQuery {
        graph_id: graph_name.to_string(),
        labels: filter.label.iter().cloned().collect(),
        filters: filter
            .properties
            .iter()
            .filter_map(unified_property_filter_to_graph_property_filter)
            .collect(),
        limit: None,
        offset: None,
        continuation_token: None,
    };

    match graph_service.query_nodes(graph_name, query).await {
        Ok(nodes) => {
            let ids: Vec<String> = nodes.into_iter().map(|node| node.id.clone()).collect();
            info!(
                "Resolved {} nodes matching filter in graph '{}'",
                ids.len(),
                graph_name
            );
            Ok(ids)
        }
        Err(error) => {
            warn!("Failed to query nodes by filter: {}", error);
            Ok(Vec::new())
        }
    }
}

fn resolve_nodes_from_component(
    component_idx: usize,
    context: Option<&HashMap<usize, &SubQueryResult>>,
) -> Vec<String> {
    if context.is_none() {
        warn!(
            "FromComponent({}) requires context, but none provided",
            component_idx
        );
    } else if context.and_then(|ctx| ctx.get(&component_idx)).is_none() {
        warn!(
            "FromComponent({}) references non-existent component",
            component_idx
        );
    }

    let ids = resolve_component_record_ids(component_idx, context);

    if let Some(prior_result) = context.and_then(|ctx| ctx.get(&component_idx)) {
        info!(
            "Resolved {} node IDs from component {} (model: {:?})",
            ids.len(),
            component_idx,
            prior_result.source_model
        );
    }

    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use proximadb_graph_query::service::{
        GraphQueryReadService, GraphQueryResult, GraphQueryTraversalService,
    };
    use proximadb_graph_query::traversal::PropertyFilter;
    use proximadb_proto::proximadb_v1::{
        Edge, EdgeQuery, Node, PropertyValue, TraversalResponse, TraversalStats, property_value,
    };
    use proximadb_query_filter::{FilterOperator, FilterValue};
    use std::sync::Arc;

    #[test]
    fn graph_property_filter_conversion_handles_string_and_number() {
        let string_filter = UnifiedPropertyFilter {
            name: "name".to_string(),
            operator: FilterOperator::Eq,
            value: FilterValue::String("alice".to_string()),
        };
        let converted =
            unified_property_filter_to_graph_property_filter(&string_filter).expect("converted");
        assert_eq!(converted.key, "name");

        let number = filter_value_to_graph_property_value(&FilterValue::Number(42.0)).unwrap();
        assert!(matches!(
            number.value,
            Some(property_value::Value::IntValue(42))
        ));
    }

    #[test]
    fn extract_node_labels_returns_only_present_labels() {
        let filters = vec![
            NodeFilter {
                label: Some("Person".to_string()),
                properties: vec![],
            },
            NodeFilter {
                label: None,
                properties: vec![],
            },
        ];

        assert_eq!(extract_node_labels(&filters), vec!["Person".to_string()]);
    }

    #[test]
    fn traversal_request_copies_graph_fields() {
        let expr = GraphTraversalExpr {
            graph_name: "knowledge".to_string(),
            start_nodes: StartNodeSpec::Ids(vec!["n1".to_string()]),
            edge_types: vec!["KNOWS".to_string()],
            direction: TraversalDirection::Both,
            max_depth: 3,
            min_depth: 1,
            node_filters: vec![],
            edge_filters: vec![proximadb_graph_query::traversal::EdgeFilter {
                edge_type: None,
                properties: vec![PropertyFilter {
                    name: "weight".to_string(),
                    operator: FilterOperator::Gt,
                    value: FilterValue::Number(0.5),
                }],
                weight_range: None,
            }],
            return_paths: false,
        };

        let request = build_traversal_request(&expr, "start".to_string(), vec!["Person".into()]);
        assert_eq!(request.graph_id, "knowledge");
        assert_eq!(request.start_node_id, "start");
        assert_eq!(request.max_depth, 3);
        assert_eq!(request.edge_types, vec!["KNOWS"]);
        assert_eq!(request.node_labels, vec!["Person"]);
    }

    #[test]
    fn build_graph_query_record_uses_stable_row_identifier() {
        let record = build_graph_query_record(serde_json::json!({ "node_id": "alice" }), 7);
        assert_eq!(record.id, "alice");
        assert_eq!(record.source_model, DataModel::Graph);
        assert_eq!(record.data["node_id"], "alice");
    }

    #[test]
    fn build_graph_traversal_record_adds_start_node_when_present() {
        let node = Node {
            id: "alice".to_string(),
            labels: vec!["Person".to_string()],
            properties: HashMap::from([(
                "name".to_string(),
                PropertyValue {
                    value: Some(property_value::Value::StringValue("Alice".to_string())),
                },
            )]),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };

        let record = build_graph_traversal_record(&node, Some("seed-1"));
        assert_eq!(record.id, "alice");
        assert_eq!(record.data["start_node"], "seed-1");
        assert_eq!(record.data["labels"], serde_json::json!(["Person"]));
    }

    struct StubGraphReadService {
        nodes: Vec<Arc<Node>>,
    }

    #[async_trait]
    impl GraphQueryReadService for StubGraphReadService {
        async fn list_graphs(&self) -> GraphQueryResult<Vec<String>> {
            Ok(vec!["social".to_string()])
        }

        async fn get_node(
            &self,
            _graph_id: &str,
            node_id: &str,
        ) -> GraphQueryResult<Option<Arc<Node>>> {
            Ok(self.nodes.iter().find(|node| node.id == node_id).cloned())
        }

        async fn query_nodes(
            &self,
            _graph_id: &str,
            query: NodeQuery,
        ) -> GraphQueryResult<Vec<Arc<Node>>> {
            let mut nodes = self.nodes.clone();
            if !query.labels.is_empty() {
                nodes.retain(|node| query.labels.iter().all(|label| node.labels.contains(label)));
            }
            if !query.filters.is_empty() {
                nodes.retain(|node| {
                    query.filters.iter().all(|filter| {
                        node.properties
                            .get(&filter.key)
                            .and_then(|value| value.value.as_ref())
                            == filter.value.as_ref().and_then(|value| value.value.as_ref())
                    })
                });
            }
            Ok(nodes)
        }

        async fn query_edges(
            &self,
            _graph_id: &str,
            _query: EdgeQuery,
        ) -> GraphQueryResult<Vec<Arc<Edge>>> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl GraphQueryTraversalService for StubGraphReadService {
        async fn traverse(
            &self,
            _graph_id: &str,
            request: TraversalRequest,
        ) -> GraphQueryResult<TraversalResponse> {
            let nodes = if request.start_node_id == "seed-1" {
                vec![stub_node("alice", "Person", "Alice")]
            } else {
                vec![
                    stub_node("alice", "Person", "Alice"),
                    stub_node("bob", "Person", "Bob"),
                ]
            };

            Ok(TraversalResponse {
                nodes: nodes.into_iter().map(|node| (*node).clone()).collect(),
                edges: Vec::new(),
                paths: Vec::new(),
                stats: Some(TraversalStats {
                    nodes_visited: 0,
                    edges_traversed: 0,
                    max_depth_reached: 0,
                    execution_time_microseconds: 0,
                }),
            })
        }

        async fn get_neighbors(
            &self,
            _graph_id: &str,
            _node_id: &str,
        ) -> GraphQueryResult<Vec<Arc<Node>>> {
            Ok(Vec::new())
        }
    }

    fn stub_node(id: &str, label: &str, name: &str) -> Arc<Node> {
        Arc::new(Node {
            id: id.to_string(),
            labels: vec![label.to_string()],
            properties: HashMap::from([(
                "name".to_string(),
                PropertyValue {
                    value: Some(property_value::Value::StringValue(name.to_string())),
                },
            )]),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        })
    }

    #[tokio::test]
    async fn resolve_start_nodes_uses_canonical_label_and_filter_queries() {
        let graph_service = StubGraphReadService {
            nodes: vec![
                stub_node("alice", "Person", "Alice"),
                stub_node("bob", "Person", "Bob"),
            ],
        };

        let label_ids = resolve_start_nodes(
            &StartNodeSpec::Label("Person".to_string()),
            "social",
            &graph_service,
            None,
        )
        .await
        .expect("label-based resolution should succeed");
        assert_eq!(label_ids.len(), 2);

        let filter_ids = resolve_start_nodes(
            &StartNodeSpec::Filter(NodeFilter {
                label: Some("Person".to_string()),
                properties: vec![UnifiedPropertyFilter {
                    name: "name".to_string(),
                    operator: FilterOperator::Eq,
                    value: FilterValue::String("Alice".to_string()),
                }],
            }),
            "social",
            &graph_service,
            None,
        )
        .await
        .expect("filter-based resolution should succeed");
        assert_eq!(filter_ids, vec!["alice".to_string()]);
    }

    #[tokio::test]
    async fn execute_graph_traversal_with_service_deduplicates_across_start_nodes() {
        let graph_service = StubGraphReadService { nodes: Vec::new() };

        let result = execute_graph_traversal_with_service(
            &GraphTraversalExpr {
                graph_name: "social".to_string(),
                start_nodes: StartNodeSpec::Ids(vec!["seed-1".to_string(), "seed-2".to_string()]),
                edge_types: vec!["KNOWS".to_string()],
                direction: TraversalDirection::Outgoing,
                max_depth: 2,
                min_depth: 1,
                node_filters: vec![NodeFilter {
                    label: Some("Person".to_string()),
                    properties: vec![],
                }],
                edge_filters: vec![],
                return_paths: false,
            },
            &graph_service,
            None,
        )
        .await
        .expect("traversal should succeed");

        assert_eq!(result.records_returned, 2);
        let ids: std::collections::HashSet<_> = result
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect();
        assert!(ids.contains("alice"));
        assert!(ids.contains("bob"));
    }

    #[tokio::test]
    async fn execute_graph_traversal_with_input_service_shapes_nodes() {
        let graph_service = StubGraphReadService { nodes: Vec::new() };

        let result = execute_graph_traversal_with_input_service(
            &GraphTraversalExpr {
                graph_name: "social".to_string(),
                start_nodes: StartNodeSpec::Ids(vec!["seed-1".to_string()]),
                edge_types: vec![],
                direction: TraversalDirection::Outgoing,
                max_depth: 1,
                min_depth: 0,
                node_filters: vec![],
                edge_filters: vec![],
                return_paths: false,
            },
            Some(vec!["seed-1".to_string()]),
            &graph_service,
        )
        .await
        .expect("input traversal should succeed");

        assert_eq!(result.records_returned, 1);
        assert_eq!(result.records[0].id, "alice");
        assert!(result.records[0].data.get("start_node").is_none());
    }
}
