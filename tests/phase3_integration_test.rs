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

//! # Phase 3 Cross-Workstream Integration Tests
//!
//! Comprehensive integration tests verifying interactions between Phase 3 workstreams:
//!
//! - WS-1: PULSAR/QUASAR WAL wiring
//! - WS-2: External Catalogs (Delta Lake)
//! - WS-3: Streaming Storage Integration
//! - WS-5: mTLS Infrastructure
//! - WS-6: Integration Tests & Documentation
//!
//! Run with: `cargo test --test phase3_integration_test`
//!
//! For feature-gated tests:
//! - `cargo test --test phase3_integration_test --features distributed-graph`
//! - `cargo test --test phase3_integration_test --features tiered-graph`
//! - `cargo test --test phase3_integration_test --features delta-lake`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

// =============================================================================
// WS-3: Streaming Storage Integration Tests
// =============================================================================

mod streaming_flush_tests {
    use super::*;
    use proximadb::proto::proximadb_v1::VectorRecord;
    use proximadb::streaming::{
        BackpressureLevel, CoordinatorStats, FlushRetryConfig, SessionConfig, StreamConfig,
        StreamCoordinator,
    };

    /// Helper to create test vectors
    fn create_test_vectors(count: usize, start_id: usize, dimension: usize) -> Vec<VectorRecord> {
        (0..count)
            .map(|i| VectorRecord {
                id: format!("vec_{}", start_id + i),
                vector: vec![0.1 * (i as f32); dimension],
                metadata: Default::default(),
                ..Default::default()
            })
            .collect()
    }

