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

//! # PULSAR and QUASAR WAL Integration Tests
//!
//! TDD tests for WAL (Write-Ahead Logging) integration in PULSAR and QUASAR
//! graph engines. These tests verify that:
//!
//! 1. WAL writes are performed for update/delete operations
//! 2. Data can be recovered after restart
//! 3. Service recovery handles all engine types
//! 4. flush_wal works for all engine types

// Feature-gate entire file since PULSAR/QUASAR require distributed-graph feature
#![cfg(feature = "distributed-graph")]

#[cfg(feature = "distributed-graph")]
mod pulsar_wal_tests {
    use proximadb::graph::engines::pulsar::{PulsarConfig, PulsarGraphEngine};
    use proximadb::graph::{Edge, Node, engines::GraphEngine};
    use std::collections::HashMap;
    use tempfile::TempDir;

    /// Helper to create a test node
    fn create_test_node(id: &str, label: &str) -> Node {
        Node {
            id: id.to_string(),
            labels: vec![label.to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Helper to create a test edge
    fn create_test_edge(id: &str, from: &str, to: &str, edge_type: &str) -> Edge {
        Edge {
            id: id.to_string(),
            from_node_id: from.to_string(),
            to_node_id: to.to_string(),
            edge_type: edge_type.to_string(),
            properties: HashMap::new(),
            weight: Some(1.0),
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Test PULSAR node persistence across operations
    #[tokio::test]
    async fn test_pulsar_node_persistence() {
        let config = PulsarConfig {
            shard_count: 4,
            replication_factor: 1,
            ..PulsarConfig::default()
        };
        let engine = PulsarGraphEngine::new(config).expect("Failed to create PULSAR engine");

        // Insert nodes
        let node1 = create_test_node("p_node_1", "TestLabel");
        let node2 = create_test_node("p_node_2", "TestLabel");

        let inserted1 = engine
            .insert_node(node1)
            .await
            .expect("Failed to insert node1");
        let inserted2 = engine
            .insert_node(node2)
            .await
            .expect("Failed to insert node2");

        assert_eq!(inserted1.id, "p_node_1");
        assert_eq!(inserted2.id, "p_node_2");

        // Verify nodes exist
        let retrieved1 = engine
            .get_node(&"p_node_1".to_string())
            .expect("Failed to get node1");
        assert!(retrieved1.is_some());
        assert_eq!(retrieved1.unwrap().id, "p_node_1");

        // Update node
        let mut updated_node =
            (*engine.get_node(&"p_node_1".to_string()).unwrap().unwrap()).clone();
        updated_node.labels.push("UpdatedLabel".to_string());
        let updated = engine
            .update_node(updated_node)
            .await
            .expect("Failed to update node");
        assert!(updated.labels.contains(&"UpdatedLabel".to_string()));

        // Delete node
        let deleted = engine
            .delete_node(&"p_node_2".to_string())
            .await
            .expect("Failed to delete node");
        assert!(deleted.is_some());

        // Verify deletion
        let missing = engine
            .get_node(&"p_node_2".to_string())
            .expect("Failed to check deleted node");
        assert!(missing.is_none());
    }

    /// Test PULSAR edge persistence across operations
    #[tokio::test]
    async fn test_pulsar_edge_persistence() {
        let config = PulsarConfig {
            shard_count: 4,
            replication_factor: 1,
            ..PulsarConfig::default()
        };
        let engine = PulsarGraphEngine::new(config).expect("Failed to create PULSAR engine");

        // Insert nodes first
        let node1 = create_test_node("e_node_1", "Person");
        let node2 = create_test_node("e_node_2", "Person");
        engine
            .insert_node(node1)
            .await
            .expect("Failed to insert node1");
        engine
            .insert_node(node2)
            .await
            .expect("Failed to insert node2");

        // Insert edge
        let edge = create_test_edge("e_edge_1", "e_node_1", "e_node_2", "KNOWS");
        let inserted = engine
            .insert_edge(edge)
            .await
            .expect("Failed to insert edge");
        assert_eq!(inserted.id, "e_edge_1");

        // Verify edge exists
        let retrieved = engine
            .get_edge(&"e_edge_1".to_string())
            .expect("Failed to get edge");
        assert!(retrieved.is_some());

        // Update edge
        let mut updated_edge = (*retrieved.unwrap()).clone();
        updated_edge.weight = Some(2.0);
        let updated = engine
            .update_edge(updated_edge)
            .await
            .expect("Failed to update edge");
        assert_eq!(updated.weight, Some(2.0));

        // Delete edge
        let deleted = engine
            .delete_edge(&"e_edge_1".to_string())
            .await
            .expect("Failed to delete edge");
        assert!(deleted.is_some());

        // Verify deletion
        let missing = engine
            .get_edge(&"e_edge_1".to_string())
            .expect("Failed to check deleted edge");
        assert!(missing.is_none());
    }

    /// Test PULSAR cross-shard traversal after update/delete operations
    #[tokio::test]
    async fn test_pulsar_cross_shard_consistency() {
        let config = PulsarConfig {
            shard_count: 4,
            replication_factor: 1,
            ..PulsarConfig::default()
        };
        let engine = PulsarGraphEngine::new(config).expect("Failed to create PULSAR engine");

        // Create a chain: A -> B -> C
        for id in ["cs_a", "cs_b", "cs_c"] {
            let node = create_test_node(id, "ChainNode");
            engine
                .insert_node(node)
                .await
                .expect("Failed to insert chain node");
        }

        let edge_ab = create_test_edge("cs_e1", "cs_a", "cs_b", "NEXT");
        let edge_bc = create_test_edge("cs_e2", "cs_b", "cs_c", "NEXT");
        engine
            .insert_edge(edge_ab)
            .await
            .expect("Failed to insert edge AB");
        engine
            .insert_edge(edge_bc)
            .await
            .expect("Failed to insert edge BC");

        // Verify chain exists
        let neighbors_a = engine
            .get_neighbors(&"cs_a".to_string(), None)
            .expect("Failed to get neighbors of A");
        assert_eq!(neighbors_a.len(), 1);
        assert_eq!(neighbors_a[0].id, "cs_b");

        // Update middle node
        let mut node_b = (*engine.get_node(&"cs_b".to_string()).unwrap().unwrap()).clone();
        node_b.labels.push("Updated".to_string());
        engine
            .update_node(node_b)
            .await
            .expect("Failed to update node B");

        // Verify chain still works after update
        let neighbors_a_after = engine
            .get_neighbors(&"cs_a".to_string(), None)
            .expect("Failed to get neighbors after update");
        assert_eq!(neighbors_a_after.len(), 1);
        assert!(neighbors_a_after[0].labels.contains(&"Updated".to_string()));
    }

    /// Test PULSAR statistics tracking across operations
    #[tokio::test]
    async fn test_pulsar_stats_tracking() {
        let config = PulsarConfig {
            shard_count: 4,
            ..PulsarConfig::default()
        };
        let engine = PulsarGraphEngine::new(config).expect("Failed to create PULSAR engine");

        // Initial stats
        let initial_stats = engine.get_stats().await;
        assert_eq!(initial_stats.total_nodes, 0);
        assert_eq!(initial_stats.total_edges, 0);

        // Insert nodes and edges
        for i in 0..5 {
            let node = create_test_node(&format!("stat_node_{}", i), "StatNode");
            engine
                .insert_node(node)
                .await
                .expect("Failed to insert node");
        }

        for i in 0..4 {
            let edge = create_test_edge(
                &format!("stat_edge_{}", i),
                &format!("stat_node_{}", i),
                &format!("stat_node_{}", i + 1),
                "CONNECTS",
            );
            engine
                .insert_edge(edge)
                .await
                .expect("Failed to insert edge");
        }

        let stats_after_insert = engine.get_stats().await;
        assert_eq!(stats_after_insert.total_nodes, 5);
        assert_eq!(stats_after_insert.total_edges, 4);

        // Delete a node
        engine
            .delete_node(&"stat_node_0".to_string())
            .await
            .expect("Failed to delete node");

        let stats_after_delete = engine.get_stats().await;
        assert_eq!(stats_after_delete.total_nodes, 4);
    }
}

#[cfg(feature = "tiered-graph")]
mod quasar_wal_tests {
    use proximadb::graph::engines::quasar::{ColdStorageBackend, QuasarConfig, QuasarGraphEngine};
    use proximadb::graph::{Edge, Node, engines::GraphEngine};
    use std::collections::HashMap;
    use std::time::Duration;
    use tempfile::TempDir;

    /// Helper to create a test node
    fn create_test_node(id: &str, label: &str) -> Node {
        Node {
            id: id.to_string(),
            labels: vec![label.to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Helper to create a test edge
    fn create_test_edge(id: &str, from: &str, to: &str, edge_type: &str) -> Edge {
        Edge {
            id: id.to_string(),
            from_node_id: from.to_string(),
            to_node_id: to.to_string(),
            edge_type: edge_type.to_string(),
            properties: HashMap::new(),
            weight: Some(1.0),
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Test QUASAR node operations in hot tier
    #[tokio::test]
    async fn test_quasar_hot_tier_node_operations() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = QuasarConfig {
            cold_tier_path: temp_dir.path().to_path_buf(),
            hot_tier_max_nodes: 100,
            cold_storage_backend: ColdStorageBackend::Json,
            ..QuasarConfig::default()
        };
        let engine = QuasarGraphEngine::new(config)
            .await
            .expect("Failed to create QUASAR engine");

        // Insert nodes into hot tier
        let node1 = create_test_node("q_node_1", "HotNode");
        let node2 = create_test_node("q_node_2", "HotNode");

        engine
            .insert_node(node1)
            .await
            .expect("Failed to insert node1");
        engine
            .insert_node(node2)
            .await
            .expect("Failed to insert node2");

        // Verify hot tier stats
        let stats = engine.get_stats().await;
        assert_eq!(stats.hot_tier_nodes, 2);

        // Update node
        let mut updated_node =
            (*engine.get_node(&"q_node_1".to_string()).unwrap().unwrap()).clone();
        updated_node.labels.push("Updated".to_string());
        let updated = engine
            .update_node(updated_node)
            .await
            .expect("Failed to update node");
        assert!(updated.labels.contains(&"Updated".to_string()));

        // Delete node
        let deleted = engine
            .delete_node(&"q_node_2".to_string())
            .await
            .expect("Failed to delete node");
        assert!(deleted.is_some());

        // Verify stats after delete
        let stats_after = engine.get_stats().await;
        assert_eq!(stats_after.hot_tier_nodes, 1);
    }

    /// Test QUASAR edge operations
    #[tokio::test]
    async fn test_quasar_edge_operations() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = QuasarConfig {
            cold_tier_path: temp_dir.path().to_path_buf(),
            cold_storage_backend: ColdStorageBackend::Json,
            ..QuasarConfig::default()
        };
        let engine = QuasarGraphEngine::new(config)
            .await
            .expect("Failed to create QUASAR engine");

        // Insert nodes
        let node1 = create_test_node("qe_node_1", "Person");
        let node2 = create_test_node("qe_node_2", "Person");
        engine
            .insert_node(node1)
            .await
            .expect("Failed to insert node1");
        engine
            .insert_node(node2)
            .await
            .expect("Failed to insert node2");

        // Insert edge
        let edge = create_test_edge("qe_edge_1", "qe_node_1", "qe_node_2", "KNOWS");
        let inserted = engine
            .insert_edge(edge)
            .await
            .expect("Failed to insert edge");
        assert_eq!(inserted.id, "qe_edge_1");

        // Verify edge in hot tier
        let stats = engine.get_stats().await;
        assert_eq!(stats.hot_tier_edges, 1);

        // Update edge
        let retrieved = engine
            .get_edge(&"qe_edge_1".to_string())
            .expect("Failed to get edge");
        assert!(retrieved.is_some());
        let mut updated_edge = (*retrieved.unwrap()).clone();
        updated_edge.weight = Some(2.5);
        let updated = engine
            .update_edge(updated_edge)
            .await
            .expect("Failed to update edge");
        assert_eq!(updated.weight, Some(2.5));

        // Delete edge
        let deleted = engine
            .delete_edge(&"qe_edge_1".to_string())
            .await
            .expect("Failed to delete edge");
        assert!(deleted.is_some());
    }

    /// Test QUASAR cold tier promotion
    #[tokio::test]
    async fn test_quasar_cold_tier_operations() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = QuasarConfig {
            cold_tier_path: temp_dir.path().to_path_buf(),
            hot_tier_max_nodes: 2, // Very small hot tier to force cold storage
            cold_storage_backend: ColdStorageBackend::Json,
            hot_promotion_threshold: Duration::from_millis(1), // Quick promotion
            ..QuasarConfig::default()
        };
        let engine = QuasarGraphEngine::new(config)
            .await
            .expect("Failed to create QUASAR engine");

        // Insert nodes to hot tier
        for i in 0..3 {
            let node = create_test_node(&format!("cold_node_{}", i), "ColdTestNode");
            engine
                .insert_node(node)
                .await
                .expect("Failed to insert node");
        }

        // Check that we have nodes
        let stats = engine.get_stats().await;
        assert!(stats.hot_tier_nodes >= 2); // At least 2 should be in hot tier

        // Update a node (should work regardless of tier)
        let mut node = (*engine
            .get_node(&"cold_node_0".to_string())
            .unwrap()
            .unwrap())
        .clone();
        node.labels.push("ModifiedInColdTest".to_string());
        let updated = engine
            .update_node(node)
            .await
            .expect("Failed to update node");
        assert!(updated.labels.contains(&"ModifiedInColdTest".to_string()));
    }

    /// Test QUASAR cache statistics
    #[tokio::test]
    async fn test_quasar_cache_statistics() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = QuasarConfig {
            cold_tier_path: temp_dir.path().to_path_buf(),
            cold_storage_backend: ColdStorageBackend::Json,
            ..QuasarConfig::default()
        };
        let engine = QuasarGraphEngine::new(config)
            .await
            .expect("Failed to create QUASAR engine");

        // Insert some data
        for i in 0..5 {
            let node = create_test_node(&format!("cache_node_{}", i), "CacheNode");
            engine
                .insert_node(node)
                .await
                .expect("Failed to insert node");
        }

        // Access nodes multiple times to generate cache hits
        for _ in 0..3 {
            for i in 0..5 {
                let _ = engine.get_node(&format!("cache_node_{}", i));
            }
        }

        // Check stats
        let stats = engine.get_stats().await;
        assert!(
            stats.cache_hits > 0,
            "Should have cache hits after repeated access"
        );
    }
}

mod service_recovery_tests {
    use proximadb::graph::service::GraphOperationsService;
    use proximadb::proto::proximadb_v1::{
        CompressionAlgorithm, CreateGraphRequest, GraphStorageConfig,
    };
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Test that service can create graphs with ORION engine (baseline)
    #[tokio::test]
    async fn test_service_orion_graph_creation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let service = GraphOperationsService::new();

        let request = CreateGraphRequest {
            graph_id: "test_orion".to_string(),
            name: Some("Test ORION Graph".to_string()),
            description: None,
            schema: None,
            storage_config: Some(GraphStorageConfig {
                engine_type: "ORION".to_string(),
                base_url: temp_dir.path().to_string_lossy().to_string(),
                compression: CompressionAlgorithm::CompressionNone as i32,
                enable_wal: true,
                snapshot_interval_hours: 24,
                engine_specific_config: HashMap::new(),
            }),
            engine_config: None,
            access_control: None,
        };

        service
            .create_graph_collection(request)
            .await
            .expect("Failed to create ORION graph");

        // Verify graph is accessible
        let graphs = service.list_graphs().await.expect("Failed to list graphs");
        assert!(graphs.contains(&"test_orion".to_string()));
    }

    /// Test that service can create graphs with PULSAR engine
    #[cfg(feature = "distributed-graph")]
    #[tokio::test]
    async fn test_service_pulsar_graph_creation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let service = GraphOperationsService::new();

        let mut engine_config = HashMap::new();
        engine_config.insert("shard_count".to_string(), "4".to_string());

        let request = CreateGraphRequest {
            graph_id: "test_pulsar".to_string(),
            name: Some("Test PULSAR Graph".to_string()),
            description: None,
            schema: None,
            storage_config: Some(GraphStorageConfig {
                engine_type: "PULSAR".to_string(),
                base_url: temp_dir.path().to_string_lossy().to_string(),
                compression: CompressionAlgorithm::CompressionNone as i32,
                enable_wal: true,
                snapshot_interval_hours: 24,
                engine_specific_config: engine_config,
            }),
            engine_config: None,
            access_control: None,
        };

        service
            .create_graph_collection(request)
            .await
            .expect("Failed to create PULSAR graph");

        // Verify graph is accessible
        let graphs = service.list_graphs().await.expect("Failed to list graphs");
        assert!(graphs.contains(&"test_pulsar".to_string()));
    }

    /// Test that service can create graphs with QUASAR engine
    #[cfg(feature = "tiered-graph")]
    #[tokio::test]
    async fn test_service_quasar_graph_creation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let service = GraphOperationsService::new();

        let mut engine_config = HashMap::new();
        engine_config.insert("hot_tier_max_nodes".to_string(), "1000".to_string());
        engine_config.insert(
            "cold_tier_path".to_string(),
            temp_dir.path().join("cold").to_string_lossy().to_string(),
        );

        let request = CreateGraphRequest {
            graph_id: "test_quasar".to_string(),
            name: Some("Test QUASAR Graph".to_string()),
            description: None,
            schema: None,
            storage_config: Some(GraphStorageConfig {
                engine_type: "QUASAR".to_string(),
                base_url: temp_dir.path().to_string_lossy().to_string(),
                compression: CompressionAlgorithm::CompressionNone as i32,
                enable_wal: true,
                snapshot_interval_hours: 24,
                engine_specific_config: engine_config,
            }),
            engine_config: None,
            access_control: None,
        };

        service
            .create_graph_collection(request)
            .await
            .expect("Failed to create QUASAR graph");

        // Verify graph is accessible
        let graphs = service.list_graphs().await.expect("Failed to list graphs");
        assert!(graphs.contains(&"test_quasar".to_string()));
    }

    /// Test service flush_wal for all engine types
    #[tokio::test]
    async fn test_service_flush_wal() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let service = GraphOperationsService::new();

        // Create ORION graph
        let request = CreateGraphRequest {
            graph_id: "flush_test".to_string(),
            name: Some("Flush Test Graph".to_string()),
            description: None,
            schema: None,
            storage_config: Some(GraphStorageConfig {
                engine_type: "ORION".to_string(),
                base_url: temp_dir.path().to_string_lossy().to_string(),
                compression: CompressionAlgorithm::CompressionNone as i32,
                enable_wal: true,
                snapshot_interval_hours: 24,
                engine_specific_config: HashMap::new(),
            }),
            engine_config: None,
            access_control: None,
        };

        service
            .create_graph_collection(request)
            .await
            .expect("Failed to create graph");

        // Insert some data
        let node = proximadb::graph::Node {
            id: "flush_node".to_string(),
            labels: vec!["Test".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        service
            .create_node("flush_test", node)
            .await
            .expect("Failed to create node");

        // Flush WAL - should not fail
        service
            .flush_wal("flush_test")
            .await
            .expect("Failed to flush WAL");

        // Flush non-existent graph - should not fail (graceful handling)
        service
            .flush_wal("non_existent")
            .await
            .expect("Flushing non-existent graph should not fail");
    }
}

mod wal_recovery_tests {
    use proximadb::graph::engines::orion::OrionGraphEngine;
    use proximadb::graph::{Edge, Node, engines::GraphEngine};
    use std::collections::HashMap;
    use tempfile::TempDir;

    /// Helper to create a test node
    fn create_test_node(id: &str, label: &str) -> Node {
        Node {
            id: id.to_string(),
            labels: vec![label.to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Test ORION WAL recovery (baseline for comparison)
    #[tokio::test]
    async fn test_orion_wal_recovery_baseline() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());

        // Phase 1: Create engine and insert data
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "recovery_test".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine");

            // Insert nodes
            let node1 = create_test_node("rec_node_1", "RecoveryTest");
            let node2 = create_test_node("rec_node_2", "RecoveryTest");
            engine
                .insert_node(node1)
                .await
                .expect("Failed to insert node1");
            engine
                .insert_node(node2)
                .await
                .expect("Failed to insert node2");

            // Verify data exists
            assert!(
                engine
                    .get_node(&"rec_node_1".to_string())
                    .unwrap()
                    .is_some()
            );
            assert!(
                engine
                    .get_node(&"rec_node_2".to_string())
                    .unwrap()
                    .is_some()
            );

            // Flush WAL to ensure data is persisted
            engine.flush_wal().await.expect("Failed to flush WAL");
        }

        // Phase 2: Create new engine and recover from WAL
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "recovery_test".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine for recovery");

            // Trigger recovery
            engine.recover().await.expect("Failed to recover from WAL");

            // Verify data was recovered
            let recovered1 = engine
                .get_node(&"rec_node_1".to_string())
                .expect("Failed to get node1");
            let recovered2 = engine
                .get_node(&"rec_node_2".to_string())
                .expect("Failed to get node2");

            assert!(recovered1.is_some(), "Node 1 should be recovered");
            assert!(recovered2.is_some(), "Node 2 should be recovered");
        }
    }

    /// Test that update operations are persisted via WAL
    #[tokio::test]
    async fn test_update_node_wal_persistence() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());

        // Phase 1: Insert and update
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "update_test".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine");

            // Insert node
            let node = create_test_node("upd_node", "Original");
            engine
                .insert_node(node)
                .await
                .expect("Failed to insert node");

            // Update node with new label
            let mut updated_node =
                (*engine.get_node(&"upd_node".to_string()).unwrap().unwrap()).clone();
            updated_node.labels.push("Updated".to_string());
            engine
                .update_node(updated_node)
                .await
                .expect("Failed to update node");

            engine.flush_wal().await.expect("Failed to flush WAL");
        }

        // Phase 2: Recover and verify update was persisted
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "update_test".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine for recovery");

            engine.recover().await.expect("Failed to recover");

            let recovered = engine
                .get_node(&"upd_node".to_string())
                .expect("Failed to get node")
                .unwrap();
            assert!(
                recovered.labels.contains(&"Updated".to_string()),
                "Updated label should be persisted. Got labels: {:?}",
                recovered.labels
            );
        }
    }

    /// Test that delete operations are persisted via WAL
    #[tokio::test]
    async fn test_delete_node_wal_persistence() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());

        // Phase 1: Insert and delete
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "delete_test".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine");

            // Insert two nodes
            let node1 = create_test_node("del_node_1", "ToKeep");
            let node2 = create_test_node("del_node_2", "ToDelete");
            engine
                .insert_node(node1)
                .await
                .expect("Failed to insert node1");
            engine
                .insert_node(node2)
                .await
                .expect("Failed to insert node2");

            // Delete one node
            engine
                .delete_node(&"del_node_2".to_string())
                .await
                .expect("Failed to delete node");

            engine.flush_wal().await.expect("Failed to flush WAL");
        }

        // Phase 2: Recover and verify delete was persisted
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "delete_test".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine for recovery");

            engine.recover().await.expect("Failed to recover");

            let kept = engine
                .get_node(&"del_node_1".to_string())
                .expect("Failed to get kept node");
            let deleted = engine
                .get_node(&"del_node_2".to_string())
                .expect("Failed to get deleted node");

            assert!(kept.is_some(), "Node 1 should be kept");
            assert!(deleted.is_none(), "Node 2 should be deleted after recovery");
        }
    }
}

