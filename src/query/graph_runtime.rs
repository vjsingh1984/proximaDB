//! Shared runtime helpers for declarative graph queries.
//!
//! This module centralizes execution of the supported read-only graph subset
//! once it has been lowered into [`GraphQueryExpr`]. It also owns the
//! canonical row-shaping contract so facade, federated, and unified runtimes
//! do not each materialize graph rows differently.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use serde_json::Value;

use crate::graph::Node;
use crate::graph::service::GraphOperationsService;
use crate::proto::proximadb_v1::{PropertyValue, property_value};
use crate::query::graph_subset::{
    GraphExecutionStats, execute_supported_graph_query,
    execute_supported_graph_query_with_start_nodes, parse_supported_graph_query,
};
use crate::query::unified::ast::GraphQueryExpr;

#[derive(Debug, Clone)]
pub(crate) struct GraphQueryRuntimeResult {
    pub(crate) rows: Vec<Value>,
    pub(crate) stats: GraphExecutionStats,
}

/// Execute a lowered declarative graph query through the shared graph subset.
///
/// The returned rows are already shaped into the canonical outward contract:
/// scalar projections preserve their declared columns, while whole-node
/// projections use the legacy `node_id`/`label`/`properties` row envelope.
pub(crate) async fn execute_graph_query_expr(
    graph_ops: &GraphOperationsService,
    expr: &GraphQueryExpr,
) -> Result<GraphQueryRuntimeResult> {
    execute_graph_query_expr_with_start_nodes(graph_ops, expr, None).await
}

/// Execute a lowered declarative graph query through the shared graph subset,
/// optionally constraining the first bound node variable to explicit node IDs.
pub(crate) async fn execute_graph_query_expr_with_start_nodes(
    graph_ops: &GraphOperationsService,
    expr: &GraphQueryExpr,
    start_node_ids: Option<&[String]>,
) -> Result<GraphQueryRuntimeResult> {
    let parsed = parse_supported_graph_query(
        &expr.normalized_query,
        Some(&expr.graph_name),
        Some(&expr.graph_name),
    )?;
    let executed = if let Some(start_node_ids) = start_node_ids {
        execute_supported_graph_query_with_start_nodes(graph_ops, &parsed, Some(start_node_ids))
            .await?
    } else {
        execute_supported_graph_query(graph_ops, &parsed).await?
    };
    let rows = executed
        .rows
        .into_iter()
        .map(|row| shape_graph_query_row(row, expr.uses_legacy_node_rows, &expr.output_columns))
        .collect::<Result<Vec<_>>>()?;

    Ok(GraphQueryRuntimeResult {
        rows,
        stats: executed.stats,
    })
}

/// Shape a raw graph-subset row into the outward graph query contract.
pub(crate) fn shape_graph_query_row(
    row: Value,
    uses_legacy_node_rows: bool,
    output_columns: &[String],
) -> Result<Value> {
    if uses_legacy_node_rows {
        materialize_legacy_graph_query_row(row)
    } else {
        retain_graph_query_output_columns(row, output_columns)
    }
}

/// Build a stable record identifier for a shaped graph query row.
pub(crate) fn graph_query_row_id(row: &Value, row_index: usize) -> String {
    row.as_object()
        .and_then(|object| object.get("node_id").and_then(|value| value.as_str()))
        .map(ToString::to_string)
        .or_else(|| {
            row.as_object()
                .and_then(|object| object.get("id").and_then(|value| value.as_str()))
                .map(ToString::to_string)
        })
        .or_else(|| {
            row.as_object().and_then(|object| {
                object.values().find_map(|value| {
                    value
                        .as_object()
                        .and_then(|nested| nested.get("id"))
                        .and_then(|value| value.as_str())
                        .map(ToString::to_string)
                })
            })
        })
        .unwrap_or_else(|| format!("graph_row_{}", row_index))
}

/// Convert a legacy-shaped graph row into a graph node for Arrow tabular paths.
pub(crate) fn legacy_graph_row_to_node(row: &Value) -> Result<Arc<Node>> {
    let Value::Object(columns) = row else {
        return Err(anyhow!(
            "Legacy graph row projection expected an object row"
        ));
    };

    let id = columns
        .get("node_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Legacy graph row is missing 'node_id'"))?
        .to_string();
    let labels = columns
        .get("label")
        .and_then(Value::as_str)
        .map(|label| vec![label.to_string()])
        .unwrap_or_default();
    let properties = columns
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .iter()
                .map(|(key, value)| (key.clone(), json_value_to_property_value(value)))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    Ok(Arc::new(Node {
        id,
        labels,
        properties,
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    }))
}

fn retain_graph_query_output_columns(row: Value, output_columns: &[String]) -> Result<Value> {
    let Value::Object(object) = row else {
        return Err(anyhow!("Graph query row projection expected an object row"));
    };

    if output_columns.is_empty() {
        return Ok(Value::Object(object));
    }

    let projected = output_columns
        .iter()
        .filter_map(|column| {
            object
                .get(column)
                .cloned()
                .map(|value| (column.clone(), value))
        })
        .collect::<serde_json::Map<_, _>>();
    Ok(Value::Object(projected))
}

fn materialize_legacy_graph_query_row(row: Value) -> Result<Value> {
    let Value::Object(columns) = row else {
        return Err(anyhow!(
            "Legacy graph row materialization expected an object row"
        ));
    };
    let Some(node_value) = columns.values().next() else {
        return Err(anyhow!(
            "Legacy graph row materialization expected a projected node value"
        ));
    };
    let Value::Object(node) = node_value else {
        return Err(anyhow!(
            "Legacy graph row materialization expected a node object"
        ));
    };

    let node_id = node.get("id").cloned().unwrap_or(Value::Null);
    let label = node
        .get("labels")
        .and_then(|labels| labels.as_array())
        .and_then(|labels| labels.first())
        .cloned()
        .unwrap_or(Value::Null);
    let properties = node
        .get("properties")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    Ok(serde_json::json!({
        "node_id": node_id,
        "label": label,
        "properties": properties,
    }))
}