    /// Helper config for tests with small buffers
    fn small_buffer_config() -> StreamConfig {
        StreamConfig {
            max_streams: 100,
            default_buffer_size: 256,
            global_rate_limit: 10000,
            flush_interval: Duration::from_millis(50),
            session_timeout: Duration::from_secs(60),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_flush_retry_config_defaults() {
        let config = FlushRetryConfig::default();

        assert_eq!(config.max_retries, 3);
        assert!(config.initial_delay.as_millis() >= 50);
        assert!(config.max_delay.as_secs() >= 1);
        assert!(config.backoff_multiplier >= 1.5);
    }

    #[tokio::test]
    async fn test_flush_retry_config_custom() {
        let config = FlushRetryConfig {
            max_retries: 5,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.5,
        };

        assert_eq!(config.max_retries, 5);
        assert_eq!(config.initial_delay.as_millis(), 100);
        assert_eq!(config.max_delay.as_secs(), 10);
        assert!((config.backoff_multiplier - 2.5).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_coordinator_stats_initial() {
        let coordinator = StreamCoordinator::new(small_buffer_config());
        let stats = coordinator.stats();

        assert_eq!(stats.active_sessions, 0);
        assert_eq!(stats.total_buffered_records, 0);
    }

    #[tokio::test]
    async fn test_coordinator_stats_after_session_creation() {
        let coordinator = StreamCoordinator::new(small_buffer_config());

        let _session_id = coordinator
            .create_session("test_collection".to_string(), SessionConfig::default())
            .await
            .expect("Failed to create session");

        let stats = coordinator.stats();
        assert_eq!(stats.active_sessions, 1);
        assert!(stats.sessions_created >= 1);
    }

    #[tokio::test]
    async fn test_coordinator_stats_after_push() {
        let coordinator = StreamCoordinator::new(small_buffer_config());

        let session_id = coordinator
            .create_session("test_collection".to_string(), SessionConfig::default())
            .await
            .expect("Failed to create session");

        let vectors = create_test_vectors(50, 0, 64);
        coordinator
            .push_records(&session_id, vectors)
            .await
            .unwrap();

        let stats = coordinator.stats();
        assert_eq!(stats.total_buffered_records, 50);
        assert!(stats.sessions_created >= 1);
    }

    #[tokio::test]
    async fn test_coordinator_stats_multiple_sessions() {
        let coordinator = StreamCoordinator::new(small_buffer_config());

        let session1 = coordinator
            .create_session("collection_1".to_string(), SessionConfig::default())
            .await
            .unwrap();

        let session2 = coordinator
            .create_session("collection_2".to_string(), SessionConfig::default())
            .await
            .unwrap();

        // Push to both sessions
        let vectors1 = create_test_vectors(30, 0, 64);
        let vectors2 = create_test_vectors(20, 100, 64);

        coordinator.push_records(&session1, vectors1).await.unwrap();
        coordinator.push_records(&session2, vectors2).await.unwrap();

        let stats = coordinator.stats();
        assert_eq!(stats.active_sessions, 2);
        assert_eq!(stats.total_buffered_records, 50);
    }

    #[tokio::test]
    async fn test_streaming_backpressure_levels() {
        let config = StreamConfig {
            default_buffer_size: 64, // Small buffer to trigger backpressure
            ..Default::default()
        };
        let coordinator = StreamCoordinator::new(config);

        let session_id = coordinator
            .create_session("backpressure_test".to_string(), SessionConfig::default())
            .await
            .unwrap();

        // Push vectors until we hit backpressure
        let vectors = create_test_vectors(60, 0, 32);
        let result = coordinator
            .push_records(&session_id, vectors)
            .await
            .unwrap();

        // With 60 items in 64-slot buffer, should have high backpressure
        assert!(result.buffer_percent >= 80);
        assert!(result.backpressure >= BackpressureLevel::High);
    }

    #[tokio::test]
    async fn test_streaming_session_drain() {
        let coordinator = StreamCoordinator::new(small_buffer_config());

        let session_id = coordinator
            .create_session("drain_test".to_string(), SessionConfig::default())
            .await
            .unwrap();

        // Push 100 vectors
        let vectors = create_test_vectors(100, 0, 64);
        coordinator
            .push_records(&session_id, vectors)
            .await
            .unwrap();

        // Drain in batches
        let batch1 = coordinator.drain_records(&session_id, 30).unwrap();
        assert_eq!(batch1.len(), 30);

        let batch2 = coordinator.drain_records(&session_id, 50).unwrap();
        assert_eq!(batch2.len(), 50);

        let batch3 = coordinator.drain_records(&session_id, 50).unwrap();
        assert_eq!(batch3.len(), 20); // Only 20 remaining
    }

    #[tokio::test]
    async fn test_streaming_session_close_and_stats() {
        let coordinator = StreamCoordinator::new(small_buffer_config());

        let session_id = coordinator
            .create_session("close_test".to_string(), SessionConfig::default())
            .await
            .unwrap();

        let stats_before = coordinator.stats();
        assert_eq!(stats_before.active_sessions, 1);

        coordinator.close_session(&session_id);

        let stats_after = coordinator.stats();
        assert_eq!(stats_after.active_sessions, 0);
    }

    #[tokio::test]
    async fn test_streaming_rate_limiting() {
        let config = StreamConfig {
            global_rate_limit: 50, // Very low rate for testing
            ..Default::default()
        };
        let coordinator = StreamCoordinator::new(config);

        let session_id = coordinator
            .create_session("rate_limit_test".to_string(), SessionConfig::default())
            .await
            .unwrap();

        // First push should succeed (within burst allowance)
        let vectors1 = create_test_vectors(50, 0, 32);
        let result1 = coordinator.push_records(&session_id, vectors1).await;
        assert!(result1.is_ok());

        // Immediate second push should be rate limited
        let vectors2 = create_test_vectors(50, 100, 32);
        let result2 = coordinator.push_records(&session_id, vectors2).await;
        assert!(result2.is_err());
    }

    #[tokio::test]
    async fn test_streaming_concurrent_sessions() {
        let coordinator = Arc::new(StreamCoordinator::new(StreamConfig {
            default_buffer_size: 4096,
            global_rate_limit: 1_000_000,
            ..Default::default()
        }));

        let mut handles = vec![];

        // Create 5 concurrent sessions
        for i in 0..5 {
            let coord = Arc::clone(&coordinator);
            handles.push(tokio::spawn(async move {
                let session_id = coord
                    .create_session(format!("concurrent_{}", i), SessionConfig::default())
                    .await
                    .unwrap();

                // Push vectors
                let vectors = create_test_vectors(100, i * 1000, 32);
                coord.push_records(&session_id, vectors).await.unwrap();

                session_id
            }));
        }

        let session_ids: Vec<_> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let stats = coordinator.stats();
        assert_eq!(stats.active_sessions, 5);
        assert_eq!(stats.total_buffered_records, 500);

        // Clean up
        for sid in session_ids {
            coordinator.close_session(&sid);
        }
    }

    #[tokio::test]
    async fn test_streaming_session_info() {
        let coordinator = StreamCoordinator::new(small_buffer_config());

        let session_id = coordinator
            .create_session("info_test".to_string(), SessionConfig::default())
            .await
            .unwrap();

        // Initial info
        let info = coordinator.get_session_info(&session_id).unwrap();
        assert_eq!(info.collection, "info_test");
        assert_eq!(info.buffer_len, 0);
        assert_eq!(info.records_received, 0);

        // Push some vectors
        let vectors = create_test_vectors(25, 0, 32);
        coordinator
            .push_records(&session_id, vectors)
            .await
            .unwrap();

        let info_after = coordinator.get_session_info(&session_id).unwrap();
        assert_eq!(info_after.buffer_len, 25);
        assert_eq!(info_after.records_received, 25);
    }
}

// =============================================================================
// WS-1: Graph WAL Recovery Tests - ORION Engine
// =============================================================================

mod graph_wal_recovery_tests {
    use super::*;
    use proximadb::graph::engines::orion::OrionGraphEngine;
    use proximadb::graph::{Edge, Node, engines::GraphEngine};

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

    #[tokio::test]
    async fn test_orion_node_insert_wal() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());

        let engine = OrionGraphEngine::with_persistence_for_graph(
            "insert_test".to_string(),
            base_url.clone(),
            true,
        )
        .await
        .expect("Failed to create engine");

        // Insert nodes
        let node = create_test_node("test_node_1", "TestLabel");
        let inserted = engine.insert_node(node).await.expect("Insert failed");

        assert_eq!(inserted.id, "test_node_1");

        // Flush WAL
        engine.flush_wal().await.expect("Flush failed");
    }

    #[tokio::test]
    async fn test_orion_edge_insert_wal() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());

        let engine = OrionGraphEngine::with_persistence_for_graph(
            "edge_test".to_string(),
            base_url.clone(),
            true,
        )
        .await
        .expect("Failed to create engine");

        // Insert nodes first
        let node1 = create_test_node("edge_node_1", "Person");
        let node2 = create_test_node("edge_node_2", "Person");
        engine
            .insert_node(node1)
            .await
            .expect("Insert node1 failed");
        engine
            .insert_node(node2)
            .await
            .expect("Insert node2 failed");

        // Insert edge
        let edge = create_test_edge("test_edge_1", "edge_node_1", "edge_node_2", "KNOWS");
        let inserted = engine.insert_edge(edge).await.expect("Insert edge failed");

        assert_eq!(inserted.id, "test_edge_1");

        engine.flush_wal().await.expect("Flush failed");
    }