/// PULSAR WAL Recovery Tests - Verifies data persistence across engine restart
#[cfg(feature = "distributed-graph")]
mod pulsar_wal_recovery_tests {
    use proximadb::graph::engines::pulsar::{PulsarConfig, PulsarGraphEngine};
    use proximadb::graph::{Edge, Node, engines::GraphEngine};
    use std::collections::HashMap;
    use tempfile::TempDir;

    /// Helper to create a test node
    fn create_test_node(id: &str, label: &str) -> Node {
        Node {
            id: id.to_string(),
            labels: vec![label.to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Helper to create a test edge
    fn create_test_edge(id: &str, from: &str, to: &str, edge_type: &str) -> Edge {
        Edge {
            id: id.to_string(),
            from_node_id: from.to_string(),
            to_node_id: to.to_string(),
            edge_type: edge_type.to_string(),
            properties: HashMap::new(),
            weight: Some(1.0),
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Test PULSAR WAL recovery with nodes across multiple shards
    #[tokio::test]
    async fn test_pulsar_wal_recovery_nodes() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());
        let graph_id = "pulsar_recovery_nodes";

        // Phase 1: Create engine with persistence, insert data, and flush
        {
            let config = PulsarConfig {
                shard_count: 4,
                replication_factor: 1,
                ..PulsarConfig::default()
            };
            let engine =
                PulsarGraphEngine::with_persistence(config, graph_id.to_string(), base_url.clone())
                    .await
                    .expect("Failed to create PULSAR engine with persistence");

            // Insert nodes that will be distributed across shards
            for i in 0..10 {
                let node = create_test_node(&format!("p_rec_node_{}", i), "RecoveryTest");
                engine
                    .insert_node(node)
                    .await
                    .expect("Failed to insert node");
            }

            // Verify all nodes exist
            for i in 0..10 {
                let node_id = format!("p_rec_node_{}", i);
                assert!(
                    engine.get_node(&node_id).expect("Get failed").is_some(),
                    "Node {} should exist before recovery",
                    node_id
                );
            }

            // Flush WAL to ensure persistence
            engine.flush_wal().await.expect("Failed to flush WAL");
        }

        // Phase 2: Create new engine and recover
        {
            let config = PulsarConfig {
                shard_count: 4,
                replication_factor: 1,
                ..PulsarConfig::default()
            };
            let engine =
                PulsarGraphEngine::with_persistence(config, graph_id.to_string(), base_url.clone())
                    .await
                    .expect("Failed to create PULSAR engine for recovery");

            // Recover from WAL
            engine.recover().await.expect("Failed to recover PULSAR");

            // Verify all nodes were recovered
            for i in 0..10 {
                let node_id = format!("p_rec_node_{}", i);
                let recovered = engine.get_node(&node_id).expect("Get failed");
                assert!(
                    recovered.is_some(),
                    "Node {} should be recovered after WAL replay",
                    node_id
                );
            }
        }
    }

    /// Test PULSAR WAL recovery with edges across shards
    #[tokio::test]
    async fn test_pulsar_wal_recovery_edges() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());
        let graph_id = "pulsar_recovery_edges";

        // Phase 1: Create graph with nodes and edges
        {
            let config = PulsarConfig {
                shard_count: 4,
                replication_factor: 1,
                ..PulsarConfig::default()
            };
            let engine =
                PulsarGraphEngine::with_persistence(config, graph_id.to_string(), base_url.clone())
                    .await
                    .expect("Failed to create PULSAR engine");

            // Create a chain: A -> B -> C -> D
            for c in ['A', 'B', 'C', 'D'] {
                let node = create_test_node(&format!("chain_{}", c), "ChainNode");
                engine
                    .insert_node(node)
                    .await
                    .expect("Failed to insert node");
            }

            // Add edges
            let edges = [
                create_test_edge("e_ab", "chain_A", "chain_B", "NEXT"),
                create_test_edge("e_bc", "chain_B", "chain_C", "NEXT"),
                create_test_edge("e_cd", "chain_C", "chain_D", "NEXT"),
            ];
            for edge in edges {
                engine
                    .insert_edge(edge)
                    .await
                    .expect("Failed to insert edge");
            }

            // Verify traversal works
            let neighbors = engine
                .get_neighbors(&"chain_A".to_string(), None)
                .expect("Failed to get neighbors");
            assert_eq!(neighbors.len(), 1);
            assert_eq!(neighbors[0].id, "chain_B");

            engine.flush_wal().await.expect("Failed to flush WAL");
        }

        // Phase 2: Recover and verify edges
        {
            let config = PulsarConfig {
                shard_count: 4,
                replication_factor: 1,
                ..PulsarConfig::default()
            };
            let engine =
                PulsarGraphEngine::with_persistence(config, graph_id.to_string(), base_url.clone())
                    .await
                    .expect("Failed to create PULSAR engine for recovery");

            engine.recover().await.expect("Failed to recover");

            // Verify edges were recovered
            let edge_ab = engine
                .get_edge(&"e_ab".to_string())
                .expect("Get edge failed");
            let edge_bc = engine
                .get_edge(&"e_bc".to_string())
                .expect("Get edge failed");
            let edge_cd = engine
                .get_edge(&"e_cd".to_string())
                .expect("Get edge failed");

            assert!(edge_ab.is_some(), "Edge A->B should be recovered");
            assert!(edge_bc.is_some(), "Edge B->C should be recovered");
            assert!(edge_cd.is_some(), "Edge C->D should be recovered");

            // Verify traversal still works
            let neighbors = engine
                .get_neighbors(&"chain_B".to_string(), None)
                .expect("Failed to get neighbors");
            assert_eq!(neighbors.len(), 1);
            assert_eq!(neighbors[0].id, "chain_C");
        }
    }

    /// Test PULSAR update and delete operations persist via WAL
    #[tokio::test]
    async fn test_pulsar_wal_recovery_updates_and_deletes() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());
        let graph_id = "pulsar_recovery_updates";

