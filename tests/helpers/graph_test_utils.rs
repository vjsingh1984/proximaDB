//! # Graph Testing Utilities
//!
//! Helper functions and utilities for testing ProximaDB graph capabilities

use proximadb::{
    graph::{Edge, Node, OperationMode, PropertyValue, service::GraphOperationsService},
    proto::proximadb_v1::{
        property_value::Value, CreateGraphRequest, NodeQuery, TraversalRequest,
        TraversalAlgorithm, GraphStats, PropertyFilter, PropertyFilterOperator
    },
};
use std::collections::HashMap;
use std::sync::Arc;
use anyhow::Result;

/// Test graph collection identifier
pub const TEST_GRAPH_ID: &str = "test_graph";

/// Initialize a graph service with a test graph collection
pub async fn setup_test_graph_service() -> Result<Arc<GraphOperationsService>> {
    let service = Arc::new(GraphOperationsService::new());

    // Create the test graph collection
    let create_request = CreateGraphRequest {
        graph_id: TEST_GRAPH_ID.to_string(),
        name: Some("Test Graph Collection".to_string()),
        description: Some("Test graph for integration testing".to_string()),
        schema: None,
        storage_config: None,
        engine_config: None,
        access_control: None,
    };

    // Create the graph collection (ignore if it already exists)
    match service.create_graph_collection(create_request).await {
        Ok(_) => println!("Created test graph collection: {}", TEST_GRAPH_ID),
        Err(e) if e.to_string().contains("already exists") => {
            println!("Test graph collection already exists: {}", TEST_GRAPH_ID);
        }
        Err(e) => return Err(e),
    }

    Ok(service)
}

/// Create a test user node
pub fn create_test_user_node(id: &str, name: &str, age: i32) -> Node {
    Node {
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
                    value: Some(Value::IntValue(age)),
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
    }
}

/// Create a test product node
pub fn create_test_product_node(id: &str, name: &str, price: f64) -> Node {
    Node {
        id: id.to_string(),
        labels: vec!["Product".to_string()],
        properties: HashMap::from([
            (
                "name".to_string(),
                PropertyValue {
                    value: Some(Value::StringValue(name.to_string())),
                },
            ),
            (
                "price".to_string(),
                PropertyValue {
                    value: Some(Value::NumberValue(price)),
                },
            ),
        ]),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

/// Create a test edge between two nodes
pub fn create_test_edge(from_id: &str, to_id: &str, edge_type: &str, weight: f64) -> Edge {
    Edge {
        id: format!("{}_{}_{}_{}", from_id, edge_type, to_id, weight as i32),
        from_node_id: from_id.to_string(),
        to_node_id: to_id.to_string(),
        edge_type: edge_type.to_string(),
        weight: Some(weight),
        properties: HashMap::new(),
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

/// Clean up test data from the graph
pub async fn cleanup_test_graph(service: &GraphOperationsService) -> Result<()> {
    // Note: We don't delete the collection as it might be shared across tests
    // Instead, we could clear all nodes and edges if needed
    println!("Test cleanup completed for graph: {}", TEST_GRAPH_ID);
    Ok(())
}

/// Verify graph collection exists and is accessible
pub async fn verify_graph_collection(service: &GraphOperationsService) -> Result<bool> {
    // Try to get stats to verify the collection exists
    match service.get_stats(TEST_GRAPH_ID).await {
        Ok(_) => Ok(true),
        Err(e) if e.to_string().contains("does not exist") => Ok(false),
        Err(e) => Err(e),
    }
}

/// Create a sample graph with users and products for testing
pub async fn populate_sample_graph(service: &GraphOperationsService) -> Result<()> {
    // Create users
    let alice = create_test_user_node("alice", "Alice Smith", 29);
    let bob = create_test_user_node("bob", "Bob Johnson", 34);

    // Create products
    let laptop = create_test_product_node("laptop1", "Gaming Laptop", 1299.99);
    let mouse = create_test_product_node("mouse1", "Wireless Mouse", 79.99);

    // Add nodes
    service.create_node(TEST_GRAPH_ID, alice).await?;
    service.create_node(TEST_GRAPH_ID, bob).await?;
    service.create_node(TEST_GRAPH_ID, laptop).await?;
    service.create_node(TEST_GRAPH_ID, mouse).await?;

    // Create relationships
    let alice_owns_laptop = create_test_edge("alice", "laptop1", "OWNS", 1.0);
    let alice_likes_mouse = create_test_edge("alice", "mouse1", "LIKES", 0.8);
    let bob_owns_mouse = create_test_edge("bob", "mouse1", "OWNS", 1.0);
    let alice_knows_bob = create_test_edge("alice", "bob", "KNOWS", 0.9);

    // Add edges
    service.create_edge(TEST_GRAPH_ID, alice_owns_laptop).await?;
    service.create_edge(TEST_GRAPH_ID, alice_likes_mouse).await?;
    service.create_edge(TEST_GRAPH_ID, bob_owns_mouse).await?;
    service.create_edge(TEST_GRAPH_ID, alice_knows_bob).await?;

    println!("Sample graph populated with 4 nodes and 4 edges");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_setup_graph_service() {
        let service = setup_test_graph_service().await.unwrap();
        let exists = verify_graph_collection(&service).await.unwrap();
        assert!(exists, "Test graph collection should exist after setup");
    }

    #[tokio::test]
    async fn test_sample_graph_population() {
        let service = setup_test_graph_service().await.unwrap();
        populate_sample_graph(&service).await.unwrap();

        // Verify nodes exist
        let alice = service.get_node(TEST_GRAPH_ID, &"alice".to_string()).await.unwrap();
        assert!(alice.is_some(), "Alice node should exist");

        let laptop = service.get_node(TEST_GRAPH_ID, &"laptop1".to_string()).await.unwrap();
        assert!(laptop.is_some(), "Laptop node should exist");

        // Verify stats
        let stats = service.get_stats(TEST_GRAPH_ID).await.unwrap();
        assert!(stats.node_count >= 4, "Should have at least 4 nodes");
        assert!(stats.edge_count >= 4, "Should have at least 4 edges");
    }
}