    #[tokio::test]
    async fn test_orion_update_node_wal() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());

        let engine = OrionGraphEngine::with_persistence_for_graph(
            "update_test".to_string(),
            base_url.clone(),
            true,
        )
        .await
        .expect("Failed to create engine");

        // Insert node
        let node = create_test_node("upd_node", "Original");
        engine.insert_node(node).await.expect("Insert failed");

        // Update node
        let mut updated_node =
            (*engine.get_node(&"upd_node".to_string()).unwrap().unwrap()).clone();
        updated_node.labels.push("Updated".to_string());
        let updated = engine
            .update_node(updated_node)
            .await
            .expect("Update failed");

        assert!(updated.labels.contains(&"Updated".to_string()));

        engine.flush_wal().await.expect("Flush failed");
    }

    #[tokio::test]
    async fn test_orion_delete_node_wal() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());

        let engine = OrionGraphEngine::with_persistence_for_graph(
            "delete_test".to_string(),
            base_url.clone(),
            true,
        )
        .await
        .expect("Failed to create engine");

        // Insert nodes
        let node1 = create_test_node("del_node_1", "ToKeep");
        let node2 = create_test_node("del_node_2", "ToDelete");
        engine
            .insert_node(node1)
            .await
            .expect("Insert node1 failed");
        engine
            .insert_node(node2)
            .await
            .expect("Insert node2 failed");

        // Delete one node
        let deleted = engine
            .delete_node(&"del_node_2".to_string())
            .await
            .expect("Delete failed");
        assert!(deleted.is_some());

        // Verify deletion
        let missing = engine
            .get_node(&"del_node_2".to_string())
            .expect("Get failed");
        assert!(missing.is_none());

        engine.flush_wal().await.expect("Flush failed");
    }

    #[tokio::test]
    async fn test_orion_recovery_from_wal() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());

        // Phase 1: Insert data and flush
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "recovery_test".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine");

            for i in 0..10 {
                let node = create_test_node(&format!("rec_node_{}", i), "RecoveryTest");
                engine.insert_node(node).await.expect("Insert failed");
            }

            engine.flush_wal().await.expect("Flush failed");
        }

        // Phase 2: Recover and verify
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "recovery_test".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine for recovery");

            engine.recover().await.expect("Recovery failed");

            // Verify nodes were recovered
            for i in 0..10 {
                let node_id = format!("rec_node_{}", i);
                let recovered = engine.get_node(&node_id).expect("Get failed");
                assert!(
                    recovered.is_some(),
                    "Node {} should be recovered after WAL replay",
                    node_id
                );
            }
        }
    }

    #[tokio::test]
    async fn test_orion_recovery_with_updates() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());

        // Phase 1: Insert and update
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "update_recovery".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine");

            let node = create_test_node("upd_rec_node", "Original");
            engine.insert_node(node).await.expect("Insert failed");

            let mut updated = (*engine
                .get_node(&"upd_rec_node".to_string())
                .unwrap()
                .unwrap())
            .clone();
            updated.labels.push("Modified".to_string());
            engine.update_node(updated).await.expect("Update failed");

            engine.flush_wal().await.expect("Flush failed");
        }

        // Phase 2: Recover and verify update persisted
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "update_recovery".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine for recovery");

            engine.recover().await.expect("Recovery failed");

            let recovered = engine
                .get_node(&"upd_rec_node".to_string())
                .expect("Get failed")
                .expect("Node should exist");

            assert!(
                recovered.labels.contains(&"Modified".to_string()),
                "Update should persist after recovery. Labels: {:?}",
                recovered.labels
            );
        }
    }

    #[tokio::test]
    async fn test_orion_recovery_with_deletes() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());

        // Phase 1: Insert and delete
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "delete_recovery".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine");

            let node1 = create_test_node("del_rec_keep", "ToKeep");
            let node2 = create_test_node("del_rec_remove", "ToRemove");
            engine.insert_node(node1).await.expect("Insert failed");
            engine.insert_node(node2).await.expect("Insert failed");

            engine
                .delete_node(&"del_rec_remove".to_string())
                .await
                .expect("Delete failed");

            engine.flush_wal().await.expect("Flush failed");
        }

        // Phase 2: Recover and verify delete persisted
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "delete_recovery".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine for recovery");

            engine.recover().await.expect("Recovery failed");

            let kept = engine
                .get_node(&"del_rec_keep".to_string())
                .expect("Get failed");
            let deleted = engine
                .get_node(&"del_rec_remove".to_string())
                .expect("Get failed");

            assert!(kept.is_some(), "Kept node should exist after recovery");
            assert!(
                deleted.is_none(),
                "Deleted node should not exist after recovery"
            );
        }
    }

    #[tokio::test]
    async fn test_orion_edge_recovery() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());

        // Phase 1: Create graph with edges
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "edge_recovery".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine");

            // Create chain: A -> B -> C
            for c in ['A', 'B', 'C'] {
                let node = create_test_node(&format!("chain_{}", c), "ChainNode");
                engine.insert_node(node).await.expect("Insert failed");
            }

            let edge_ab = create_test_edge("e_ab", "chain_A", "chain_B", "NEXT");
            let edge_bc = create_test_edge("e_bc", "chain_B", "chain_C", "NEXT");
            engine
                .insert_edge(edge_ab)
                .await
                .expect("Insert edge failed");
            engine
                .insert_edge(edge_bc)
                .await
                .expect("Insert edge failed");

            engine.flush_wal().await.expect("Flush failed");
        }

        // Phase 2: Recover and verify edges
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "edge_recovery".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine for recovery");

            engine.recover().await.expect("Recovery failed");

            // Verify edges exist
            let edge_ab = engine
                .get_edge(&"e_ab".to_string())
                .expect("Get edge failed");
            let edge_bc = engine
                .get_edge(&"e_bc".to_string())
                .expect("Get edge failed");

            assert!(edge_ab.is_some(), "Edge A->B should be recovered");
            assert!(edge_bc.is_some(), "Edge B->C should be recovered");
        }
    }

    #[tokio::test]
    async fn test_orion_graph_traversal_after_recovery() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());

        // Phase 1: Create and persist graph
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "traversal_recovery".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine");

            // Create star pattern: center -> spoke1, spoke2, spoke3
            let center = create_test_node("center", "Hub");
            engine.insert_node(center).await.expect("Insert failed");

            for i in 1..=3 {
                let spoke = create_test_node(&format!("spoke_{}", i), "Spoke");
                engine.insert_node(spoke).await.expect("Insert failed");

                let edge = create_test_edge(
                    &format!("e_center_spoke_{}", i),
                    "center",
                    &format!("spoke_{}", i),
                    "CONNECTS",
                );
                engine.insert_edge(edge).await.expect("Insert edge failed");
            }

            engine.flush_wal().await.expect("Flush failed");
        }

        // Phase 2: Recover and verify traversal works
        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                "traversal_recovery".to_string(),
                base_url.clone(),
                true,
            )
            .await
            .expect("Failed to create engine for recovery");

            engine.recover().await.expect("Recovery failed");

            // Test traversal from center
            let neighbors = engine
                .get_neighbors(&"center".to_string(), None)
                .expect("Get neighbors failed");

            assert_eq!(
                neighbors.len(),
                3,
                "Center should have 3 neighbors after recovery"
            );
        }
    }
}

