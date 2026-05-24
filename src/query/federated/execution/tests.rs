#[cfg(test)]
mod tests {
    use crate::graph::GraphOperationsService;
    use crate::proto::proximadb_v1::{
        CreateGraphRequest, Node, Node as ProtoNode, PropertyValue, SqlObject, SqlValue,
        VectorData, property_value, sql_value,
    };
    use std::collections::HashMap;

    // Import required types from parent module
    use crate::query::federated::execution::{ExecutionConfig, ExecutionResult, FederatedExecutor};
    use crate::storage::MultiModalStorageFacade;
    use crate::storage::traits::DocumentRecord;
    use arrow::array::{ArrayRef, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use std::sync::Arc;

    #[test]
    fn test_execution_result_empty() {
        let result = ExecutionResult::empty();
        assert_eq!(result.row_count(), 0);
        assert!(result.batches.is_empty());
    }

    #[test]
    fn test_execution_config_default() {
        let config = ExecutionConfig::default();
        assert_eq!(config.batch_size, 10_000);
        assert!(config.parallel_execution);
    }

    #[test]
    fn test_source_alias_matching_respects_identifier_quoting() {
        assert!(FederatedExecutor::source_alias_matches(
            "RightAlias",
            "rightalias"
        ));
        assert!(FederatedExecutor::source_alias_matches(
            "\"RightAlias\"",
            "\"RightAlias\""
        ));
        assert!(!FederatedExecutor::source_alias_matches(
            "\"RightAlias\"",
            "\"RIGHTALIAS\""
        ));
        assert!(!FederatedExecutor::source_alias_matches(
            "\"RightAlias\"",
            "RightAlias"
        ));
    }

    #[tokio::test]
    async fn test_executor_creation() {
        let storage = Arc::new(MultiModalStorageFacade::new());
        let executor = FederatedExecutor::new(storage);
        assert!(executor.config.parallel_execution);
    }

    async fn seed_service_backed_graph() -> Arc<GraphOperationsService> {
        let service = Arc::new(GraphOperationsService::new());
        for graph_id in ["left", "right"] {
            service
                .create_graph_collection(CreateGraphRequest {
                    graph_id: graph_id.to_string(),
                    name: Some(graph_id.to_string()),
                    description: None,
                    schema: None,
                    storage_config: None,
                    engine_config: None,
                    access_control: None,
                })
                .await
                .expect("graph creation should succeed");
        }

        service
            .create_node(
                "left",
                ProtoNode {
                    id: "left-person".to_string(),
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
                },
            )
            .await
            .expect("left graph node should be created");
        service
            .create_node(
                "right",
                ProtoNode {
                    id: "right-person".to_string(),
                    labels: vec!["Person".to_string()],
                    properties: HashMap::from([
                        (
                            "name".to_string(),
                            PropertyValue {
                                value: Some(property_value::Value::StringValue("Bob".to_string())),
                            },
                        ),
                        (
                            "embedding".to_string(),
                            PropertyValue {
                                value: Some(property_value::Value::VectorValue(VectorData {
                                    values: vec![0.4, 0.6],
                                })),
                            },
                        ),
                    ]),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                },
            )
            .await
            .expect("right graph node should be created");

        service
    }

    #[tokio::test]
    async fn test_graph_query_uses_service_target_and_legacy_node_shape() {
        let graph_service = seed_service_backed_graph().await;
        let graph_store = Arc::new(
            crate::storage::multimodal::stores::GraphStore::new(Default::default())
                .with_service(graph_service),
        );
        let storage = Arc::new(MultiModalStorageFacade::new().with_graph_store(graph_store));
        let executor = FederatedExecutor::new(storage);

        let result = executor
            .execute_graph_traversal("MATCH (n:Person) FROM right RETURN n", None, Some("g"))
            .await
            .expect("service-backed graph query should execute");

        assert_eq!(result.row_count(), 1);
        let batch = &result.batches[0];
        let fields: Vec<String> = batch
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect();
        assert_eq!(fields, vec!["node_id", "label", "properties", "embedding"]);

        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("node_id should be utf8");
        assert_eq!(ids.value(0), "right-person");

        let vector = executor
            .resolve_vector_from_outer_column(batch, 0, "g", "properties.embedding")
            .expect("legacy properties.embedding path should still resolve");
        assert_eq!(vector, vec![0.4, 0.6]);
    }

    #[tokio::test]
    async fn test_graph_query_uses_projected_columns_for_scalar_subset_queries() {
        let graph_service = seed_service_backed_graph().await;
        let graph_store = Arc::new(
            crate::storage::multimodal::stores::GraphStore::new(Default::default())
                .with_service(graph_service),
        );
        let storage = Arc::new(MultiModalStorageFacade::new().with_graph_store(graph_store));
        let executor = FederatedExecutor::new(storage);

        let result = executor
            .execute_graph_traversal(
                "MATCH (n:Person) FROM right RETURN n.name AS person_name",
                None,
                None,
            )
            .await
            .expect("scalar graph projection should execute");

        let batch = &result.batches[0];
        let fields: Vec<String> = batch
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect();
        assert_eq!(fields, vec!["person_name"]);

        let names = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("projected graph column should be utf8");
        assert_eq!(names.value(0), "Bob");
    }

    #[tokio::test]
    async fn test_graph_query_with_bound_start_nodes_uses_shared_subset_projection() {
        let graph_service = Arc::new(GraphOperationsService::new());
        graph_service
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
            .expect("graph creation should succeed");
        for (id, name) in [("alice", "Alice"), ("bob", "Bob")] {
            graph_service
                .create_node(
                    "social",
                    ProtoNode {
                        id: id.to_string(),
                        labels: vec!["Person".to_string()],
                        properties: HashMap::from([(
                            "name".to_string(),
                            PropertyValue {
                                value: Some(property_value::Value::StringValue(name.to_string())),
                            },
                        )]),
                        embedding: None,
                        created_at_ms: 0,
                        updated_at_ms: 0,
                    },
                )
                .await
                .expect("graph node should be created");
        }
        graph_service
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
            .expect("graph edge should be created");

        let graph_store = Arc::new(
            crate::storage::multimodal::stores::GraphStore::new(Default::default())
                .with_service(graph_service),
        );
        let storage = Arc::new(MultiModalStorageFacade::new().with_graph_store(graph_store));
        let executor = FederatedExecutor::new(storage);

        let start_nodes = vec!["alice".to_string()];
        let result = executor
            .execute_graph_traversal(
                "MATCH (n:Person)-[:KNOWS]->(m:Person) FROM social RETURN m.name AS neighbor",
                Some(&start_nodes),
                None,
            )
            .await
            .expect("bound graph query should execute");

        let batch = &result.batches[0];
        let fields: Vec<String> = batch
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect();
        assert_eq!(fields, vec!["neighbor"]);

        let names = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("projected graph column should be utf8");
        assert_eq!(names.value(0), "Bob");
    }

    #[test]
    fn test_document_batch_exposes_nested_native_vector_columns() {
        let documents = vec![
            DocumentRecord {
                id: "doc-1".to_string(),
                document: SqlObject {
                    fields: HashMap::from([(
                        "profile".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::ObjectValue(SqlObject {
                                fields: HashMap::from([(
                                    "embedding".to_string(),
                                    SqlValue {
                                        value: Some(sql_value::Value::ArrayValue(
                                            crate::proto::proximadb_v1::SqlArray {
                                                values: vec![
                                                    SqlValue {
                                                        value: Some(sql_value::Value::NumberValue(
                                                            0.1,
                                                        )),
                                                    },
                                                    SqlValue {
                                                        value: Some(sql_value::Value::NumberValue(
                                                            0.2,
                                                        )),
                                                    },
                                                ],
                                            },
                                        )),
                                    },
                                )]),
                            })),
                        },
                    )]),
                },
                version: 1,
                created_at_ns: 1,
                updated_at_ns: 1,
            },
            DocumentRecord {
                id: "doc-2".to_string(),
                document: SqlObject {
                    fields: HashMap::from([(
                        "profile".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::ObjectValue(SqlObject {
                                fields: HashMap::from([(
                                    "embedding".to_string(),
                                    SqlValue {
                                        value: Some(sql_value::Value::ArrayValue(
                                            crate::proto::proximadb_v1::SqlArray {
                                                values: vec![
                                                    SqlValue {
                                                        value: Some(sql_value::Value::NumberValue(
                                                            0.3,
                                                        )),
                                                    },
                                                    SqlValue {
                                                        value: Some(sql_value::Value::NumberValue(
                                                            0.4,
                                                        )),
                                                    },
                                                ],
                                            },
                                        )),
                                    },
                                )]),
                            })),
                        },
                    )]),
                },
                version: 1,
                created_at_ns: 2,
                updated_at_ns: 2,
            },
        ];

        let batch = FederatedExecutor::build_document_record_batch(&documents, None)
            .expect("document batch should build");
        let executor = FederatedExecutor::new(Arc::new(MultiModalStorageFacade::new()));

        assert!(
            batch
                .schema()
                .field_with_name("document.profile.embedding")
                .is_ok(),
            "nested embedding column should be materialized natively"
        );
        let vector = executor
            .resolve_vector_from_outer_column(&batch, 1, "p", "document.profile.embedding")
            .expect("nested vector path should resolve from Arrow");
        assert_eq!(vector, vec![0.3, 0.4]);
    }

    #[test]
    fn test_document_nested_vector_path_beats_leaf_name_collision() {
        let documents = vec![DocumentRecord {
            id: "doc-1".to_string(),
            document: SqlObject {
                fields: HashMap::from([
                    (
                        "embedding".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::ArrayValue(
                                crate::proto::proximadb_v1::SqlArray {
                                    values: vec![
                                        SqlValue {
                                            value: Some(sql_value::Value::NumberValue(9.0)),
                                        },
                                        SqlValue {
                                            value: Some(sql_value::Value::NumberValue(8.0)),
                                        },
                                    ],
                                },
                            )),
                        },
                    ),
                    (
                        "profile".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::ObjectValue(SqlObject {
                                fields: HashMap::from([(
                                    "embedding".to_string(),
                                    SqlValue {
                                        value: Some(sql_value::Value::ArrayValue(
                                            crate::proto::proximadb_v1::SqlArray {
                                                values: vec![
                                                    SqlValue {
                                                        value: Some(sql_value::Value::NumberValue(
                                                            0.1,
                                                        )),
                                                    },
                                                    SqlValue {
                                                        value: Some(sql_value::Value::NumberValue(
                                                            0.2,
                                                        )),
                                                    },
                                                ],
                                            },
                                        )),
                                    },
                                )]),
                            })),
                        },
                    ),
                ]),
            },
            version: 1,
            created_at_ns: 1,
            updated_at_ns: 1,
        }];

        let batch = FederatedExecutor::build_document_record_batch(&documents, None)
            .expect("document batch should build");
        let executor = FederatedExecutor::new(Arc::new(MultiModalStorageFacade::new()));

        let vector = executor
            .resolve_vector_from_outer_column(&batch, 0, "p", "document.profile.embedding")
            .expect("nested vector path should resolve from exact Arrow column");
        assert_eq!(vector, vec![0.1, 0.2]);
    }

    #[test]
    fn test_graph_batch_exposes_native_vector_property_columns() {
        let nodes = vec![
            Arc::new(Node {
                id: "node-1".to_string(),
                labels: vec!["Entity".to_string()],
                properties: HashMap::from([(
                    "embedding".to_string(),
                    PropertyValue {
                        value: Some(property_value::Value::VectorValue(VectorData {
                            values: vec![0.9, 0.1],
                        })),
                    },
                )]),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            }),
            Arc::new(Node {
                id: "node-2".to_string(),
                labels: vec!["Entity".to_string()],
                properties: HashMap::from([(
                    "embedding".to_string(),
                    PropertyValue {
                        value: Some(property_value::Value::VectorValue(VectorData {
                            values: vec![0.2, 0.8],
                        })),
                    },
                )]),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            }),
        ];

        let batch = FederatedExecutor::build_graph_node_batch(&nodes, None)
            .expect("graph batch should build");
        let executor = FederatedExecutor::new(Arc::new(MultiModalStorageFacade::new()));

        assert!(
            batch.schema().field_with_name("embedding").is_ok(),
            "graph vector property should become a native Arrow column"
        );
        let vector = executor
            .resolve_vector_from_outer_column(&batch, 0, "g", "properties.embedding")
            .expect("graph vector property should resolve from Arrow");
        assert_eq!(vector, vec![0.9, 0.1]);
    }

    #[test]
    fn test_graph_nested_vector_path_beats_leaf_name_collision() {
        let nodes = vec![Arc::new(Node {
            id: "node-1".to_string(),
            labels: vec!["Entity".to_string()],
            properties: HashMap::from([
                (
                    "embedding".to_string(),
                    PropertyValue {
                        value: Some(property_value::Value::VectorValue(VectorData {
                            values: vec![7.0, 6.0],
                        })),
                    },
                ),
                (
                    "profile".to_string(),
                    PropertyValue {
                        value: Some(property_value::Value::ObjectValue(
                            crate::proto::proximadb_v1::PropertyObject {
                                fields: HashMap::from([(
                                    "embedding".to_string(),
                                    PropertyValue {
                                        value: Some(property_value::Value::VectorValue(
                                            VectorData {
                                                values: vec![0.9, 0.1],
                                            },
                                        )),
                                    },
                                )]),
                            },
                        )),
                    },
                ),
            ]),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        })];

        let batch = FederatedExecutor::build_graph_node_batch(&nodes, None)
            .expect("graph batch should build");
        let executor = FederatedExecutor::new(Arc::new(MultiModalStorageFacade::new()));

        let vector = executor
            .resolve_vector_from_outer_column(&batch, 0, "g", "properties.profile.embedding")
            .expect("nested graph vector path should resolve from exact Arrow column");
        assert_eq!(vector, vec![0.9, 0.1]);
    }

    #[test]
    fn test_joined_batch_resolves_graph_vector_from_renamed_native_column() {
        let documents = vec![DocumentRecord {
            id: "doc-1".to_string(),
            document: SqlObject {
                fields: HashMap::from([(
                    "embedding".to_string(),
                    SqlValue {
                        value: Some(sql_value::Value::ArrayValue(
                            crate::proto::proximadb_v1::SqlArray {
                                values: vec![
                                    SqlValue {
                                        value: Some(sql_value::Value::NumberValue(9.0)),
                                    },
                                    SqlValue {
                                        value: Some(sql_value::Value::NumberValue(8.0)),
                                    },
                                ],
                            },
                        )),
                    },
                )]),
            },
            version: 1,
            created_at_ns: 1,
            updated_at_ns: 1,
        }];
        let nodes = vec![Arc::new(Node {
            id: "node-1".to_string(),
            labels: vec!["Entity".to_string()],
            properties: HashMap::from([(
                "embedding".to_string(),
                PropertyValue {
                    value: Some(property_value::Value::VectorValue(VectorData {
                        values: vec![0.9, 0.1],
                    })),
                },
            )]),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        })];

        let document_batch = FederatedExecutor::build_document_record_batch(&documents, None)
            .expect("document batch should build");
        let graph_batch = FederatedExecutor::build_graph_node_batch(&nodes, None)
            .expect("graph batch should build");
        let executor = FederatedExecutor::new(Arc::new(MultiModalStorageFacade::new()));
        let joined = executor
            .join_batches(&document_batch, &graph_batch, &[Some(0)], &[Some(0)])
            .expect("joined batch should build");

        assert!(
            joined.schema().field_with_name("right_embedding").is_ok(),
            "graph vector column should be renamed on collision"
        );

        let vector = executor
            .resolve_vector_from_outer_column(&joined, 0, "g", "properties.embedding")
            .expect("graph vector should resolve from renamed native column");
        assert_eq!(vector, vec![0.9, 0.1]);
    }

    #[test]
    fn test_joined_batch_resolves_document_vector_from_renamed_native_column() {
        let nodes = vec![Arc::new(Node {
            id: "node-1".to_string(),
            labels: vec!["Entity".to_string()],
            properties: HashMap::from([(
                "embedding".to_string(),
                PropertyValue {
                    value: Some(property_value::Value::VectorValue(VectorData {
                        values: vec![7.0, 6.0],
                    })),
                },
            )]),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        })];
        let documents = vec![DocumentRecord {
            id: "doc-1".to_string(),
            document: SqlObject {
                fields: HashMap::from([(
                    "embedding".to_string(),
                    SqlValue {
                        value: Some(sql_value::Value::ArrayValue(
                            crate::proto::proximadb_v1::SqlArray {
                                values: vec![
                                    SqlValue {
                                        value: Some(sql_value::Value::NumberValue(0.1)),
                                    },
                                    SqlValue {
                                        value: Some(sql_value::Value::NumberValue(0.2)),
                                    },
                                ],
                            },
                        )),
                    },
                )]),
            },
            version: 1,
            created_at_ns: 1,
            updated_at_ns: 1,
        }];

        let graph_batch = FederatedExecutor::build_graph_node_batch(&nodes, None)
            .expect("graph batch should build");
        let document_batch = FederatedExecutor::build_document_record_batch(&documents, None)
            .expect("document batch should build");
        let executor = FederatedExecutor::new(Arc::new(MultiModalStorageFacade::new()));
        let joined = executor
            .join_batches(&graph_batch, &document_batch, &[Some(0)], &[Some(0)])
            .expect("joined batch should build");

        assert!(
            joined.schema().field_with_name("right_embedding").is_ok(),
            "document vector column should be renamed on collision"
        );

        let vector = executor
            .resolve_vector_from_outer_column(&joined, 0, "p", "document.embedding")
            .expect("document vector should resolve from renamed native column");
        assert_eq!(vector, vec![0.1, 0.2]);
    }

    #[test]
    fn test_legacy_direct_document_path_column_resolves_utf8_vector() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "document.profile.embedding",
            DataType::Utf8,
            false,
        )]));
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(StringArray::from(vec![Some("[0.1,0.2]")])) as ArrayRef],
        )
        .expect("record batch should build");
        let executor = FederatedExecutor::new(Arc::new(MultiModalStorageFacade::new()));

        let vector = executor
            .resolve_vector_from_outer_column(&batch, 0, "p", "document.profile.embedding")
            .expect("legacy direct document path column should resolve");
        assert_eq!(vector, vec![0.1, 0.2]);
    }

    #[test]
    fn test_legacy_direct_graph_path_column_resolves_utf8_vector() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "properties.embedding",
            DataType::Utf8,
            false,
        )]));
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(StringArray::from(vec![Some("[0.9,0.1]")])) as ArrayRef],
        )
        .expect("record batch should build");
        let executor = FederatedExecutor::new(Arc::new(MultiModalStorageFacade::new()));

        let vector = executor
            .resolve_vector_from_outer_column(&batch, 0, "g", "properties.embedding")
            .expect("legacy direct graph path column should resolve");
        assert_eq!(vector, vec![0.9, 0.1]);
    }

    #[test]
    fn test_root_projection_does_not_leak_native_vector_columns() {
        let documents = vec![DocumentRecord {
            id: "doc-1".to_string(),
            document: SqlObject {
                fields: HashMap::from([(
                    "embedding".to_string(),
                    SqlValue {
                        value: Some(sql_value::Value::ArrayValue(
                            crate::proto::proximadb_v1::SqlArray {
                                values: vec![
                                    SqlValue {
                                        value: Some(sql_value::Value::NumberValue(0.1)),
                                    },
                                    SqlValue {
                                        value: Some(sql_value::Value::NumberValue(0.2)),
                                    },
                                ],
                            },
                        )),
                    },
                )]),
            },
            version: 1,
            created_at_ns: 1,
            updated_at_ns: 1,
        }];
        let batch = FederatedExecutor::build_document_record_batch(&documents, None)
            .expect("document batch should build");
        let executor = FederatedExecutor::new(Arc::new(MultiModalStorageFacade::new()));

        let stripped = executor
            .project_result_to_output_columns(
                ExecutionResult::from_batch(batch.clone()),
                &["id".to_string(), "document".to_string()],
                false,
            )
            .expect("root projection should strip internal native vectors");
        let preserved = executor
            .project_result_to_output_columns(
                ExecutionResult::from_batch(batch),
                &["id".to_string(), "document".to_string()],
                true,
            )
            .expect("intermediate projection should preserve native vectors");

        let stripped_fields: Vec<String> = stripped
            .schema
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect();
        let preserved_fields: Vec<String> = preserved
            .schema
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect();

        assert_eq!(stripped_fields, vec!["id", "document"]);
        assert_eq!(preserved_fields, vec!["id", "document", "embedding"]);
    }
}
