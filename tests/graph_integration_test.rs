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

// mod helpers;
// use helpers::graph_test_utils::*;
use proximadb::{
    graph::{Edge, Node, PropertyValue, service::GraphOperationsService},
    proto::proximadb_v1::{
        CompressionAlgorithm, GraphStorageConfig, NodeQuery, PropertyFilter, TraversalAlgorithm,
        TraversalRequest, property_value::Value,
    },
};
use std::collections::HashMap;
use std::sync::Arc;

const TEST_GRAPH_ID: &str = "test_graph";

/// Helper function to ensure the test graph collection exists
async fn ensure_test_graph_exists(service: &GraphOperationsService) {
    // Clean up any existing test data from previous runs
    let test_dir = "/tmp/proximadb-test-graph";
    let _ = std::fs::remove_dir_all(test_dir);
    std::fs::create_dir_all(test_dir).unwrap();

    let create_request = proximadb::proto::proximadb_v1::CreateGraphRequest {
        graph_id: TEST_GRAPH_ID.to_string(),
        name: Some("Test Graph Collection".to_string()),
        description: Some("Test graph for integration testing".to_string()),
        schema: None,
        storage_config: Some(GraphStorageConfig {
            engine_type: "ORION".to_string(),
            base_url: test_dir.to_string(),
            compression: CompressionAlgorithm::CompressionSnappy as i32,
            enable_wal: true,
            snapshot_interval_hours: 24,
            engine_specific_config: HashMap::new(),
        }),
        engine_config: None,
        access_control: None,
    };

    // Create the graph collection (ignore if it already exists)
    let _ = service.create_graph_collection(create_request).await;
}