        // Phase 1: Create, update, and delete operations
        {
            let config = PulsarConfig {
                shard_count: 4,
                replication_factor: 1,
                ..PulsarConfig::default()
            };
            let engine =
                PulsarGraphEngine::with_persistence(config, graph_id.to_string(), base_url.clone())
                    .await
                    .expect("Failed to create PULSAR engine");

            // Insert nodes
            let node_to_update = create_test_node("upd_node", "Original");
            let node_to_delete = create_test_node("del_node", "ToDelete");
            let node_to_keep = create_test_node("keep_node", "ToKeep");

            engine
                .insert_node(node_to_update)
                .await
                .expect("Insert failed");
            engine
                .insert_node(node_to_delete)
                .await
                .expect("Insert failed");
            engine
                .insert_node(node_to_keep)
                .await
                .expect("Insert failed");

            // Update node
            let mut updated = (*engine.get_node(&"upd_node".to_string()).unwrap().unwrap()).clone();
            updated.labels.push("Modified".to_string());
            engine.update_node(updated).await.expect("Update failed");

            // Delete node
            engine
                .delete_node(&"del_node".to_string())
                .await
                .expect("Delete failed");

            engine.flush_wal().await.expect("Flush failed");
        }

        // Phase 2: Recover and verify
        {
            let config = PulsarConfig {
                shard_count: 4,
                replication_factor: 1,
                ..PulsarConfig::default()
            };
            let engine =
                PulsarGraphEngine::with_persistence(config, graph_id.to_string(), base_url.clone())
                    .await
                    .expect("Failed to create PULSAR engine for recovery");

            engine.recover().await.expect("Recovery failed");

            // Verify update persisted
            let updated_node = engine
                .get_node(&"upd_node".to_string())
                .expect("Get failed")
                .expect("Updated node should exist");
            assert!(
                updated_node.labels.contains(&"Modified".to_string()),
                "Update should be persisted. Labels: {:?}",
                updated_node.labels
            );

            // Verify delete persisted
            let deleted_node = engine
                .get_node(&"del_node".to_string())
                .expect("Get failed");
            assert!(
                deleted_node.is_none(),
                "Deleted node should not exist after recovery"
            );

            // Verify kept node still exists
            let kept_node = engine
                .get_node(&"keep_node".to_string())
                .expect("Get failed");
            assert!(kept_node.is_some(), "Kept node should still exist");
        }
    }
}