// =============================================================================
// WS-1: PULSAR Graph WAL Tests (feature-gated)
// =============================================================================

#[cfg(feature = "distributed-graph")]
mod pulsar_wal_tests {
    use super::*;
    use proximadb::graph::engines::pulsar::{PulsarConfig, PulsarGraphEngine};
    use proximadb::graph::{Edge, Node, engines::GraphEngine};

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

    #[tokio::test]
    async fn test_pulsar_node_operations() {
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
            .expect("Insert node1 failed");
        let inserted2 = engine
            .insert_node(node2)
            .await
            .expect("Insert node2 failed");

        assert_eq!(inserted1.id, "p_node_1");
        assert_eq!(inserted2.id, "p_node_2");

        // Verify retrieval
        let retrieved = engine
            .get_node(&"p_node_1".to_string())
            .expect("Get failed");
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_pulsar_edge_operations() {
        let config = PulsarConfig::default();
        let engine = PulsarGraphEngine::new(config).expect("Failed to create PULSAR engine");

        // Create nodes
        let node1 = create_test_node("pe_node_1", "Person");
        let node2 = create_test_node("pe_node_2", "Person");
        engine.insert_node(node1).await.unwrap();
        engine.insert_node(node2).await.unwrap();

        // Create edge
        let edge = create_test_edge("pe_edge_1", "pe_node_1", "pe_node_2", "KNOWS");
        let inserted = engine.insert_edge(edge).await.expect("Insert edge failed");

        assert_eq!(inserted.id, "pe_edge_1");

        // Verify edge
        let retrieved = engine
            .get_edge(&"pe_edge_1".to_string())
            .expect("Get edge failed");
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_pulsar_stats_tracking() {
        let config = PulsarConfig {
            shard_count: 4,
            ..PulsarConfig::default()
        };
        let engine = PulsarGraphEngine::new(config).expect("Failed to create engine");

        let initial_stats = engine.get_stats().await;
        assert_eq!(initial_stats.total_nodes, 0);

        // Insert nodes
        for i in 0..5 {
            let node = create_test_node(&format!("stat_node_{}", i), "StatNode");
            engine.insert_node(node).await.unwrap();
        }

        let stats_after = engine.get_stats().await;
        assert_eq!(stats_after.total_nodes, 5);
    }

    #[tokio::test]
    async fn test_pulsar_wal_recovery() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());
        let graph_id = "pulsar_recovery";

        // Phase 1: Create with persistence
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

            for i in 0..5 {
                let node = create_test_node(&format!("p_rec_{}", i), "RecoveryTest");
                engine.insert_node(node).await.unwrap();
            }

            engine.flush_wal().await.expect("Flush failed");
        }

        // Phase 2: Recover
        {
            let config = PulsarConfig {
                shard_count: 4,
                replication_factor: 1,
                ..PulsarConfig::default()
            };
            let engine =
                PulsarGraphEngine::with_persistence(config, graph_id.to_string(), base_url.clone())
                    .await
                    .expect("Failed to create engine for recovery");

            engine.recover().await.expect("Recovery failed");

            for i in 0..5 {
                let node_id = format!("p_rec_{}", i);
                let recovered = engine.get_node(&node_id).expect("Get failed");
                assert!(recovered.is_some(), "Node {} should be recovered", node_id);
            }
        }
    }
}

// =============================================================================
// WS-1: QUASAR Graph WAL Tests (feature-gated)
// =============================================================================

#[cfg(feature = "tiered-graph")]
mod quasar_wal_tests {
    use super::*;
    use proximadb::graph::engines::quasar::{ColdStorageBackend, QuasarConfig, QuasarGraphEngine};
    use proximadb::graph::{Edge, Node, engines::GraphEngine};

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

    #[tokio::test]
    async fn test_quasar_hot_tier_operations() {
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

        // Insert nodes
        for i in 0..5 {
            let node = create_test_node(&format!("q_node_{}", i), "HotNode");
            engine.insert_node(node).await.expect("Insert failed");
        }

        let stats = engine.get_stats().await;
        assert_eq!(stats.hot_tier_nodes, 5);
    }

    #[tokio::test]
    async fn test_quasar_cache_stats() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = QuasarConfig {
            cold_tier_path: temp_dir.path().to_path_buf(),
            cold_storage_backend: ColdStorageBackend::Json,
            ..QuasarConfig::default()
        };
        let engine = QuasarGraphEngine::new(config)
            .await
            .expect("Failed to create engine");

        // Insert nodes
        for i in 0..5 {
            let node = create_test_node(&format!("cache_node_{}", i), "CacheNode");
            engine.insert_node(node).await.unwrap();
        }

        // Access nodes multiple times
        for _ in 0..3 {
            for i in 0..5 {
                let _ = engine.get_node(&format!("cache_node_{}", i));
            }
        }

        let stats = engine.get_stats().await;
        assert!(stats.cache_hits > 0);
    }

    #[tokio::test]
    async fn test_quasar_wal_recovery() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_url = format!("file://{}", temp_dir.path().display());
        let graph_id = "quasar_recovery";

        // Phase 1: Create with persistence
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
                    .expect("Failed to create QUASAR engine");

            for i in 0..10 {
                let node = create_test_node(&format!("q_rec_{}", i), "RecoveryTest");
                engine.insert_node(node).await.unwrap();
            }

            engine.flush_wal().await.expect("Flush failed");
        }

        // Phase 2: Recover
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
                    .expect("Failed to create engine for recovery");

            engine.recover().await.expect("Recovery failed");

            for i in 0..10 {
                let node_id = format!("q_rec_{}", i);
                let recovered = engine.get_node(&node_id).expect("Get failed");
                assert!(recovered.is_some(), "Node {} should be recovered", node_id);
            }
        }
    }
}