/// Test basic CRUD operations on nodes
#[tokio::test]
async fn test_node_crud_operations() {
    let service = Arc::new(GraphOperationsService::new());
    ensure_test_graph_exists(&service).await;

    // Create a test node
    let node = Node {
        id: "user_123".to_string(),
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
    let created_node = service
        .create_node(TEST_GRAPH_ID, node.clone())
        .await
        .unwrap();
    assert_eq!(created_node.id, "user_123".to_string());
    assert_eq!(created_node.labels.len(), 2);
    assert!(created_node.labels.contains(&"User".to_string()));
    assert!(created_node.labels.contains(&"Person".to_string()));

    // Test read
    let retrieved_node = service
        .get_node(TEST_GRAPH_ID, &"user_123".to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retrieved_node.id, "user_123".to_string());
    assert_eq!(retrieved_node.properties.len(), 3);

    // Test update
    let mut updated_node = (*retrieved_node).clone();
    updated_node.properties.insert(
        "email".to_string(),
        PropertyValue {
            value: Some(Value::StringValue("alice@example.com".to_string())),
        },
    );

    let updated = service
        .update_node(TEST_GRAPH_ID, updated_node)
        .await
        .unwrap();
    assert_eq!(updated.properties.len(), 4);

    // Test delete
    let deleted = service
        .delete_node(TEST_GRAPH_ID, &"user_123".to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deleted.id, "user_123".to_string());

    // Verify deletion
    let missing = service
        .get_node(TEST_GRAPH_ID, &"user_123".to_string())
        .await
        .unwrap();
    assert!(missing.is_none());
}

/// Test basic CRUD operations on edges
#[tokio::test]
async fn test_edge_crud_operations() {
    let service = Arc::new(GraphOperationsService::new());
    ensure_test_graph_exists(&service).await;

    // Create nodes first
    let node1 = Node {
        id: "user_1".to_string(),
        labels: vec!["User".to_string()],
        properties: HashMap::new(),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    let node2 = Node {
        id: "user_2".to_string(),
        labels: vec!["User".to_string()],
        properties: HashMap::new(),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    service.create_node(TEST_GRAPH_ID, node1).await.unwrap();
    service.create_node(TEST_GRAPH_ID, node2).await.unwrap();

    // Create an edge
    let edge = Edge {
        id: "friendship_1".to_string(),
        from_node_id: "user_1".to_string(),
        to_node_id: "user_2".to_string(),
        edge_type: "FRIENDS_WITH".to_string(),
        properties: HashMap::from([(
            "since".to_string(),
            PropertyValue {
                value: Some(Value::StringValue("2020-01-01".to_string())),
            },
        )]),
        weight: Some(1.0),
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    // Test create
    let created_edge = service
        .create_edge(TEST_GRAPH_ID, edge.clone())
        .await
        .unwrap();
    assert_eq!(created_edge.id, "friendship_1".to_string());
    assert_eq!(created_edge.edge_type, "FRIENDS_WITH".to_string());

    // Test read
    let retrieved_edge = service
        .get_edge(TEST_GRAPH_ID, &"friendship_1".to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retrieved_edge.from_node_id, "user_1".to_string());
    assert_eq!(retrieved_edge.to_node_id, "user_2".to_string());

    // Test update
    let mut updated_edge = (*retrieved_edge).clone();
    updated_edge.weight = Some(2.0);

    let updated = service
        .update_edge(TEST_GRAPH_ID, updated_edge)
        .await
        .unwrap();
    assert_eq!(updated.weight, Some(2.0));

    // Test delete
    let deleted = service
        .delete_edge(TEST_GRAPH_ID, &"friendship_1".to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deleted.id, "friendship_1".to_string());

    // Verify deletion
    let missing = service
        .get_edge(TEST_GRAPH_ID, &"friendship_1".to_string())
        .await
        .unwrap();
    assert!(missing.is_none());
}

/// Test graph traversal and neighbor operations
#[tokio::test]
async fn test_graph_traversal() {
    let service = Arc::new(GraphOperationsService::new());
    ensure_test_graph_exists(&service).await;

    // Create a small graph: A -> B -> C -> D
    //                            -> E
    println!("🔍 DEBUG: Creating nodes...");
    for id in ["A", "B", "C", "D", "E"] {
        let node = Node {
            id: id.to_string(),
            labels: vec!["TestNode".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let created = service.create_node(TEST_GRAPH_ID, node).await.unwrap();
        println!("  ✅ Created node: {}", created.id);
    }

    // Create edges
    let edges = vec![
        ("edge1", "A", "B"),
        ("edge2", "B", "C"),
        ("edge3", "C", "D"),
        ("edge4", "B", "E"),
    ];

    println!("🔍 DEBUG: Creating edges...");
    for (id, from, to) in edges {
        let edge = Edge {
            id: id.to_string(),
            from_node_id: from.to_string(),
            to_node_id: to.to_string(),
            edge_type: "CONNECTS".to_string(),
            properties: HashMap::new(),
            weight: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let created = service.create_edge(TEST_GRAPH_ID, edge).await.unwrap();
        println!(
            "  ✅ Created edge: {} ({} -> {})",
            created.id, created.from_node_id, created.to_node_id
        );
    }

    // Test get neighbors
    println!("🔍 DEBUG: Getting neighbors of A...");
    let neighbors = service
        .get_neighbors(TEST_GRAPH_ID, &"A".to_string())
        .await
        .unwrap();
    println!(
        "  Found {} neighbors: {:?}",
        neighbors.len(),
        neighbors.iter().map(|n| &n.id).collect::<Vec<_>>()
    );
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0].id, "B".to_string());

    // Test traversal
    println!("🔍 DEBUG: Starting BFS traversal from A with max_depth=2...");
    let traversal_request = TraversalRequest {
        graph_id: TEST_GRAPH_ID.to_string(),
        start_node_id: "A".to_string(),
        algorithm: TraversalAlgorithm::Bfs as i32,
        max_depth: 2,
        edge_types: vec!["CONNECTS".to_string()],
        node_labels: vec![],
        filters: vec![],
        limit: Some(10),
        timeout_ms: None,
        max_frontier: None,
    };

    let result = service
        .traverse(TEST_GRAPH_ID, traversal_request)
        .await
        .unwrap();

    println!("🔍 DEBUG: Traversal result:");
    println!("  Nodes found: {}", result.nodes.len());
    for node in &result.nodes {
        println!("    - Node: {}", node.id);
    }
    println!("  Edges found: {}", result.edges.len());
    for edge in &result.edges {
        println!("    - Edge: {} -> {}", edge.from_node_id, edge.to_node_id);
    }
    if let Some(stats) = &result.stats {
        println!(
            "  Stats: nodes_visited={}, edges_traversed={}, max_depth={}",
            stats.nodes_visited, stats.edges_traversed, stats.max_depth_reached
        );
    }

    // Should find A, B, C, E (depth 0, 1, 2)
    println!("🔍 DEBUG: Checking assertion (expecting >= 3 nodes)...");
    assert!(
        result.nodes.len() >= 3,
        "Expected at least 3 nodes, got {}",
        result.nodes.len()
    );

    // Verify nodes are found
    let node_ids: Vec<String> = result.nodes.iter().map(|n| n.id.clone()).collect();
    println!("🔍 DEBUG: Node IDs: {:?}", node_ids);
    assert!(node_ids.contains(&"A".to_string()), "Node A not found");
    assert!(node_ids.contains(&"B".to_string()), "Node B not found");
}

/// Test node querying with filters
#[tokio::test]
async fn test_node_query() {
    let service = Arc::new(GraphOperationsService::new());
    ensure_test_graph_exists(&service).await;

    // Create test nodes with different properties
    for i in 0..5 {
        let node = Node {
            id: format!("node_{}", i),
            labels: vec!["Person".to_string()],
            properties: HashMap::from([
                (
                    "age".to_string(),
                    PropertyValue {
                        value: Some(Value::IntValue(20 + i * 5)),
                    },
                ),
                (
                    "city".to_string(),
                    PropertyValue {
                        value: Some(Value::StringValue(if i % 2 == 0 {
                            "New York".to_string()
                        } else {
                            "San Francisco".to_string()
                        })),
                    },
                ),
            ]),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        service.create_node(TEST_GRAPH_ID, node).await.unwrap();
    }

    // Query by label
    let query = NodeQuery {
        graph_id: TEST_GRAPH_ID.to_string(),
        labels: vec!["Person".to_string()],
        filters: vec![],
        limit: Some(10),
        offset: Some(0),
        continuation_token: None,
    };

    let results = service.query_nodes(TEST_GRAPH_ID, query).await.unwrap();
    assert_eq!(results.len(), 5);

    // Query with property filter
    let person_query = NodeQuery {
        graph_id: TEST_GRAPH_ID.to_string(),
        labels: vec!["Person".to_string()],
        filters: vec![PropertyFilter {
            key: "city".to_string(),
            operator: proximadb::proto::proximadb_v1::PropertyFilterOperator::Equals as i32,
            value: Some(PropertyValue {
                value: Some(Value::StringValue("New York".to_string())),
            }),
        }],
        limit: Some(10),
        offset: Some(0),
        continuation_token: None,
    };

    let person_results = service
        .query_nodes(TEST_GRAPH_ID, person_query)
        .await
        .unwrap();
    assert!(person_results.len() >= 2); // nodes 0, 2, 4
}

/// Test batch operations
#[tokio::test]
async fn test_batch_operations() {
    let service = Arc::new(GraphOperationsService::new());
    ensure_test_graph_exists(&service).await;

    // Batch create nodes
    let mut nodes = Vec::new();
    for i in 0..5 {
        nodes.push(Node {
            id: format!("batch_node_{}", i),
            labels: vec!["BatchNode".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        });
    }

    let created_nodes = service
        .batch_create_nodes(TEST_GRAPH_ID, nodes)
        .await
        .unwrap();
    assert_eq!(created_nodes.len(), 5);

    // Batch create edges
    let mut edges = Vec::new();
    for i in 0..4 {
        edges.push(Edge {
            id: format!("batch_edge_{}", i),
            from_node_id: format!("batch_node_{}", i),
            to_node_id: format!("batch_node_{}", i + 1),
            edge_type: "NEXT".to_string(),
            properties: HashMap::new(),
            weight: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        });
    }

    let created_edges = service
        .batch_create_edges(TEST_GRAPH_ID, edges)
        .await
        .unwrap();
    assert_eq!(created_edges.len(), 4);

    // Verify the chain
    let neighbors = service
        .get_neighbors(TEST_GRAPH_ID, &"batch_node_0".to_string())
        .await
        .unwrap();
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0].id, "batch_node_1".to_string());
}

/// Test graph statistics
#[tokio::test]
async fn test_graph_stats() {
    let service = Arc::new(GraphOperationsService::new());
    ensure_test_graph_exists(&service).await;

    // Create some nodes and edges
    println!("🔍 DEBUG: Creating nodes for stats test...");
    for i in 0..10 {
        let node = Node {
            id: format!("stats_node_{}", i),
            labels: vec![format!("Label{}", i % 3)],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let created = service.create_node(TEST_GRAPH_ID, node).await.unwrap();
        println!(
            "  ✅ Created node: {} with labels: {:?}",
            created.id, created.labels
        );
    }

    println!("🔍 DEBUG: Creating edges for stats test...");
    for i in 0..5 {
        let edge = Edge {
            id: format!("stats_edge_{}", i),
            from_node_id: format!("stats_node_{}", i),
            to_node_id: format!("stats_node_{}", i + 1),
            edge_type: format!("Type{}", i % 2),
            properties: HashMap::new(),
            weight: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let created = service.create_edge(TEST_GRAPH_ID, edge).await.unwrap();
        println!(
            "  ✅ Created edge: {} ({} -> {}) type: {}",
            created.id, created.from_node_id, created.to_node_id, created.edge_type
        );
    }

    // Get statistics
    println!("🔍 DEBUG: Fetching graph statistics...");
    let stats = service.get_stats(TEST_GRAPH_ID).await.unwrap();
    println!("  Total nodes: {}", stats.total_nodes);
    println!("  Total edges: {}", stats.total_edges);
    println!("  Label stats entries: {}", stats.label_stats.len());
    for label_stat in &stats.label_stats {
        println!("    - {}: {} nodes", label_stat.label, label_stat.count);
    }
    println!("  Edge type stats entries: {}", stats.edge_type_stats.len());
    for edge_stat in &stats.edge_type_stats {
        println!("    - {}: {} edges", edge_stat.edge_type, edge_stat.count);
    }

    assert_eq!(
        stats.total_nodes, 10,
        "Expected 10 nodes, got {}",
        stats.total_nodes
    );
    assert_eq!(
        stats.total_edges, 5,
        "Expected 5 edges, got {}",
        stats.total_edges
    );
    assert!(
        stats.label_stats.len() > 0,
        "Expected label stats, got {}",
        stats.label_stats.len()
    );
    assert!(
        stats.edge_type_stats.len() > 0,
        "Expected edge type stats, got {}",
        stats.edge_type_stats.len()
    );
}

/// Test unique constraints
#[tokio::test]
async fn test_unique_constraints() {
    let service = Arc::new(GraphOperationsService::new());
    ensure_test_graph_exists(&service).await;

    println!("🔍 DEBUG: Adding unique constraint on User.email...");
    // Add unique constraint on email property for User label
    service
        .add_unique_constraint(TEST_GRAPH_ID, "User", "email")
        .await
        .unwrap();
    println!("  ✅ Unique constraint added");

    println!("🔍 DEBUG: Creating first user with email test@example.com...");
    // Create first user
    let node1 = Node {
        id: "user_with_email_1".to_string(),
        labels: vec!["User".to_string()],
        properties: HashMap::from([(
            "email".to_string(),
            PropertyValue {
                value: Some(Value::StringValue("test@example.com".to_string())),
            },
        )]),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    let created = service.create_node(TEST_GRAPH_ID, node1).await.unwrap();
    println!("  ✅ Created first user: {}", created.id);
    assert_eq!(created.id, "user_with_email_1".to_string());

    println!("🔍 DEBUG: Attempting to create second user with duplicate email (should fail)...");
    // Try to create second user with same email - should fail
    let node2 = Node {
        id: "user_with_email_2".to_string(),
        labels: vec!["User".to_string()],
        properties: HashMap::from([(
            "email".to_string(),
            PropertyValue {
                value: Some(Value::StringValue("test@example.com".to_string())),
            },
        )]),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    let result = service.create_node(TEST_GRAPH_ID, node2).await;

    match &result {
        Ok(node) => {
            println!(
                "  ❌ UNEXPECTED: Second user was created: {} (should have failed!)",
                node.id
            );
        }
        Err(e) => {
            println!("  ✅ Creation correctly failed with error: {}", e);
        }
    }

    assert!(
        result.is_err(),
        "Expected error due to unique constraint violation, but creation succeeded"
    );

    // Verify error message contains constraint violation
    if let Err(e) = result {
        let error_msg = e.to_string();
        println!("  Error message: {}", error_msg);
        assert!(
            error_msg.contains("unique constraint") || error_msg.contains("Unique constraint"),
            "Error message doesn't mention unique constraint: {}",
            error_msg
        );
    }
}

/// Test concurrent operations
#[tokio::test]
async fn test_concurrent_operations() {
    let service = Arc::new(GraphOperationsService::new());
    ensure_test_graph_exists(&service).await;

    // Phase 1: Create all nodes concurrently
    let mut handles = vec![];
    for i in 0..10 {
        let service_clone = Arc::clone(&service);
        let handle = tokio::spawn(async move {
            let node = Node {
                id: format!("concurrent_node_{}", i),
                labels: vec!["ConcurrentNode".to_string()],
                properties: HashMap::from([(
                    "thread_id".to_string(),
                    PropertyValue {
                        value: Some(Value::IntValue(i as i64)),
                    },
                )]),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service_clone
                .create_node(TEST_GRAPH_ID, node)
                .await
                .unwrap();
        });
        handles.push(handle);
    }
    for handle in handles {
        handle.await.unwrap();
    }

    // Phase 2: Create edges (nodes guaranteed to exist now)
    let mut edge_handles = vec![];
    for i in 1..10 {
        let service_clone = Arc::clone(&service);
        let handle = tokio::spawn(async move {
            let edge = Edge {
                id: format!("concurrent_edge_{}", i),
                from_node_id: format!("concurrent_node_{}", i - 1),
                to_node_id: format!("concurrent_node_{}", i),
                edge_type: "NEXT".to_string(),
                properties: HashMap::new(),
                weight: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service_clone
                .create_edge(TEST_GRAPH_ID, edge)
                .await
                .unwrap();
        });
        edge_handles.push(handle);
    }
    for handle in edge_handles {
        handle.await.unwrap();
    }

    // Verify all nodes and edges were created
    let stats = service.get_stats(TEST_GRAPH_ID).await.unwrap();
    assert!(stats.total_nodes >= 10);
    assert!(stats.total_edges >= 9);
}