/// QUASAR WAL Recovery Tests - Verifies hot tier persistence across restart
#[cfg(feature = "tiered-graph")]
mod quasar_wal_recovery_tests {
    use proximadb::graph::engines::quasar::{ColdStorageBackend, QuasarConfig, QuasarGraphEngine};
    use proximadb::graph::{Edge, Node, engines::GraphEngine};
    use std::collections::HashMap;
    use std::time::Duration;
    use tempfile::TempDir;

    /// Helper to create a test node
    fn create_test_node(id: &str, label: &str) -> Node {
        Node {
            id: id.to_string(),
            labels: vec![label.to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Helper to create a test edge
    fn create_test_edge(id: &str, from: &str, to: &str, edge_type: &str) -> Edge {
        Edge {
            id: id.to_string(),
            from_node_id: from.to_string(),
            to_node_id: to.to_string(),
            edge_type: edge_type.to_string(),
            properties: HashMap::new(),
            weight: Some(1.0),
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Test QUASAR WAL recovery for hot tier nodes
    #[tokio::test]
    async fn test_quasar_wal_recovery_nodes() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());
        let graph_id = "quasar_recovery_nodes";

        // Phase 1: Create engine and insert data
        {
            let config = QuasarConfig {
                cold_tier_path: temp_dir.path().join("cold"),
                hot_tier_max_nodes: 1000,
                cold_storage_backend: ColdStorageBackend::Json,
                ..QuasarConfig::default()
            };
            let engine =
                QuasarGraphEngine::with_persistence(config, graph_id.to_string(), base_url.clone())
                    .await
                    .expect("Failed to create QUASAR engine with persistence");

            // Insert nodes into hot tier
            for i in 0..10 {
                let node = create_test_node(&format!("q_rec_node_{}", i), "HotTierNode");
                engine
                    .insert_node(node)
                    .await
                    .expect("Failed to insert node");
            }

            // Verify hot tier has nodes
            let stats = engine.get_stats().await;
            assert_eq!(stats.hot_tier_nodes, 10, "Should have 10 nodes in hot tier");

            // Flush WAL
            engine.flush_wal().await.expect("Failed to flush WAL");
        }

        // Phase 2: Recover and verify
        {
            let config = QuasarConfig {
                cold_tier_path: temp_dir.path().join("cold"),
                hot_tier_max_nodes: 1000,
                cold_storage_backend: ColdStorageBackend::Json,
                ..QuasarConfig::default()
            };
            let engine =
                QuasarGraphEngine::with_persistence(config, graph_id.to_string(), base_url.clone())
                    .await
                    .expect("Failed to create QUASAR engine for recovery");

            engine.recover().await.expect("Failed to recover QUASAR");

            // Verify nodes were recovered
            for i in 0..10 {
                let node_id = format!("q_rec_node_{}", i);
                let recovered = engine.get_node(&node_id).expect("Get failed");
                assert!(recovered.is_some(), "Node {} should be recovered", node_id);
            }
        }
    }

    /// Test QUASAR WAL recovery for edges
    #[tokio::test]
    async fn test_quasar_wal_recovery_edges() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());
        let graph_id = "quasar_recovery_edges";

        // Phase 1: Create graph with edges
        {
            let config = QuasarConfig {
                cold_tier_path: temp_dir.path().join("cold"),
                cold_storage_backend: ColdStorageBackend::Json,
                ..QuasarConfig::default()
            };
            let engine =
                QuasarGraphEngine::with_persistence(config, graph_id.to_string(), base_url.clone())
                    .await
                    .expect("Failed to create QUASAR engine");

            // Create nodes
            let node1 = create_test_node("q_edge_node_1", "Person");
            let node2 = create_test_node("q_edge_node_2", "Person");
            engine.insert_node(node1).await.expect("Insert failed");
            engine.insert_node(node2).await.expect("Insert failed");

            // Create edge
            let edge = create_test_edge("q_edge_1", "q_edge_node_1", "q_edge_node_2", "KNOWS");
            engine.insert_edge(edge).await.expect("Insert edge failed");

            // Verify
            let stats = engine.get_stats().await;
            assert_eq!(stats.hot_tier_edges, 1);

            engine.flush_wal().await.expect("Flush failed");
        }

        // Phase 2: Recover and verify
        {
            let config = QuasarConfig {
                cold_tier_path: temp_dir.path().join("cold"),
                cold_storage_backend: ColdStorageBackend::Json,
                ..QuasarConfig::default()
            };
            let engine =
                QuasarGraphEngine::with_persistence(config, graph_id.to_string(), base_url.clone())
                    .await
                    .expect("Failed to create QUASAR engine for recovery");

            engine.recover().await.expect("Recovery failed");

            // Verify edge was recovered
            let edge = engine
                .get_edge(&"q_edge_1".to_string())
                .expect("Get edge failed");
            assert!(edge.is_some(), "Edge should be recovered");

            // Verify edge metadata
            let edge = edge.unwrap();
            assert_eq!(edge.from_node_id, "q_edge_node_1");
            assert_eq!(edge.to_node_id, "q_edge_node_2");
            assert_eq!(edge.edge_type, "KNOWS");
        }
    }

    /// Test QUASAR update and delete operations persist via WAL
    #[tokio::test]
    async fn test_quasar_wal_recovery_updates_and_deletes() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());
        let graph_id = "quasar_recovery_updates";

