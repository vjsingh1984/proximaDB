/*
 * Copyright 2025 ProximaDB
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

//! Integration tests for distributed graph features
//!
//! Tests the following components:
//! - Graph RAFT consensus
//! - Two-phase commit transactions
//! - Multi-region coordination
//! - Cross-shard query execution

use proximadb::cluster::consensus::ConsensusConfig;
use proximadb::graph::engines::orion::OrionGraphEngine;
use proximadb::graph::engines::pulsar::consensus::{GraphCommand, GraphRaftNode};
use proximadb::graph::engines::pulsar::regions::{
    MultiRegionCoordinator, RegionConfig, RegionManager, ReplicationStrategy,
};
use proximadb::graph::engines::pulsar::transactions::{
    GraphOperation, TransactionCoordinator, TransactionState, TwoPhaseCommitCoordinator,
};
use proximadb::graph::engines::GraphEngine;
use proximadb::proto::proximadb_v1::Node;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Test helper to create a test ORION engine (in-memory, no persistence)
fn create_test_orion_engine(_graph_id: &str) -> Arc<OrionGraphEngine> {
    // Use in-memory engine for distributed tests (no persistence needed)
    Arc::new(OrionGraphEngine::new())
}

// =============================================================================
// RAFT Consensus Tests
// =============================================================================

#[tokio::test]
async fn test_raft_node_creation_and_leadership() {
    let config = ConsensusConfig::default();
    let engine = create_test_orion_engine("test_graph");

    let raft_node = GraphRaftNode::new(config, engine as Arc<dyn GraphEngine>).unwrap();

    // Verify node is created
    assert!(!raft_node.node_id().is_empty());

    // Single-node cluster is always leader
    assert!(raft_node.is_leader().await);

    // Leader ID should be self
    let leader_id = raft_node.get_leader_id().await;
    assert!(leader_id.is_some());
    assert_eq!(leader_id.unwrap(), raft_node.node_id());
}

#[tokio::test]
async fn test_raft_submit_create_node_command() {
    let config = ConsensusConfig::default();
    let engine = create_test_orion_engine("test_graph");

    let raft_node = GraphRaftNode::new(config, engine).unwrap();

    // Create a node command
    let node = Node {
        id: "node_1".to_string(),
        labels: vec!["Person".to_string()],
        properties: HashMap::new(),
        ..Default::default()
    };

    let command = GraphCommand::CreateNode {
        graph_id: "test_graph".to_string(),
        node,
    };

    // Submit command
    let result = raft_node.submit_command(command).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_raft_bulk_operations() {
    let config = ConsensusConfig::default();
    let engine = create_test_orion_engine("test_graph");

    let raft_node = GraphRaftNode::new(config, engine).unwrap();

    // Create bulk nodes command
    let nodes: Vec<Node> = (0..10)
        .map(|i| Node {
            id: format!("node_{}", i),
            labels: vec!["Test".to_string()],
            properties: HashMap::new(),
            ..Default::default()
        })
        .collect();

    let command = GraphCommand::BulkCreateNodes {
        graph_id: "test_graph".to_string(),
        nodes,
    };

    // Submit bulk command
    let result = raft_node.submit_command(command).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_raft_noop_command() {
    let config = ConsensusConfig::default();
    let engine = create_test_orion_engine("test_graph");

    let raft_node = GraphRaftNode::new(config, engine).unwrap();

    // Submit no-op command (used for leader establishment)
    let command = GraphCommand::Noop;
    let result = raft_node.submit_command(command).await;
    assert!(result.is_ok());
}

// =============================================================================
// Two-Phase Commit Transaction Tests
// =============================================================================

#[tokio::test]
async fn test_2pc_begin_transaction() {
    let shard1 = create_test_orion_engine("shard1");
    let shard2 = create_test_orion_engine("shard2");

    let mut shards = HashMap::new();
    shards.insert("shard1".to_string(), shard1 as Arc<dyn GraphEngine>);
    shards.insert("shard2".to_string(), shard2 as Arc<dyn GraphEngine>);

    let coordinator = TwoPhaseCommitCoordinator::new(shards, Duration::from_secs(30));

    // Begin transaction
    let tx_id = coordinator
        .begin_transaction(vec!["shard1".to_string(), "shard2".to_string()])
        .await
        .unwrap();

    assert!(!tx_id.is_empty());

    // Check transaction state
    let state = coordinator.get_state(&tx_id).await.unwrap();
    assert_eq!(state, TransactionState::Active);
}

#[tokio::test]
async fn test_2pc_execute_and_commit() {
    let shard1 = create_test_orion_engine("shard1");
    let mut shards = HashMap::new();
    shards.insert("shard1".to_string(), shard1 as Arc<dyn GraphEngine>);

    let coordinator = TwoPhaseCommitCoordinator::new(shards, Duration::from_secs(30));

    // Begin transaction
    let tx_id = coordinator
        .begin_transaction(vec!["shard1".to_string()])
        .await
        .unwrap();

    // Execute operation
    let node = Node {
        id: "tx_node_1".to_string(),
        labels: vec!["TxTest".to_string()],
        properties: HashMap::new(),
        ..Default::default()
    };

    let op = GraphOperation::InsertNode {
        shard_id: "shard1".to_string(),
        node,
    };

    coordinator
        .execute_operation(tx_id.clone(), op)
        .await
        .unwrap();

    // Commit transaction
    let commit_result = coordinator.commit(tx_id.clone()).await;
    assert!(commit_result.is_ok());

    // Verify committed state
    let state = coordinator.get_state(&tx_id).await.unwrap();
    assert_eq!(state, TransactionState::Committed);
}

#[tokio::test]
async fn test_2pc_abort_transaction() {
    let shard1 = create_test_orion_engine("shard1");
    let mut shards = HashMap::new();
    shards.insert("shard1".to_string(), shard1 as Arc<dyn GraphEngine>);

    let coordinator = TwoPhaseCommitCoordinator::new(shards, Duration::from_secs(30));

    // Begin transaction
    let tx_id = coordinator
        .begin_transaction(vec!["shard1".to_string()])
        .await
        .unwrap();

    // Execute operation
    let node = Node {
        id: "abort_node".to_string(),
        labels: vec!["Test".to_string()],
        properties: HashMap::new(),
        ..Default::default()
    };

    let op = GraphOperation::InsertNode {
        shard_id: "shard1".to_string(),
        node,
    };

    coordinator
        .execute_operation(tx_id.clone(), op)
        .await
        .unwrap();

    // Abort transaction
    coordinator.abort(tx_id.clone()).await.unwrap();

    // Verify aborted state
    let state = coordinator.get_state(&tx_id).await.unwrap();
    assert_eq!(state, TransactionState::Aborted);
}

#[tokio::test]
async fn test_2pc_cross_shard_transaction() {
    let shard1 = create_test_orion_engine("shard1");
    let shard2 = create_test_orion_engine("shard2");

    let mut shards = HashMap::new();
    shards.insert("shard1".to_string(), shard1 as Arc<dyn GraphEngine>);
    shards.insert("shard2".to_string(), shard2 as Arc<dyn GraphEngine>);

    let coordinator = TwoPhaseCommitCoordinator::new(shards, Duration::from_secs(30));

    // Begin cross-shard transaction
    let tx_id = coordinator
        .begin_transaction(vec!["shard1".to_string(), "shard2".to_string()])
        .await
        .unwrap();

    // Execute operations on both shards
    let node1 = Node {
        id: "cross_node_1".to_string(),
        labels: vec!["CrossShard".to_string()],
        properties: HashMap::new(),
        ..Default::default()
    };

    let op1 = GraphOperation::InsertNode {
        shard_id: "shard1".to_string(),
        node: node1,
    };

    coordinator
        .execute_operation(tx_id.clone(), op1)
        .await
        .unwrap();

    let node2 = Node {
        id: "cross_node_2".to_string(),
        labels: vec!["CrossShard".to_string()],
        properties: HashMap::new(),
        ..Default::default()
    };

    let op2 = GraphOperation::InsertNode {
        shard_id: "shard2".to_string(),
        node: node2,
    };

    coordinator
        .execute_operation(tx_id.clone(), op2)
        .await
        .unwrap();

    // Commit cross-shard transaction
    let result = coordinator.commit(tx_id.clone()).await;
    assert!(result.is_ok());

    let state = coordinator.get_state(&tx_id).await.unwrap();
    assert_eq!(state, TransactionState::Committed);
}

// =============================================================================
// Multi-Region Coordinator Tests
// =============================================================================

#[tokio::test]
async fn test_multi_region_creation() {
    let peer_regions = vec![
        RegionConfig {
            id: "us-east-1".to_string(),
            name: "US East".to_string(),
            location: (39.0, -77.0),
            endpoint: "https://us-east-1.example.com".to_string(),
            active: true,
            read_priority: 1,
        },
        RegionConfig {
            id: "eu-west-1".to_string(),
            name: "EU West".to_string(),
            location: (53.0, -8.0),
            endpoint: "https://eu-west-1.example.com".to_string(),
            active: true,
            read_priority: 2,
        },
    ];

    let coordinator = MultiRegionCoordinator::new(
        "us-west-1".to_string(),
        peer_regions,
        100,
        1000,
        ReplicationStrategy::Asynchronous,
    );

    // Verify local region
    let local = coordinator.get_local_region().await.unwrap();
    assert_eq!(local, "us-west-1");

    // Verify peer regions
    let peers = coordinator.get_peer_regions().await.unwrap();
    assert_eq!(peers.len(), 2);
}

#[tokio::test]
async fn test_multi_region_add_remove_peers() {
    let coordinator = MultiRegionCoordinator::new(
        "us-west-1".to_string(),
        vec![],
        100,
        1000,
        ReplicationStrategy::Asynchronous,
    );

    // Add peer region
    let region = RegionConfig {
        id: "ap-south-1".to_string(),
        name: "Asia Pacific".to_string(),
        location: (19.0, 72.0),
        endpoint: "https://ap-south-1.example.com".to_string(),
        active: true,
        read_priority: 3,
    };

    coordinator.add_peer_region(region).await.unwrap();

    let peers = coordinator.get_peer_regions().await.unwrap();
    assert_eq!(peers.len(), 1);

    // Remove peer region
    coordinator
        .remove_peer_region(&"ap-south-1".to_string())
        .await
        .unwrap();

    let peers = coordinator.get_peer_regions().await.unwrap();
    assert_eq!(peers.len(), 0);
}

#[tokio::test]
async fn test_multi_region_geo_routing() {
    let peer_regions = vec![
        RegionConfig {
            id: "us-east-1".to_string(),
            name: "US East".to_string(),
            location: (39.0, -77.0), // Virginia
            endpoint: "https://us-east-1.example.com".to_string(),
            active: true,
            read_priority: 1,
        },
        RegionConfig {
            id: "eu-west-1".to_string(),
            name: "EU West".to_string(),
            location: (53.0, -8.0), // Ireland
            endpoint: "https://eu-west-1.example.com".to_string(),
            active: true,
            read_priority: 2,
        },
        RegionConfig {
            id: "ap-northeast-1".to_string(),
            name: "Asia Pacific".to_string(),
            location: (35.7, 139.7), // Tokyo
            endpoint: "https://ap-northeast-1.example.com".to_string(),
            active: true,
            read_priority: 3,
        },
    ];

    let coordinator = MultiRegionCoordinator::new(
        "us-west-1".to_string(),
        peer_regions,
        100,
        1000,
        ReplicationStrategy::Asynchronous,
    );

    // Mark all regions as healthy
    coordinator
        .lag_tracker()
        .update_lag("us-east-1".to_string(), 30, 0);
    coordinator
        .lag_tracker()
        .update_lag("eu-west-1".to_string(), 40, 0);
    coordinator
        .lag_tracker()
        .update_lag("ap-northeast-1".to_string(), 50, 0);

    // Query from New York (should route to us-east-1)
    let region = coordinator
        .route_read_query(Some((40.7, -74.0)))
        .await
        .unwrap();
    assert_eq!(region, "us-east-1");

    // Query from London (should route to eu-west-1)
    let region = coordinator
        .route_read_query(Some((51.5, -0.1)))
        .await
        .unwrap();
    assert_eq!(region, "eu-west-1");

    // Query from Tokyo (should route to ap-northeast-1)
    let region = coordinator
        .route_read_query(Some((35.7, 139.7)))
        .await
        .unwrap();
    assert_eq!(region, "ap-northeast-1");
}

#[tokio::test]
async fn test_multi_region_replication() {
    let peer_regions = vec![RegionConfig {
        id: "us-east-1".to_string(),
        name: "US East".to_string(),
        location: (39.0, -77.0),
        endpoint: "https://us-east-1.example.com".to_string(),
        active: true,
        read_priority: 1,
    }];

    let coordinator = MultiRegionCoordinator::new(
        "us-west-1".to_string(),
        peer_regions,
        100,
        1000,
        ReplicationStrategy::Synchronous,
    );

    // Create operations to replicate
    let node = Node {
        id: "replicated_node".to_string(),
        labels: vec!["Replicated".to_string()],
        properties: HashMap::new(),
        ..Default::default()
    };

    let ops = vec![GraphCommand::CreateNode {
        graph_id: "test_graph".to_string(),
        node,
    }];

    // Replicate to peer region
    let result = coordinator
        .replicate_to_region("us-east-1".to_string(), ops)
        .await;
    assert!(result.is_ok());

    // Verify lag was updated
    let lag = coordinator.get_replication_lag().await.unwrap();
    assert!(!lag.is_empty());
}

#[tokio::test]
async fn test_multi_region_failover() {
    let peer_regions = vec![
        RegionConfig {
            id: "us-east-1".to_string(),
            name: "US East".to_string(),
            location: (39.0, -77.0),
            endpoint: "https://us-east-1.example.com".to_string(),
            active: true,
            read_priority: 1,
        },
        RegionConfig {
            id: "us-east-2".to_string(),
            name: "US East 2".to_string(),
            location: (40.0, -83.0),
            endpoint: "https://us-east-2.example.com".to_string(),
            active: false,
            read_priority: 10,
        },
    ];

    let coordinator = MultiRegionCoordinator::new(
        "us-west-1".to_string(),
        peer_regions,
        100,
        1000,
        ReplicationStrategy::Asynchronous,
    );

    // Promote us-east-2 (simulating failover)
    coordinator
        .promote_region("us-east-2".to_string())
        .await
        .unwrap();

    // Verify us-east-2 is now active with high priority
    let peers = coordinator.get_peer_regions().await.unwrap();
    let promoted = peers.iter().find(|r| r.id == "us-east-2").unwrap();
    assert!(promoted.active);
    assert_eq!(promoted.read_priority, 0);
}

// =============================================================================
// Integration Test: End-to-End Distributed Workflow
// =============================================================================

#[tokio::test]
async fn test_end_to_end_distributed_workflow() {
    // 1. Setup: Create shards with RAFT consensus
    let shard1_engine = create_test_orion_engine("shard1");
    let shard2_engine = create_test_orion_engine("shard2");

    let config = ConsensusConfig::default();
    let raft1 = GraphRaftNode::new(config.clone(), shard1_engine.clone() as Arc<dyn GraphEngine>)
        .unwrap();
    let raft2 = GraphRaftNode::new(config, shard2_engine.clone() as Arc<dyn GraphEngine>).unwrap();

    // Verify both nodes are leaders (single-node clusters)
    assert!(raft1.is_leader().await);
    assert!(raft2.is_leader().await);

    // 2. Setup: Create transaction coordinator
    let mut shards = HashMap::new();
    shards.insert("shard1".to_string(), shard1_engine as Arc<dyn GraphEngine>);
    shards.insert("shard2".to_string(), shard2_engine as Arc<dyn GraphEngine>);

    let tx_coordinator = TwoPhaseCommitCoordinator::new(shards, Duration::from_secs(30));

    // 3. Execute: Distributed transaction
    let tx_id = tx_coordinator
        .begin_transaction(vec!["shard1".to_string(), "shard2".to_string()])
        .await
        .unwrap();

    // Insert nodes into both shards
    let node1 = Node {
        id: "e2e_node_1".to_string(),
        labels: vec!["E2E".to_string()],
        properties: HashMap::new(),
        ..Default::default()
    };

    tx_coordinator
        .execute_operation(
            tx_id.clone(),
            GraphOperation::InsertNode {
                shard_id: "shard1".to_string(),
                node: node1,
            },
        )
        .await
        .unwrap();

    let node2 = Node {
        id: "e2e_node_2".to_string(),
        labels: vec!["E2E".to_string()],
        properties: HashMap::new(),
        ..Default::default()
    };

    tx_coordinator
        .execute_operation(
            tx_id.clone(),
            GraphOperation::InsertNode {
                shard_id: "shard2".to_string(),
                node: node2,
            },
        )
        .await
        .unwrap();

    // 4. Commit: Ensure atomic commit across shards
    tx_coordinator.commit(tx_id.clone()).await.unwrap();

    let state = tx_coordinator.get_state(&tx_id).await.unwrap();
    assert_eq!(state, TransactionState::Committed);

    // 5. Setup: Multi-region coordinator
    let regions = vec![RegionConfig {
        id: "backup-region".to_string(),
        name: "Backup Region".to_string(),
        location: (0.0, 0.0),
        endpoint: "https://backup.example.com".to_string(),
        active: true,
        read_priority: 10,
    }];

    let region_coordinator = MultiRegionCoordinator::new(
        "primary-region".to_string(),
        regions,
        100,
        1000,
        ReplicationStrategy::Asynchronous,
    );

    // 6. Replicate: Send operations to backup region
    let replicate_ops = vec![
        GraphCommand::CreateNode {
            graph_id: "test_graph".to_string(),
            node: Node {
                id: "replicated_1".to_string(),
                labels: vec!["Replicated".to_string()],
                properties: HashMap::new(),
                ..Default::default()
            },
        },
        GraphCommand::CreateNode {
            graph_id: "test_graph".to_string(),
            node: Node {
                id: "replicated_2".to_string(),
                labels: vec!["Replicated".to_string()],
                properties: HashMap::new(),
                ..Default::default()
            },
        },
    ];

    region_coordinator
        .replicate_to_region("backup-region".to_string(), replicate_ops)
        .await
        .unwrap();

    // 7. Verify: Complete workflow succeeded
    assert_eq!(
        region_coordinator.get_local_region().await.unwrap(),
        "primary-region"
    );
    assert_eq!(region_coordinator.get_peer_regions().await.unwrap().len(), 1);
}