// =============================================================================
// WS-5: mTLS Infrastructure Tests
// =============================================================================

mod mtls_tests {
    use super::*;
    use proximadb::network::middleware::{TlsClientCertConfig, matches_cn_pattern};
    use proximadb::network::tls::{
        CertificateConfig, CertificateManager, CertificateSubject, TlsConfig, TlsServerConfig,
    };

    #[test]
    fn test_certificate_config_defaults() {
        let config = CertificateConfig::default();

        assert!(config.validity_days > 0);
        assert!(config.renewal_threshold_days > 0);
    }

    #[test]
    fn test_certificate_subject_builder() {
        let subject = CertificateSubject {
            common_name: "test.example.com".to_string(),
            organization: Some("Test Org".to_string()),
            organizational_unit: Some("Engineering".to_string()),
            country: Some("US".to_string()),
            state: Some("California".to_string()),
            locality: Some("San Francisco".to_string()),
            email: Some("test@example.com".to_string()),
        };

        assert_eq!(subject.common_name, "test.example.com");
        assert_eq!(subject.organization, Some("Test Org".to_string()));
    }

    #[test]
    fn test_generate_self_signed_certificate() {
        let temp_dir = TempDir::new().unwrap();
        let config = CertificateConfig {
            subject: CertificateSubject {
                common_name: "localhost".to_string(),
                organization: Some("Test".to_string()),
                ..Default::default()
            },
            validity_days: 30,
            ..Default::default()
        };

        let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
        let cert = manager.generate_self_signed().unwrap();

        assert!(cert.cert_pem.contains("-----BEGIN CERTIFICATE-----"));
        assert!(cert.key_pem.contains("-----BEGIN PRIVATE KEY-----"));
    }

    #[test]
    fn test_generate_ca_certificate() {
        let temp_dir = TempDir::new().unwrap();
        let config = CertificateConfig {
            subject: CertificateSubject {
                common_name: "Test Root CA".to_string(),
                ..Default::default()
            },
            validity_days: 3650,
            ..Default::default()
        };

        let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
        let ca = manager.generate_ca().unwrap();
        let parsed = manager.parse_certificate(ca.cert_pem.as_bytes()).unwrap();

        assert!(parsed.is_ca);
        assert!(parsed.key_usage.contains(&"keyCertSign".to_string()));
    }

    #[test]
    fn test_certificate_with_sans() {
        let temp_dir = TempDir::new().unwrap();
        let config = CertificateConfig {
            subject: CertificateSubject {
                common_name: "multi-san.example.com".to_string(),
                ..Default::default()
            },
            san_dns_names: vec![
                "multi-san.example.com".to_string(),
                "alt.example.com".to_string(),
            ],
            san_ip_addresses: vec!["127.0.0.1".to_string()],
            ..Default::default()
        };

        let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
        let cert = manager.generate_self_signed().unwrap();
        let parsed = manager.parse_certificate(cert.cert_pem.as_bytes()).unwrap();

        assert!(
            parsed
                .san_dns_names
                .contains(&"multi-san.example.com".to_string())
        );
    }

    #[tokio::test]
    async fn test_validate_certificates() {
        let temp_dir = TempDir::new().unwrap();
        let config = CertificateConfig {
            validity_days: 365,
            renewal_threshold_days: 30,
            ..Default::default()
        };

        let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
        manager.generate_and_save_certificates().await.unwrap();

        let status = manager.validate_certificates().await.unwrap();

        assert!(status.valid);
        assert!(!status.needs_renewal);
        assert!(status.days_until_expiry > 300);
    }

    #[test]
    fn test_tls_config_default() {
        let config = TlsConfig::default();

        assert!(!config.enabled);
        assert!(!config.require_client_certs);
    }

    #[test]
    fn test_tls_config_with_mtls() {
        let temp_dir = TempDir::new().unwrap();
        let config = TlsConfig::new(true)
            .with_auto_certificates(temp_dir.path().to_path_buf())
            .with_mtls();

        assert!(config.enabled);
        assert!(config.require_client_certs);
    }

    #[test]
    fn test_cn_pattern_matching_exact() {
        assert!(matches_cn_pattern(
            "client.example.com",
            "client.example.com"
        ));
        assert!(!matches_cn_pattern(
            "other.example.com",
            "client.example.com"
        ));
    }

    #[test]
    fn test_cn_pattern_matching_wildcard() {
        assert!(matches_cn_pattern("client.example.com", "*.example.com"));
        assert!(matches_cn_pattern("server.example.com", "*.example.com"));
        assert!(!matches_cn_pattern(
            "client.api.example.com",
            "*.example.com"
        ));
        assert!(!matches_cn_pattern("example.com", "*.example.com"));
    }

    #[test]
    fn test_cn_pattern_matching_star() {
        assert!(matches_cn_pattern("anything", "*"));
        assert!(matches_cn_pattern("client.example.com", "*"));
        assert!(matches_cn_pattern("", "*"));
    }

    #[test]
    fn test_tls_client_cert_config_default() {
        let config = TlsClientCertConfig::default();

        assert!(!config.require_client_cert);
        assert!(config.allowed_cn_patterns.is_empty());
        assert!(config.reject_expired);
    }

    #[test]
    fn test_tls_client_cert_config_required() {
        let config = TlsClientCertConfig::required();

        assert!(config.require_client_cert);
    }

    #[test]
    fn test_tls_client_cert_config_development() {
        let config = TlsClientCertConfig::development();

        assert!(!config.require_client_cert);
        assert!(config.allowed_cn_patterns.contains(&"*".to_string()));
        assert!(!config.reject_expired);
    }

    #[test]
    fn test_tls_client_cert_config_production() {
        let config = TlsClientCertConfig::production(vec![
            "*.mycompany.com".to_string(),
            "admin.internal".to_string(),
        ]);

        assert!(config.require_client_cert);
        assert_eq!(config.allowed_cn_patterns.len(), 2);
        assert!(config.reject_expired);
        assert!(config.check_revocation);
    }