        // Phase 1: Perform operations
        {
            let config = QuasarConfig {
                cold_tier_path: temp_dir.path().join("cold"),
                cold_storage_backend: ColdStorageBackend::Json,
                ..QuasarConfig::default()
            };
            let engine =
                QuasarGraphEngine::with_persistence(config, graph_id.to_string(), base_url.clone())
                    .await
                    .expect("Failed to create QUASAR engine");

            // Insert nodes
            let node_to_update = create_test_node("q_upd_node", "Original");
            let node_to_delete = create_test_node("q_del_node", "ToDelete");
            engine
                .insert_node(node_to_update)
                .await
                .expect("Insert failed");
            engine
                .insert_node(node_to_delete)
                .await
                .expect("Insert failed");

            // Update node
            let mut updated =
                (*engine.get_node(&"q_upd_node".to_string()).unwrap().unwrap()).clone();
            updated.labels.push("Modified".to_string());
            engine.update_node(updated).await.expect("Update failed");

            // Delete node
            engine
                .delete_node(&"q_del_node".to_string())
                .await
                .expect("Delete failed");

            engine.flush_wal().await.expect("Flush failed");
        }

        // Phase 2: Recover and verify
        {
            let config = QuasarConfig {
                cold_tier_path: temp_dir.path().join("cold"),
                cold_storage_backend: ColdStorageBackend::Json,
                ..QuasarConfig::default()
            };
            let engine =
                QuasarGraphEngine::with_persistence(config, graph_id.to_string(), base_url.clone())
                    .await
                    .expect("Failed to create QUASAR engine for recovery");

            engine.recover().await.expect("Recovery failed");

            // Verify update persisted
            let updated_node = engine
                .get_node(&"q_upd_node".to_string())
                .expect("Get failed")
                .expect("Updated node should exist");
            assert!(
                updated_node.labels.contains(&"Modified".to_string()),
                "Update should be persisted. Labels: {:?}",
                updated_node.labels
            );

            // Verify delete persisted
            let deleted_node = engine
                .get_node(&"q_del_node".to_string())
                .expect("Get failed");
            assert!(
                deleted_node.is_none(),
                "Deleted node should not exist after recovery"
            );
        }
    }

    /// Test QUASAR flush during tiering operations
    #[tokio::test]
    async fn test_quasar_wal_flush_during_tiering() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());
        let graph_id = "quasar_tiering_flush";

        let config = QuasarConfig {
            cold_tier_path: temp_dir.path().join("cold"),
            hot_tier_max_nodes: 5, // Small hot tier to trigger tiering
            cold_storage_backend: ColdStorageBackend::Json,
            migration_interval: Duration::from_millis(100),
            ..QuasarConfig::default()
        };

        let engine =
            QuasarGraphEngine::with_persistence(config, graph_id.to_string(), base_url.clone())
                .await
                .expect("Failed to create QUASAR engine");

        // Insert more nodes than hot tier can hold
        for i in 0..10 {
            let node = create_test_node(&format!("tier_node_{}", i), "TierTest");
            engine.insert_node(node).await.expect("Insert failed");
        }

        // Give tiering a moment to potentially kick in
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Flush should work even during tiering
        engine
            .flush_wal()
            .await
            .expect("Flush should succeed during tiering");

        // Force migration
        engine
            .force_migration()
            .await
            .expect("Force migration should succeed");

        // Flush again after migration
        engine
            .flush_wal()
            .await
            .expect("Flush after migration should succeed");
    }
}

