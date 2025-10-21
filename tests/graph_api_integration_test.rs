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

//! Integration tests for Graph API end-to-end functionality
//!
//! Tests the complete flow from graph collection creation through node/edge operations,
//! verifying that the GraphCollectionService and GraphOperationsService are properly wired.

use proximadb::proto::proximadb_v1::{CreateGraphRequest, Node, PropertyValue};
use proximadb::services::GraphCollectionService;
use proximadb::graph::GraphOperationsService;
use std::sync::Arc;
use std::collections::HashMap;

#[tokio::test]
async fn test_graph_collection_service_isolation_bug() {
    // This test demonstrates the bug where graph_collection_service and
    // graph_operations_service have DIFFERENT GraphCollectionService instances

    // Simulate UnifiedHandlers::new() which creates TWO instances
    let graph_collection_service_external = Arc::new(GraphCollectionService::new());
    let graph_operations_service = Arc::new(
        GraphOperationsService::new_with_collection_service(
            Arc::new(GraphCollectionService::new()), // BUG: This is a DIFFERENT instance!
        ),
    );

    // Create graph collection using external service (simulates REST /graphs endpoint)
    let create_request = CreateGraphRequest {
        graph_id: "test_graph".to_string(),
        name: Some("Test Graph".to_string()),
        description: None,
        schema: None,
        storage_config: None,
        engine_config: None,
        access_control: None,
    };

    let collection = graph_collection_service_external
        .create_graph(create_request)
        .await
        .expect("Failed to create graph collection");

    assert_eq!(collection.graph_id, "test_graph");

    // Verify graph exists in external service
    let exists = graph_collection_service_external
        .get_graph("test_graph")
        .await
        .expect("Failed to get graph");
    assert!(exists.is_some(), "Graph should exist in external service");

    // Now try to create a node using GraphOperationsService (simulates REST /nodes endpoint)
    let mut properties = HashMap::new();
    properties.insert(
        "name".to_string(),
        PropertyValue {
            value: Some(crate::proto::proximadb_v1::property_value::Value::StringValue(
                "Alice".to_string(),
            )),
        },
    );

    let node = Node {
        id: "node1".to_string(),
        labels: vec!["Person".to_string()],
        properties,
        embedding: None,
        embedding_version: None,
        created_at: None,
        updated_at: None,
    };

    // This should FAIL because GraphOperationsService has a DIFFERENT GraphCollectionService instance
    let result = graph_operations_service
        .create_node("test_graph", node)
        .await;

    match result {
        Err(proximadb::core::error::ProximaDBError::InvalidInput(msg)) => {
            assert!(
                msg.contains("Graph collection 'test_graph' does not exist"),
                "Expected 'does not exist' error, got: {}",
                msg
            );
            println!("✓ Bug confirmed: GraphCollectionService instances are isolated");
        }
        Ok(_) => panic!("Bug NOT reproduced - node creation should have failed!"),
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[tokio::test]
async fn test_graph_collection_service_shared_correctly() {
    // This test shows the CORRECT wiring where both services share the SAME instance

    // Create single GraphCollectionService instance
    let graph_collection_service = Arc::new(GraphCollectionService::new());

    // Share it with GraphOperationsService
    let graph_operations_service = Arc::new(
        GraphOperationsService::new_with_collection_service(graph_collection_service.clone()),
    );

    // Create graph collection
    let create_request = CreateGraphRequest {
        graph_id: "test_graph".to_string(),
        name: Some("Test Graph".to_string()),
        description: None,
        schema: None,
        storage_config: None,
        engine_config: None,
        access_control: None,
    };

    let collection = graph_collection_service
        .create_graph(create_request)
        .await
        .expect("Failed to create graph collection");

    assert_eq!(collection.graph_id, "test_graph");

    // Now create a node - this should SUCCEED
    let mut properties = HashMap::new();
    properties.insert(
        "name".to_string(),
        PropertyValue {
            value: Some(crate::proto::proximadb_v1::property_value::Value::StringValue(
                "Alice".to_string(),
            )),
        },
    );

    let node = Node {
        id: "node1".to_string(),
        labels: vec!["Person".to_string()],
        properties,
        embedding: None,
        embedding_version: None,
        created_at: None,
        updated_at: None,
    };

    let result = graph_operations_service
        .create_node("test_graph", node)
        .await;

    match result {
        Ok(created_node) => {
            assert_eq!(created_node.id, "node1");
            assert_eq!(created_node.labels, vec!["Person".to_string()]);
            println!("✓ Node created successfully with shared GraphCollectionService");
        }
        Err(e) => panic!("Node creation failed: {:?}", e),
    }
}

#[tokio::test]
async fn test_end_to_end_graph_operations() {
    // Full end-to-end test simulating real API usage

    let graph_collection_service = Arc::new(GraphCollectionService::new());
    let graph_operations_service = Arc::new(
        GraphOperationsService::new_with_collection_service(graph_collection_service.clone()),
    );

    // 1. Create graph collection
    let create_request = CreateGraphRequest {
        graph_id: "social_network".to_string(),
        name: Some("Social Network".to_string()),
        description: Some("A social network graph".to_string()),
        schema: None,
        storage_config: None,
        engine_config: None,
        access_control: None,
    };

    graph_collection_service
        .create_graph(create_request)
        .await
        .expect("Failed to create graph");

    // 2. Create nodes
    let alice = create_person_node("alice", "Alice");
    let bob = create_person_node("bob", "Bob");

    let alice_node = graph_operations_service
        .create_node("social_network", alice)
        .await
        .expect("Failed to create Alice node");

    let bob_node = graph_operations_service
        .create_node("social_network", bob)
        .await
        .expect("Failed to create Bob node");

    assert_eq!(alice_node.id, "alice");
    assert_eq!(bob_node.id, "bob");

    // 3. Query nodes
    let query = proximadb::proto::proximadb_v1::NodeQuery {
        labels: vec!["Person".to_string()],
        filters: vec![],
        limit: Some(10),
        offset: Some(0),
        continuation_token: None,
    };

    let nodes = graph_operations_service
        .query_nodes("social_network", query)
        .await
        .expect("Failed to query nodes");

    assert_eq!(nodes.len(), 2, "Should find both Alice and Bob");

    // 4. Get graph stats
    let stats = graph_collection_service
        .get_graph_stats("social_network")
        .await
        .expect("Failed to get stats");

    assert!(stats.is_some(), "Stats should exist");

    println!("✓ End-to-end graph operations test passed");
}

// Helper function to create a person node
fn create_person_node(id: &str, name: &str) -> Node {
    let mut properties = HashMap::new();
    properties.insert(
        "name".to_string(),
        PropertyValue {
            value: Some(crate::proto::proximadb_v1::property_value::Value::StringValue(
                name.to_string(),
            )),
        },
    );

    Node {
        id: id.to_string(),
        labels: vec!["Person".to_string()],
        properties,
        embedding: None,
        embedding_version: None,
        created_at: None,
        updated_at: None,
    }
}
