/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # Graph Database Integration Tests
//!
//! Comprehensive tests for ProximaDB's native graph database functionality,
//! testing the complete stack from API to storage engine.

use proximadb::{
    graph::{Edge, GraphService, Node, OperationMode, PropertyValue},
    proto::proximadb_v1::property_value::Value,
};
use std::collections::HashMap;

/// Test basic CRUD operations on nodes
#[tokio::test]
async fn test_node_crud_operations() {
    let service = GraphService::new();

    // Create a test node
    let node = Node {
        id: &"user_123".to_string().to_string(),
        labels: vec!["User".to_string(), "Person".to_string()],
        properties: HashMap::from([
            (
                "name".to_string(),
                PropertyValue {
                    value: Some(Value::StringValue("Alice Smith".to_string())),
                },
            ),
            (
                "age".to_string(),
                PropertyValue {
                    value: Some(Value::IntValue(29)),
                },
            ),
            (
                "active".to_string(),
                PropertyValue {
                    value: Some(Value::BoolValue(true)),
                },
            ),
        ]),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    // Test create
    let created_node = service.create_node(node.clone()).unwrap();
    assert_eq!(created_node.id, &"user_123".to_string());
    assert_eq!(created_node.labels.len(), 2);
    assert!(created_node.labels.contains(&"User".to_string()));
    assert!(created_node.labels.contains(&"Person".to_string()));

    // Test read
    let retrieved_node = service.get_node(&"user_123".to_string()).unwrap().unwrap();
    assert_eq!(retrieved_node.id, &"user_123".to_string());
    assert_eq!(retrieved_node.properties.len(), 3);

    // Test update
    let mut updated_node = (*retrieved_node).clone();
    updated_node.properties.insert(
        "email".to_string(),
        PropertyValue {
            value: Some(Value::StringValue("alice@example.com".to_string())),
        },
    );

    let updated = service.update_node(updated_node).unwrap();
    assert_eq!(updated.properties.len(), 4);

    // Test delete
    let deleted = service.delete_node(&"user_123".to_string()).unwrap().unwrap();
    assert_eq!(deleted.id, &"user_123".to_string());

    // Verify deletion
    let missing = service.get_node(&"user_123".to_string()).unwrap();
    assert!(missing.is_none());
}

/// Test basic CRUD operations on edges
#[tokio::test]
async fn test_edge_crud_operations() {
    let service = GraphService::new();

    // Create nodes first
    let node1 = Node {
        id: "user_1".to_string(),
        labels: vec!["User".to_string()],
        properties: HashMap::from([(
            "name".to_string(),
            PropertyValue {
                value: Some(Value::StringValue("Alice".to_string())),
            },
        )]),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    let node2 = Node {
        id: "user_2".to_string(),
        labels: vec!["User".to_string()],
        properties: HashMap::from([(
            "name".to_string(),
            PropertyValue {
                value: Some(Value::StringValue("Bob".to_string())),
            },
        )]),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    service.create_node(node1).unwrap();
    service.create_node(node2).unwrap();

    // Create edge
    let edge = Edge {
        id: "friendship_1".to_string(),
        from_node_id: "user_1".to_string(),
        to_node_id: "user_2".to_string(),
        edge_type: "FRIENDS_WITH".to_string(),
        properties: HashMap::from([(
            "since".to_string(),
            PropertyValue {
                value: Some(Value::StringValue("2023-01-01".to_string())),
            },
        )]),
        weight: Some(1.0),
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    // Test create
    let created_edge = service.create_edge(edge.clone()).unwrap();
    assert_eq!(created_edge.id, "friendship_1");
    assert_eq!(created_edge.edge_type, "FRIENDS_WITH");
    assert_eq!(created_edge.weight, Some(1.0));

    // Test read
    let retrieved_edge = service.get_edge("friendship_1").unwrap().unwrap();
    assert_eq!(retrieved_edge.from_node_id, "user_1");
    assert_eq!(retrieved_edge.to_node_id, "user_2");

    // Test update
    let mut updated_edge = (*retrieved_edge).clone();
    updated_edge.weight = Some(2.0);

    let updated = service.update_edge(updated_edge).unwrap();
    assert_eq!(updated.weight, Some(2.0));

    // Test delete
    let deleted = service.delete_edge("friendship_1").unwrap().unwrap();
    assert_eq!(deleted.id, "friendship_1");

    // Verify deletion
    let missing = service.get_edge("friendship_1").unwrap();
    assert!(missing.is_none());
}

/// Test graph traversal operations
#[tokio::test]
async fn test_graph_traversal() {
    let service = GraphService::new();

    // Create a small graph: A -> B -> C
    //                        \-> D /
    let nodes = vec![
        ("A", "Alice"),
        ("B", "Bob"),
        ("C", "Charlie"),
        ("D", "David"),
    ];

    for (id, name) in &nodes {
        let node = Node {
            id: id.to_string(),
            labels: vec!["Person".to_string()],
            properties: HashMap::from([(
                "name".to_string(),
                PropertyValue {
                    value: Some(Value::StringValue(name.to_string())),
                },
            )]),
            embedding: None,
            created_at: None,
            updated_at: None,
        };
        service.create_node(node).unwrap();
    }

    // Create edges
    let edges = vec![
        ("e1", "A", "B", "KNOWS"),
        ("e2", "B", "C", "KNOWS"),
        ("e3", "A", "D", "KNOWS"),
        ("e4", "D", "C", "KNOWS"),
    ];

    for (edge_id, from, to, rel_type) in &edges {
        let edge = Edge {
            id: edge_id.to_string(),
            from_node_id: from.to_string(),
            to_node_id: to.to_string(),
            edge_type: rel_type.to_string(),
            properties: HashMap::new(),
            weight: Some(1.0),
            created_at: None,
            updated_at: None,
        };
        service.create_edge(edge).unwrap();
    }

    // Test neighbor queries
    let neighbors = service.get_neighbors("A").unwrap();
    assert_eq!(neighbors.len(), 2); // B and D
    let neighbor_ids: Vec<String> = neighbors.iter().map(|n| n.id.clone()).collect();
    assert!(neighbor_ids.contains(&"B".to_string()));
    assert!(neighbor_ids.contains(&"D".to_string()));

    // Test traversal
    use proximadb::proto::proximadb_v1::{TraversalAlgorithm, TraversalRequest};

    let traversal_request = TraversalRequest {
        start_node_id: "A".to_string(),
        max_depth: 3,
        edge_types: vec!["KNOWS".to_string()],
        node_labels: vec![],
        filters: vec![],
        algorithm: TraversalAlgorithm::Bfs.into(),
        limit: Some(10),
    };

    let result = service.traverse(traversal_request).await.unwrap();
    assert!(result.nodes.len() >= 1); // At least the start node
    assert_eq!(result.nodes[0].id, "A"); // First node should be start node

    if let Some(stats) = result.stats {
        assert!(stats.nodes_visited >= 1);
    }
}

/// Test node and edge queries
#[tokio::test]
async fn test_node_edge_queries() {
    let service = GraphService::new();

    // Create test data
    let user_nodes = vec![
        ("user_1", "Alice", 25),
        ("user_2", "Bob", 30),
        ("user_3", "Charlie", 25),
    ];

    for (id, name, age) in &user_nodes {
        let node = Node {
            id: id.to_string(),
            labels: vec!["User".to_string(), "Person".to_string()],
            properties: HashMap::from([
                (
                    "name".to_string(),
                    PropertyValue {
                        value: Some(Value::StringValue(name.to_string())),
                    },
                ),
                (
                    "age".to_string(),
                    PropertyValue {
                        value: Some(Value::IntValue(*age)),
                    },
                ),
            ]),
            embedding: None,
            created_at: None,
            updated_at: None,
        };
        service.create_node(node).unwrap();
    }

    // Test node queries
    use proximadb::proto::proximadb_v1::NodeQuery;

    let query = NodeQuery {
        labels: vec!["User".to_string()],
        filters: vec![],
        limit: Some(10),
        offset: Some(0),
    };

    let results = service.query_nodes(query).unwrap();
    assert_eq!(results.len(), 3);

    // Test with specific labels
    let person_query = NodeQuery {
        labels: vec!["Person".to_string()],
        filters: vec![],
        limit: Some(10),
        offset: Some(0),
    };

    let person_results = service.query_nodes(person_query).unwrap();
    assert_eq!(person_results.len(), 3);
}

/// Test batch operations
#[tokio::test]
async fn test_batch_operations() {
    let service = GraphService::new();

    // Batch create nodes
    let nodes = (0..5)
        .map(|i| Node {
            id: format!("batch_node_{}", i),
            labels: vec!["BatchTest".to_string()],
            properties: HashMap::from([(
                "index".to_string(),
                PropertyValue {
                    value: Some(Value::IntValue(i)),
                },
            )]),
            embedding: None,
            created_at: None,
            updated_at: None,
        })
        .collect::<Vec<_>>();

    let created_nodes = service.batch_create_nodes(nodes).unwrap();
    assert_eq!(created_nodes.len(), 5);

    // Batch create edges
    let edges = (0..4)
        .map(|i| Edge {
            id: format!("batch_edge_{}", i),
            from_node_id: format!("batch_node_{}", i),
            to_node_id: format!("batch_node_{}", i + 1),
            edge_type: "NEXT".to_string(),
            properties: HashMap::new(),
            weight: Some(1.0),
            created_at: None,
            updated_at: None,
        })
        .collect::<Vec<_>>();

    let created_edges = service.batch_create_edges(edges).unwrap();
    assert_eq!(created_edges.len(), 4);

    // Verify connections
    let neighbors = service.get_neighbors("batch_node_0").unwrap();
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0].id, "batch_node_1");
}

/// Test graph statistics
#[tokio::test]
async fn test_graph_statistics() {
    let service = GraphService::new();

    // Create some test data
    for i in 0..3 {
        let node = Node {
            id: format!("stats_node_{}", i),
            labels: vec!["StatsTest".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at: None,
            updated_at: None,
        };
        service.create_node(node).unwrap();
    }

    for i in 0..2 {
        let edge = Edge {
            id: format!("stats_edge_{}", i),
            from_node_id: format!("stats_node_{}", i),
            to_node_id: format!("stats_node_{}", i + 1),
            edge_type: "CONNECTS".to_string(),
            properties: HashMap::new(),
            weight: Some(1.0),
            created_at: None,
            updated_at: None,
        };
        service.create_edge(edge).unwrap();
    }

    // Get statistics
    let stats = service.get_stats().unwrap();
    assert_eq!(stats.total_nodes, 3);
    assert_eq!(stats.total_edges, 2);
    assert!(stats.average_degree > 0.0);
}

/// Test operation mode switching
#[tokio::test]
async fn test_operation_modes() {
    let mut service = GraphService::new();
    assert_eq!(service.mode(), OperationMode::Unified);

    // Test graph-only mode
    service.set_mode(OperationMode::GraphOnly);
    assert!(service.graph_enabled());
    assert!(!service.vector_enabled());

    // Should work in graph-only mode
    let node = Node {
        id: "mode_test".to_string(),
        labels: vec!["Test".to_string()],
        properties: HashMap::new(),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    let created = service.create_node(node).unwrap();
    assert_eq!(created.id, "mode_test");

    // Test vector-only mode
    service.set_mode(OperationMode::VectorOnly);
    assert!(!service.graph_enabled());
    assert!(service.vector_enabled());

    // Should fail in vector-only mode
    let node2 = Node {
        id: "mode_test_2".to_string(),
        labels: vec!["Test".to_string()],
        properties: HashMap::new(),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    let result = service.create_node(node2);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Graph operations disabled")
    );
}

/// Test concurrent access
#[tokio::test]
async fn test_concurrent_access() {
    use std::sync::Arc;
    use tokio::task::JoinSet;

    let service = Arc::new(GraphService::new());
    let mut handles = JoinSet::new();

    // Spawn multiple concurrent tasks
    for i in 0..10 {
        let service_clone = service.clone();
        handles.spawn(async move {
            let node = Node {
                id: format!("concurrent_node_{}", i),
                labels: vec!["Concurrent".to_string()],
                properties: HashMap::from([(
                    "thread_id".to_string(),
                    PropertyValue {
                        value: Some(Value::IntValue(i)),
                    },
                )]),
                embedding: None,
                created_at: None,
                updated_at: None,
            };

            service_clone.create_node(node).unwrap();

            // Also create an edge if not the first node
            if i > 0 {
                let edge = Edge {
                    id: format!("concurrent_edge_{}", i),
                    from_node_id: format!("concurrent_node_{}", i - 1),
                    to_node_id: format!("concurrent_node_{}", i),
                    edge_type: "NEXT".to_string(),
                    properties: HashMap::new(),
                    weight: Some(1.0),
                    created_at: None,
                    updated_at: None,
                };

                service_clone.create_edge(edge).unwrap();
            }

            i
        });
    }

    // Wait for all tasks to complete
    let mut completed = 0;
    while let Some(result) = handles.join_next().await {
        result.unwrap();
        completed += 1;
    }

    assert_eq!(completed, 10);

    // Verify all nodes were created
    let stats = service.get_stats().unwrap();
    assert!(stats.total_nodes >= 10);
    assert!(stats.total_edges >= 9);
}