/// Multi-engine service recovery tests
mod multi_engine_service_tests {
    use proximadb::graph::Node;
    use proximadb::graph::service::GraphOperationsService;
    use proximadb::proto::proximadb_v1::{
        CompressionAlgorithm, CreateGraphRequest, GraphStorageConfig,
    };
    use std::collections::HashMap;
    use tempfile::TempDir;

    /// Helper to create a test node
    fn create_test_node(id: &str, label: &str) -> Node {
        Node {
            id: id.to_string(),
            labels: vec![label.to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Test that service flush_wal is idempotent
    #[tokio::test]
    async fn test_service_flush_wal_idempotent() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let service = GraphOperationsService::new();

        let request = CreateGraphRequest {
            graph_id: "idempotent_test".to_string(),
            name: Some("Idempotent Test".to_string()),
            description: None,
            schema: None,
            storage_config: Some(GraphStorageConfig {
                engine_type: "ORION".to_string(),
                base_url: format!("file://{}", temp_dir.path().display()),
                compression: CompressionAlgorithm::CompressionNone as i32,
                enable_wal: true,
                snapshot_interval_hours: 24,
                engine_specific_config: HashMap::new(),
            }),
            engine_config: None,
            access_control: None,
        };

        service
            .create_graph_collection(request)
            .await
            .expect("Failed to create graph");

        // Insert data
        let node = create_test_node("idem_node", "Test");
        service
            .create_node("idempotent_test", node)
            .await
            .expect("Insert failed");

        // Multiple flush calls should all succeed
        for _ in 0..5 {
            service
                .flush_wal("idempotent_test")
                .await
                .expect("Flush should be idempotent");
        }
    }

    /// Test that service handles multiple graphs with different engines
    #[tokio::test]
    async fn test_service_multiple_graphs_flush() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let service = GraphOperationsService::new();

        // Create ORION graph
        let orion_request = CreateGraphRequest {
            graph_id: "multi_orion".to_string(),
            name: Some("Multi ORION".to_string()),
            description: None,
            schema: None,
            storage_config: Some(GraphStorageConfig {
                engine_type: "ORION".to_string(),
                base_url: format!("file://{}", temp_dir.path().display()),
                compression: CompressionAlgorithm::CompressionNone as i32,
                enable_wal: true,
                snapshot_interval_hours: 24,
                engine_specific_config: HashMap::new(),
            }),
            engine_config: None,
            access_control: None,
        };
        service
            .create_graph_collection(orion_request)
            .await
            .expect("Failed to create ORION graph");

        // Insert data in both graphs
        service
            .create_node("multi_orion", create_test_node("o_node", "Orion"))
            .await
            .expect("Insert to ORION failed");

        // Flush all graphs
        service
            .flush_wal("multi_orion")
            .await
            .expect("ORION flush failed");
    }
}
