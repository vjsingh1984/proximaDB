//! Compatibility shim — implementation now lives in `proximadb-query`.
pub use proximadb_query::graph_runtime::*;

#[cfg(test)]
mod tests {
    use crate::graph::GraphOperationsService;
    use crate::graph::{Edge, Node as ProtoNode, property_value};
    use crate::proto::proximadb_v1::CreateGraphRequest;
    use proximadb_graph_query::declarative::LoweredGraphQuery as GraphQueryExpr;
    use proximadb_graph_subset::{
        graph_query_row_id, legacy_graph_row_to_node, shape_graph_query_row,
    };
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::*;

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
                        crate::graph::PropertyValue {
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
                        crate::graph::PropertyValue {
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
                Edge {
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

    #[test]
    fn shape_graph_query_row_retains_declared_columns_only() {
        let row = serde_json::json!({
            "neighbor": "Bob",
            "company": "Acme"
        });

        let shaped = shape_graph_query_row(row, false, &["neighbor".to_string()])
            .expect("row projection should succeed");

        assert_eq!(shaped, serde_json::json!({ "neighbor": "Bob" }));
    }

    #[test]
    fn graph_query_row_id_falls_back_across_supported_shapes() {
        assert_eq!(
            graph_query_row_id(&serde_json::json!({ "node_id": "alice" }), 0),
            "alice"
        );
        assert_eq!(
            graph_query_row_id(&serde_json::json!({ "id": "edge_1" }), 1),
            "edge_1"
        );
        assert_eq!(
            graph_query_row_id(&serde_json::json!({ "node": { "id": "nested" } }), 2),
            "nested"
        );
        assert_eq!(
            graph_query_row_id(&serde_json::json!({ "name": "anonymous" }), 3),
            "graph_row_3"
        );
    }

    #[test]
    fn legacy_graph_row_to_node_requires_node_id() {
        let error = legacy_graph_row_to_node(&serde_json::json!({
            "label": "Person",
            "properties": { "name": "Alice" }
        }))
        .expect_err("legacy row without node_id should fail");

        assert!(error.to_string().contains("missing 'node_id'"));
    }
}