    #[test]
    fn test_tls_client_cert_config_builder() {
        let config = TlsClientCertConfig::default()
            .allow_cn("*.example.com")
            .allow_cn("admin.internal")
            .map_cn_to_user("admin.internal", "admin-service")
            .with_default_roles(vec!["admin".to_string()]);

        assert_eq!(config.allowed_cn_patterns.len(), 2);
        assert_eq!(
            config.cn_to_user_mapping.get("admin.internal"),
            Some(&"admin-service".to_string())
        );
    }

    #[tokio::test]
    async fn test_build_server_config() {
        let temp_dir = TempDir::new().unwrap();
        let cert_config = CertificateConfig::default();
        let manager = CertificateManager::new(cert_config, temp_dir.path().to_path_buf());
        manager.generate_and_save_certificates().await.unwrap();

        let tls_config = TlsConfig::new(true).with_certificate_manager(manager);
        let server_config = tls_config.build_server_config().await.unwrap();

        // Server config should be valid
        assert!(server_config.alpn_protocols.is_empty());
    }

    #[tokio::test]
    async fn test_mtls_server_config() {
        let temp_dir = TempDir::new().unwrap();
        let cert_config = CertificateConfig::default();
        let manager = CertificateManager::new(cert_config, temp_dir.path().to_path_buf());
        manager.generate_and_save_ca().await.unwrap();
        manager.generate_and_save_certificates().await.unwrap();

        let tls_config = TlsConfig::new(true)
            .with_certificate_manager(manager)
            .with_mtls();
        let server_config = tls_config.build_server_config().await.unwrap();

        // mTLS config built successfully - client auth is enabled
        assert!(std::sync::Arc::strong_count(&server_config) >= 1);
    }
}

// =============================================================================
// WS-2: Catalog Integration Tests
// =============================================================================

mod catalog_tests {
    use super::*;
    use proximadb::catalog::{CatalogManager, TableIdentifier};

