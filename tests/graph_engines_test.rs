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

//! # Graph Engines Integration Tests
//!
//! Tests for PULSAR and QUASAR graph engines to verify basic functionality.

use proximadb::graph::PropertyValue;
use proximadb::graph::engines::pulsar::PulsarConfig;
use proximadb::graph::engines::quasar::QuasarConfig;
use proximadb::graph::{
    Edge, GraphEngineConfig, GraphEngineFactory, GraphEngineType, Node,
    PulsarGraphEngine, QuasarGraphEngine,
};
use proximadb::proto::proximadb_v1::property_value::Value;
use std::collections::HashMap;
use tempfile::TempDir;

#[tokio::test]
async fn test_pulsar_engine_basic_operations() {
    let config = PulsarConfig {
        shard_count: 4,
        replication_factor: 1,
        ..PulsarConfig::default()
    };

    let engine = PulsarGraphEngine::new(config).unwrap();

    // Test node insertion
    let node = Node {
        id: "test_node_pulsar".to_string(),
        labels: vec!["TestNode".to_string()],
        properties: HashMap::from([(
            "name".to_string(),
            PropertyValue {
                value: Some(Value::StringValue("PULSAR Test Node".to_string())),
            },
        )]),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    let inserted = engine.add_node(node).unwrap();
    assert_eq!(inserted.id, "test_node_pulsar");

    // Wait for async operations
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Test node retrieval
    let retrieved = engine.get_node_by_id("test_node_pulsar").unwrap().unwrap();
    assert_eq!(retrieved.id, "test_node_pulsar");
    assert_eq!(retrieved.labels, vec!["TestNode"]);

    // Test node count
    assert_eq!(engine.get_node_count().unwrap(), 1);

    // Test statistics
    let stats = engine.get_stats().await;
    assert_eq!(stats.total_nodes, 1);
    assert_eq!(stats.shards_active, 4);
}

#[tokio::test]
async fn test_quasar_engine_basic_operations() {
    let temp_dir = TempDir::new().unwrap();
    let config = QuasarConfig {
        hot_tier_max_nodes: 10,
        cold_tier_path: temp_dir.path().to_path_buf(),
        ..QuasarConfig::default()
    };

    let engine = QuasarGraphEngine::new(config).await.unwrap();

    // Test node insertion
    let node = Node {
        id: "test_node_quasar".to_string(),
        labels: vec!["TestNode".to_string()],
        properties: HashMap::from([(
            "name".to_string(),
            PropertyValue {
                value: Some(Value::StringValue("QUASAR Test Node".to_string())),
            },
        )]),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    let inserted = engine.add_node(node).unwrap();
    assert_eq!(inserted.id, "test_node_quasar");

    // Test node retrieval
    let retrieved = engine.get_node_by_id("test_node_quasar").unwrap().unwrap();
    assert_eq!(retrieved.id, "test_node_quasar");
    assert_eq!(retrieved.labels, vec!["TestNode"]);

    // Test node count
    assert_eq!(engine.get_node_count().unwrap(), 1);

    // Test statistics
    let stats = engine.get_stats().await;
    assert_eq!(stats.hot_tier_nodes, 1);
    assert_eq!(stats.cold_tier_nodes, 0);
    assert!(stats.cache_hits > 0);
}

#[tokio::test]
async fn test_pulsar_edge_operations() {
    let engine = PulsarGraphEngine::new(PulsarConfig::default()).unwrap();

    // Create nodes first
    let node1 = Node {
        id: "node1".to_string(),
        labels: vec!["Person".to_string()],
        properties: HashMap::new(),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    let node2 = Node {
        id: "node2".to_string(),
        labels: vec!["Person".to_string()],
        properties: HashMap::new(),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    engine.add_node(node1).unwrap();
    engine.add_node(node2).unwrap();

    // Create edge
    let edge = Edge {
        id: "edge1".to_string(),
        from_node_id: "node1".to_string(),
        to_node_id: "node2".to_string(),
        edge_type: "KNOWS".to_string(),
        properties: HashMap::new(),
        weight: Some(1.0),
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    let inserted_edge = engine.add_edge(edge).unwrap();
    assert_eq!(inserted_edge.id, "edge1");

    // Wait for async operations
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Test edge retrieval
    let retrieved_edge = engine.get_edge_by_id("edge1").unwrap().unwrap();
    assert_eq!(retrieved_edge.edge_type, "KNOWS");

    // Test outgoing edges
    let outgoing = engine.get_edges_from_node("node1", None).unwrap();
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].to_node_id, "node2");

    // Test neighbors
    let neighbors = engine.get_connected_nodes("node1", None).unwrap();
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0].id, "node2");
}

#[tokio::test]
async fn test_quasar_tiering_behavior() {
    let temp_dir = TempDir::new().unwrap();
    let config = QuasarConfig {
        hot_tier_max_nodes: 2, // Small hot tier to force migration
        cold_tier_path: temp_dir.path().to_path_buf(),
        ..QuasarConfig::default()
    };

    let engine = QuasarGraphEngine::new(config).await.unwrap();

    // Insert nodes beyond hot tier capacity
    for i in 0..5 {
        let node = Node {
            id: format!("node_{}", i),
            labels: vec!["TestNode".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: None,
            updated_at_ms: None,
        };

        engine.add_node(node).unwrap();
    }

    // Wait for background migration
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Verify all nodes are accessible
    assert_eq!(engine.node_count().unwrap(), 5);

    // Verify access works across tiers
    for i in 0..5 {
        let node_id = format!("node_{}", i);
        let retrieved = engine.get_node_by_id(&node_id).unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, node_id);
    }

    let stats = engine.get_stats().await;
    // Should have some nodes in hot tier and potentially some in cold tier
    assert!(stats.hot_tier_nodes + stats.cold_tier_nodes >= 5);
}

#[test]
fn test_engine_factory() {
    // Test ORION engine creation
    let orion_engine =
        GraphEngineFactory::create_engine(GraphEngineType::Orion, GraphEngineConfig::default())
            .unwrap();

    assert_eq!(orion_engine.node_count().unwrap(), 0);

    // Test PULSAR engine creation
    let config = GraphEngineConfig {
        pulsar_config: Some(PulsarConfig {
            shard_count: 2,
            ..PulsarConfig::default()
        }),
        ..GraphEngineConfig::default()
    };

    let pulsar_engine = GraphEngineFactory::create_engine(GraphEngineType::Pulsar, config).unwrap();

    assert_eq!(pulsar_engine.node_count().unwrap(), 0);
}

#[test]
fn test_engine_capabilities() {
    let orion_caps = GraphEngineFactory::get_engine_capabilities(GraphEngineType::Orion);
    assert_eq!(orion_caps.name, "ORION");
    assert!(orion_caps.features.len() > 0);
    assert!(orion_caps.use_cases.len() > 0);

    let pulsar_caps = GraphEngineFactory::get_engine_capabilities(GraphEngineType::Pulsar);
    assert_eq!(pulsar_caps.name, "PULSAR");
    assert!(pulsar_caps.description.contains("Distributed"));

    let quasar_caps = GraphEngineFactory::get_engine_capabilities(GraphEngineType::Quasar);
    assert_eq!(quasar_caps.name, "QUASAR");
    assert!(quasar_caps.description.contains("Hybrid"));
}

#[test]
fn test_engine_type_from_string() {
    assert_eq!(
        GraphEngineFactory::engine_type_from_string("orion"),
        Some(GraphEngineType::Orion)
    );
    assert_eq!(
        GraphEngineFactory::engine_type_from_string("PULSAR"),
        Some(GraphEngineType::Pulsar)
    );
    assert_eq!(
        GraphEngineFactory::engine_type_from_string("Quasar"),
        Some(GraphEngineType::Quasar)
    );
    assert_eq!(GraphEngineFactory::engine_type_from_string("unknown"), None);
}

#[tokio::test]
async fn test_pulsar_cross_shard_operations() {
    let config = PulsarConfig {
        shard_count: 3,
        replication_factor: 1,
        ..PulsarConfig::default()
    };

    let engine = PulsarGraphEngine::new(config).unwrap();

    // Create multiple nodes that will likely be on different shards
    let node_ids = vec!["alice", "bob", "charlie", "david", "eve"];
    for &id in &node_ids {
        let node = Node {
            id: id.to_string(),
            labels: vec!["Person".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: None,
            updated_at_ms: None,
        };
        engine.add_node(node).unwrap();
    }

    // Wait for async operations
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Test cross-shard traversal
    let nodes = engine.cross_shard_traversal(&"alice".to_string(), 2).await.unwrap();

    // Should return at least the starting node
    assert!(nodes.len() >= 1);

    // Test nodes by label (cross-shard query)
    let person_nodes = engine.get_nodes_with_label("Person").unwrap();
    assert_eq!(person_nodes.len(), 5);

    let stats = engine.get_stats().await;
    assert!(stats.cross_shard_queries > 0);
}

#[tokio::test]
async fn test_quasar_access_pattern_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let config = QuasarConfig {
        hot_tier_max_nodes: 10,
        cold_tier_path: temp_dir.path().to_path_buf(),
        ..QuasarConfig::default()
    };

    let engine = QuasarGraphEngine::new(config).await.unwrap();

    // Insert a test node
    let node = Node {
        id: "tracked_node".to_string(),
        labels: vec!["TrackedNode".to_string()],
        properties: HashMap::new(),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };

    engine.insert_node(node).unwrap();

    // Access the node multiple times to build access pattern
    for _ in 0..5 {
        let _ = engine.get_node_by_id("tracked_node").unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    let stats = engine.get_stats().await;
    assert!(stats.cache_hits >= 5); // Should have multiple cache hits

    let access_stats = engine.get_access_stats().await;
    assert!(access_stats.total_accesses >= 5);
    assert!(access_stats.unique_items_tracked >= 1);
}