fn json_value_to_property_value(value: &Value) -> PropertyValue {
    let value = match value {
        Value::Null => None,
        Value::Bool(value) => Some(property_value::Value::BoolValue(*value)),
        Value::Number(value) => value
            .as_i64()
            .map(property_value::Value::IntValue)
            .or_else(|| value.as_f64().map(property_value::Value::DoubleValue)),
        Value::String(value) => Some(property_value::Value::StringValue(value.clone())),
        Value::Array(values) => {
            if values.iter().all(Value::is_number) {
                Some(property_value::Value::VectorValue(
                    crate::proto::proximadb_v1::VectorData {
                        values: values
                            .iter()
                            .filter_map(|value| value.as_f64().map(|value| value as f32))
                            .collect(),
                    },
                ))
            } else {
                Some(property_value::Value::ArrayValue(
                    crate::proto::proximadb_v1::PropertyArray {
                        values: values.iter().map(json_value_to_property_value).collect(),
                    },
                ))
            }
        }
        Value::Object(values) => Some(property_value::Value::ObjectValue(
            crate::proto::proximadb_v1::PropertyObject {
                fields: values
                    .iter()
                    .map(|(key, value)| (key.clone(), json_value_to_property_value(value)))
                    .collect(),
            },
        )),
    };

    PropertyValue { value }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphOperationsService;
    use crate::proto::proximadb_v1::{CreateGraphRequest, Node as ProtoNode, property_value};

    async fn seed_graph_service() -> Arc<GraphOperationsService> {
        let service = Arc::new(GraphOperationsService::new());
        service
            .create_graph_collection(CreateGraphRequest {
                graph_id: "social".to_string(),
                name: Some("social".to_string()),
                description: None,
                schema: None,
                storage_config: None,
                engine_config: None,
                access_control: None,
            })
            .await
            .expect("graph should be created");

        service
            .create_node(
                "social",
                ProtoNode {
                    id: "alice".to_string(),
                    labels: vec!["Person".to_string()],
                    properties: HashMap::from([(
                        "name".to_string(),
                        crate::proto::proximadb_v1::PropertyValue {
                            value: Some(property_value::Value::StringValue("Alice".to_string())),
                        },
                    )]),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                },
            )
            .await
            .expect("node should be created");

        service
    }

    #[tokio::test]
    async fn execute_graph_query_expr_materializes_legacy_rows() {
        let service = seed_graph_service().await;
        let expr = GraphQueryExpr {
            graph_name: "social".to_string(),
            normalized_query: "MATCH (n:Person) RETURN n".to_string(),
            output_columns: vec![
                "node_id".to_string(),
                "label".to_string(),
                "properties".to_string(),
            ],
            uses_legacy_node_rows: true,
            max_depth: 0,
        };

        let result = execute_graph_query_expr(service.as_ref(), &expr)
            .await
            .expect("graph query should execute");

        assert_eq!(
            result.rows,
            vec![serde_json::json!({
                "node_id": "alice",
                "label": "Person",
                "properties": { "name": "Alice" }
            })]
        );
    }

    #[tokio::test]
    async fn execute_graph_query_expr_with_start_nodes_preserves_projection() {
        let service = seed_graph_service().await;
        service
            .create_node(
                "social",
                ProtoNode {
                    id: "bob".to_string(),
                    labels: vec!["Person".to_string()],
                    properties: HashMap::from([(
                        "name".to_string(),
                        crate::proto::proximadb_v1::PropertyValue {
                            value: Some(property_value::Value::StringValue("Bob".to_string())),
                        },
                    )]),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                },
            )
            .await
            .expect("second node should be created");
        service
            .create_edge(
                "social",
                crate::proto::proximadb_v1::Edge {
                    id: "knows".to_string(),
                    from_node_id: "alice".to_string(),
                    to_node_id: "bob".to_string(),
                    edge_type: "KNOWS".to_string(),
                    properties: HashMap::new(),
                    weight: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                },
            )
            .await
            .expect("edge should be created");

        let expr = GraphQueryExpr {
            graph_name: "social".to_string(),
            normalized_query: "MATCH (n:Person)-[:KNOWS]->(m:Person) RETURN m.name AS neighbor"
                .to_string(),
            output_columns: vec!["neighbor".to_string()],
            uses_legacy_node_rows: false,
            max_depth: 1,
        };

        let start_node_ids = vec!["alice".to_string()];
        let result = execute_graph_query_expr_with_start_nodes(
            service.as_ref(),
            &expr,
            Some(&start_node_ids),
        )
        .await
        .expect("bound graph query should execute");

        assert_eq!(
            result.rows,
            vec![serde_json::json!({
                "neighbor": "Bob"
            })]
        );
    }

    #[test]
    fn legacy_graph_row_to_node_preserves_properties() {
        let row = serde_json::json!({
            "node_id": "alice",
            "label": "Person",
            "properties": {
                "name": "Alice",
                "embedding": [0.1, 0.2]
            }
        });

        let node = legacy_graph_row_to_node(&row).expect("legacy row should convert");
        assert_eq!(node.id, "alice");
        assert_eq!(node.labels, vec!["Person".to_string()]);
        assert!(node.properties.contains_key("name"));
        assert!(node.properties.contains_key("embedding"));
    }
}