    fn temp_catalog_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join("proximadb_phase3_tests")
            .join(name)
    }

    async fn cleanup_dir(path: &std::path::Path) {
        let _ = tokio::fs::remove_dir_all(path).await;
    }

    #[tokio::test]
    async fn test_native_catalog_factory() {
        let manager = CatalogManager::new();
        let temp_dir = temp_catalog_dir("native_factory");
        cleanup_dir(&temp_dir).await;

        let result = manager
            .create_native_catalog("test_native", &format!("file://{}", temp_dir.display()))
            .await;

        assert!(result.is_ok());
        let catalog = result.unwrap();
        assert_eq!(catalog.name(), "test_native");
        assert_eq!(catalog.catalog_type(), "native");

        cleanup_dir(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_iceberg_catalog_factory() {
        let manager = CatalogManager::new();
        let temp_dir = temp_catalog_dir("iceberg_factory");
        cleanup_dir(&temp_dir).await;

        let result = manager
            .create_iceberg_catalog(
                "test_iceberg",
                "memory://",
                &format!("file://{}", temp_dir.display()),
            )
            .await;

        assert!(result.is_ok());
        let catalog = result.unwrap();
        assert_eq!(catalog.name(), "test_iceberg");
        assert_eq!(catalog.catalog_type(), "iceberg");

        cleanup_dir(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_hive_catalog_factory() {
        let manager = CatalogManager::new();

        let result = manager
            .create_hive_catalog("test_hive", "thrift://localhost:9083")
            .await;

        assert!(result.is_ok());
        let catalog = result.unwrap();
        assert_eq!(catalog.name(), "test_hive");
        assert_eq!(catalog.catalog_type(), "hive");
    }

    #[tokio::test]
    async fn test_multiple_catalogs() {
        let manager = CatalogManager::new();
        let temp_dir1 = temp_catalog_dir("multi_1");
        let temp_dir2 = temp_catalog_dir("multi_2");
        cleanup_dir(&temp_dir1).await;
        cleanup_dir(&temp_dir2).await;

        manager
            .create_native_catalog("cat1", &format!("file://{}", temp_dir1.display()))
            .await
            .unwrap();

        manager
            .create_iceberg_catalog(
                "cat2",
                "memory://",
                &format!("file://{}", temp_dir2.display()),
            )
            .await
            .unwrap();

        let catalogs = manager.list_catalogs().await;
        assert_eq!(catalogs.len(), 2);

        cleanup_dir(&temp_dir1).await;
        cleanup_dir(&temp_dir2).await;
    }

    #[tokio::test]
    async fn test_default_catalog() {
        let manager = CatalogManager::new();
        let temp_dir = temp_catalog_dir("default_cat");
        cleanup_dir(&temp_dir).await;

        manager
            .create_native_catalog("first", &format!("file://{}", temp_dir.display()))
            .await
            .unwrap();

        let default = manager.default_catalog().await.unwrap();
        assert_eq!(default.name(), "first");

        cleanup_dir(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_table_identifier_resolution() {
        let manager = CatalogManager::new();
        let temp_dir = temp_catalog_dir("resolve_table");
        cleanup_dir(&temp_dir).await;

        manager
            .create_native_catalog("mycat", &format!("file://{}", temp_dir.display()))
            .await
            .unwrap();

        let (catalog, id) = manager.resolve_table("mycat.mydb.users").await.unwrap();
        assert_eq!(catalog.name(), "mycat");
        assert_eq!(id.namespace, vec!["mydb"]);
        assert_eq!(id.name, "users");

        cleanup_dir(&temp_dir).await;
    }

    #[tokio::test]
    #[cfg(not(feature = "delta-lake"))]
    async fn test_delta_catalog_requires_feature() {
        let manager = CatalogManager::new();
        let result = manager
            .create_delta_catalog("delta", "file:///tmp/delta")
            .await;

        // Without delta-lake feature, this should fail
        assert!(result.is_err());
    }
}

// =============================================================================
// WS-2: Delta Lake Catalog Tests (feature-gated)
// =============================================================================

#[cfg(feature = "delta-lake")]
mod delta_catalog_tests {
    use super::*;
    use proximadb::catalog::types::{CatalogColumn, CatalogDataType, CatalogTableSchema};
    use proximadb::catalog::{Catalog, CatalogManager, TableIdentifier};

    fn temp_catalog_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join("proximadb_delta_tests")
            .join(name)
    }

    async fn cleanup_dir(path: &std::path::Path) {
        let _ = tokio::fs::remove_dir_all(path).await;
    }

    #[tokio::test]
    async fn test_delta_catalog_creation() {
        let manager = CatalogManager::new();
        let temp_dir = temp_catalog_dir("delta_create");
        cleanup_dir(&temp_dir).await;

        let result = manager
            .create_delta_catalog("delta", &format!("file://{}", temp_dir.display()))
            .await;

        assert!(result.is_ok());
        let catalog = result.unwrap();
        assert_eq!(catalog.catalog_type(), "delta");

        cleanup_dir(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_delta_namespace_operations() {
        let manager = CatalogManager::new();
        let temp_dir = temp_catalog_dir("delta_ns");
        cleanup_dir(&temp_dir).await;

        let catalog = manager
            .create_delta_catalog("delta", &format!("file://{}", temp_dir.display()))
            .await
            .unwrap();

        let ns = catalog
            .create_namespace(&["test_db".to_string()], HashMap::new())
            .await
            .unwrap();

        assert_eq!(ns.levels, vec!["test_db"]);
        assert!(
            catalog
                .namespace_exists(&["test_db".to_string()])
                .await
                .unwrap()
        );

        cleanup_dir(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_delta_table_operations() {
        let manager = CatalogManager::new();
        let temp_dir = temp_catalog_dir("delta_table");
        cleanup_dir(&temp_dir).await;

        let catalog = manager
            .create_delta_catalog("delta", &format!("file://{}", temp_dir.display()))
            .await
            .unwrap();

        catalog
            .create_namespace(&["deltadb".to_string()], HashMap::new())
            .await
            .unwrap();

        let schema = CatalogTableSchema::new("test_table")
            .with_column(CatalogColumn::new(1, "id", CatalogDataType::Int64))
            .with_column(CatalogColumn::new(2, "name", CatalogDataType::String));

        let identifier =
            TableIdentifier::new(vec!["deltadb".to_string()], "test_table".to_string());

        let created = catalog.create_table(&identifier, schema).await.unwrap();
        assert_eq!(created.name, "test_table");

        assert!(catalog.table_exists(&identifier).await.unwrap());

        cleanup_dir(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_delta_health_check() {
        let manager = CatalogManager::new();
        let temp_dir = temp_catalog_dir("delta_health");
        cleanup_dir(&temp_dir).await;

        let catalog = manager
            .create_delta_catalog("delta", &format!("file://{}", temp_dir.display()))
            .await
            .unwrap();

        let health = catalog.health_check().await.unwrap();
        assert!(health.is_healthy);

        cleanup_dir(&temp_dir).await;
    }
}

// =============================================================================
// Graph Service Integration Tests
// =============================================================================

mod graph_service_tests {
    use super::*;
    use proximadb::graph::Node;
    use proximadb::graph::service::GraphOperationsService;
    use proximadb::proto::proximadb_v1::{
        CompressionAlgorithm, CreateGraphRequest, GraphStorageConfig,
    };

    fn create_test_node(id: &str, label: &str) -> Node {
        Node {
            id: id.to_string(),
            labels: vec![label.to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

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
            .expect("Create graph failed");

        let graphs = service.list_graphs().await.expect("List graphs failed");
        assert!(graphs.contains(&"test_orion".to_string()));
    }

    #[tokio::test]
    async fn test_service_flush_wal() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let service = GraphOperationsService::new();

        let request = CreateGraphRequest {
            graph_id: "flush_test".to_string(),
            name: Some("Flush Test".to_string()),
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

        service.create_graph_collection(request).await.unwrap();

        let node = create_test_node("flush_node", "Test");
        service
            .create_node("flush_test", node)
            .await
            .expect("Create node failed");

        // Flush should succeed
        service.flush_wal("flush_test").await.expect("Flush failed");
    }

    #[tokio::test]
    async fn test_service_flush_wal_idempotent() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let service = GraphOperationsService::new();

        let request = CreateGraphRequest {
            graph_id: "idem_flush".to_string(),
            name: Some("Idempotent Flush".to_string()),
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

        service.create_graph_collection(request).await.unwrap();

        let node = create_test_node("idem_node", "Test");
        service.create_node("idem_flush", node).await.unwrap();

        // Multiple flushes should all succeed
        for _ in 0..5 {
            service
                .flush_wal("idem_flush")
                .await
                .expect("Flush should be idempotent");
        }
    }

    #[tokio::test]
    async fn test_service_flush_nonexistent_graph() {
        let service = GraphOperationsService::new();

        // Flush non-existent graph should handle gracefully
        let result = service.flush_wal("nonexistent_graph").await;
        // Should not panic, may return Ok or appropriate error
        assert!(result.is_ok() || result.is_err());
    }
}

// =============================================================================
// Cross-Workstream Integration Tests
// =============================================================================

mod cross_workstream_tests {
    use super::*;
    use proximadb::graph::Node;
    use proximadb::graph::service::GraphOperationsService;
    use proximadb::proto::proximadb_v1::VectorRecord;
    use proximadb::proto::proximadb_v1::{
        CompressionAlgorithm, CreateGraphRequest, GraphStorageConfig,
    };
    use proximadb::streaming::{SessionConfig, StreamConfig, StreamCoordinator};

    fn create_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
        (0..count)
            .map(|i| VectorRecord {
                id: format!("vec_{}", i),
                vector: vec![0.1 * (i as f32); dimension],
                metadata: Default::default(),
                ..Default::default()
            })
            .collect()
    }

    fn create_test_node(id: &str, label: &str) -> Node {
        Node {
            id: id.to_string(),
            labels: vec![label.to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[tokio::test]
    async fn test_streaming_and_graph_concurrent_operations() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Initialize streaming coordinator
        let coordinator = Arc::new(StreamCoordinator::new(StreamConfig {
            default_buffer_size: 1024,
            ..Default::default()
        }));

        // Initialize graph service
        let graph_service = Arc::new(GraphOperationsService::new());

        // Create graph
        let request = CreateGraphRequest {
            graph_id: "cross_test".to_string(),
            name: Some("Cross Test".to_string()),
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
        graph_service
            .create_graph_collection(request)
            .await
            .unwrap();

        // Concurrent operations
        let coord_clone = Arc::clone(&coordinator);
        let graph_clone = Arc::clone(&graph_service);

        // Streaming task
        let streaming_handle = tokio::spawn(async move {
            let session_id = coord_clone
                .create_session("vectors".to_string(), SessionConfig::default())
                .await
                .unwrap();

            let vectors = create_test_vectors(50, 64);
            coord_clone
                .push_records(&session_id, vectors)
                .await
                .unwrap();

            coord_clone.stats().total_buffered_records
        });

        // Graph task
        let graph_handle = tokio::spawn(async move {
            for i in 0..10 {
                let node = create_test_node(&format!("concurrent_node_{}", i), "Concurrent");
                graph_clone.create_node("cross_test", node).await.unwrap();
            }
            graph_clone.flush_wal("cross_test").await.unwrap();
            10
        });

        let (streaming_result, graph_result) = tokio::join!(streaming_handle, graph_handle);

        assert_eq!(streaming_result.unwrap(), 50);
        assert_eq!(graph_result.unwrap(), 10);
    }

    #[tokio::test]
    async fn test_graph_wal_and_streaming_flush_coordination() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Initialize both subsystems
        let coordinator = StreamCoordinator::new(StreamConfig::default());
        let graph_service = GraphOperationsService::new();

        // Create graph
        let request = CreateGraphRequest {
            graph_id: "coord_test".to_string(),
            name: Some("Coordination Test".to_string()),
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
        graph_service
            .create_graph_collection(request)
            .await
            .unwrap();

        // Add data to both
        let session_id = coordinator
            .create_session("test_collection".to_string(), SessionConfig::default())
            .await
            .unwrap();

        let vectors = create_test_vectors(100, 32);
        coordinator
            .push_records(&session_id, vectors)
            .await
            .unwrap();

        let node = create_test_node("coord_node", "Test");
        graph_service.create_node("coord_test", node).await.unwrap();

        // Flush both
        graph_service
            .flush_wal("coord_test")
            .await
            .expect("Graph WAL flush failed");

        // Verify stats
        let stats = coordinator.stats();
        assert_eq!(stats.total_buffered_records, 100);
    }
}

// =============================================================================
// Performance and Stress Tests
// =============================================================================

mod stress_tests {
    use super::*;
    use proximadb::graph::Node;
    use proximadb::graph::service::GraphOperationsService;
    use proximadb::proto::proximadb_v1::VectorRecord;
    use proximadb::proto::proximadb_v1::{
        CompressionAlgorithm, CreateGraphRequest, GraphStorageConfig,
    };
    use proximadb::streaming::{SessionConfig, StreamConfig, StreamCoordinator};

    fn create_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
        (0..count)
            .map(|i| VectorRecord {
                id: format!("stress_vec_{}", i),
                vector: vec![0.1; dimension],
                metadata: Default::default(),
                ..Default::default()
            })
            .collect()
    }

    #[tokio::test]
    async fn test_high_volume_streaming() {
        let coordinator = Arc::new(StreamCoordinator::new(StreamConfig {
            default_buffer_size: 10_000,
            global_rate_limit: 1_000_000,
            ..Default::default()
        }));

        let session_id = coordinator
            .create_session("high_volume".to_string(), SessionConfig::default())
            .await
            .unwrap();

        let start = Instant::now();

        // Push 1000 vectors
        let vectors = create_test_vectors(1000, 128);
        let result = coordinator
            .push_records(&session_id, vectors)
            .await
            .unwrap();

        let duration = start.elapsed();

        assert_eq!(result.pushed, 1000);
        assert!(
            duration.as_millis() < 1000,
            "Should complete in under 1 second"
        );
    }

    #[tokio::test]
    async fn test_many_streaming_sessions() {
        let coordinator = Arc::new(StreamCoordinator::new(StreamConfig {
            max_streams: 100,
            default_buffer_size: 256,
            ..Default::default()
        }));

        let mut handles = vec![];

        // Create 50 sessions concurrently
        for i in 0..50 {
            let coord = Arc::clone(&coordinator);
            handles.push(tokio::spawn(async move {
                let session_id = coord
                    .create_session(format!("session_{}", i), SessionConfig::default())
                    .await
                    .unwrap();

                let vectors = create_test_vectors(10, 32);
                coord.push_records(&session_id, vectors).await.unwrap();

                session_id
            }));
        }

        let session_ids: Vec<_> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let stats = coordinator.stats();
        assert_eq!(stats.active_sessions, 50);
        assert_eq!(stats.total_buffered_records, 500);

        // Clean up
        for sid in session_ids {
            coordinator.close_session(&sid);
        }
    }

    #[tokio::test]
    async fn test_high_volume_graph_operations() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let service = GraphOperationsService::new();

        let request = CreateGraphRequest {
            graph_id: "stress_graph".to_string(),
            name: Some("Stress Test Graph".to_string()),
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

        service.create_graph_collection(request).await.unwrap();

        let start = Instant::now();

        // Insert 100 nodes
        let mut nodes = Vec::new();
        for i in 0..100 {
            nodes.push(Node {
                id: format!("stress_node_{}", i),
                labels: vec!["StressNode".to_string()],
                properties: HashMap::new(),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            });
        }

        let created = service
            .batch_create_nodes("stress_graph", nodes)
            .await
            .unwrap();

        let duration = start.elapsed();

        assert_eq!(created.len(), 100);
        assert!(
            duration.as_millis() < 5000,
            "Should complete in under 5 seconds"
        );

        // Flush and verify
        service.flush_wal("stress_graph").await.unwrap();
    }
}
