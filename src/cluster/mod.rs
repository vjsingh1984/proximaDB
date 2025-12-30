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

//! Cluster Module for Distributed ProximaDB
//!
//! This module provides the foundational components for distributed operation:
//! - **Metadata Service**: Cluster-wide metadata management
//! - **Node Registry**: Node discovery and health tracking
//! - **Consensus**: Raft-based distributed consensus
//! - **Routing**: Shard-aware request routing
//! - **Shard**: Data sharding and placement
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                    Cluster Manager                       │
//! ├─────────────────────────────────────────────────────────┤
//! │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  │
//! │  │   Metadata   │  │    Node      │  │  Consensus   │  │
//! │  │   Service    │  │   Registry   │  │    (Raft)    │  │
//! │  └──────────────┘  └──────────────┘  └──────────────┘  │
//! ├─────────────────────────────────────────────────────────┤
//! │  ┌──────────────┐  ┌──────────────┐                    │
//! │  │   Routing    │  │    Shard     │                    │
//! │  │   Service    │  │   Manager    │                    │
//! │  └──────────────┘  └──────────────┘                    │
//! └─────────────────────────────────────────────────────────┘
//! ```

pub mod consensus;
pub mod distributed_ops;
pub mod metadata_service;
pub mod node_registry;
pub mod replication;
pub mod routing;
pub mod shard;

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;

pub use consensus::{ConsensusConfig, ConsensusState, RaftConsensus};
pub use distributed_ops::{
    ConsistencyLevel, DistributedCollectionOps, DistributedOpsConfig,
    DistributedSearchRequest, DistributedSearchResult, DistributedWriteRequest,
    DistributedWriteResult, QueryContext, RetryConfig, SearchResult, WriteRecord,
};
pub use metadata_service::{ClusterMetadata, MetadataService, MetadataServiceConfig};
pub use replication::{
    EngineReplication, ReplicationAck, ReplicationConfig, ReplicationEntry,
    ReplicationHealth, ReplicationOperation, ReplicationRetryConfig,
    ReplicationStatsSummary, ReplicaState,
};
pub use node_registry::{NodeHealth, NodeInfo, NodeRegistry, NodeRegistryConfig, NodeRole, NodeStatus};
pub use routing::{RouteContext, RouteDecision, RoutingConfig, RoutingService};
pub use shard::{
    MetadataBounds, PartitionConfig, PartitionStrategy, Shard, ShardConfig, ShardId,
    ShardManager, ShardPlacement, ShardState,
};

/// Cluster configuration
#[derive(Debug, Clone)]
pub struct ClusterConfig {
    /// Unique cluster identifier
    pub cluster_id: String,
    /// This node's identifier
    pub node_id: String,
    /// This node's advertised address
    pub advertise_addr: String,
    /// List of seed nodes for discovery
    pub seed_nodes: Vec<String>,
    /// Metadata service configuration
    pub metadata: MetadataServiceConfig,
    /// Node registry configuration
    pub node_registry: NodeRegistryConfig,
    /// Consensus configuration
    pub consensus: ConsensusConfig,
    /// Routing configuration
    pub routing: RoutingConfig,
    /// Shard configuration
    pub shard: ShardConfig,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            cluster_id: "proximadb-cluster".to_string(),
            node_id: uuid::Uuid::new_v4().to_string(),
            advertise_addr: "127.0.0.1:5679".to_string(),
            seed_nodes: vec![],
            metadata: MetadataServiceConfig::default(),
            node_registry: NodeRegistryConfig::default(),
            consensus: ConsensusConfig::default(),
            routing: RoutingConfig::default(),
            shard: ShardConfig::default(),
        }
    }
}

/// Main cluster manager that coordinates all distributed components
pub struct ClusterManager {
    config: ClusterConfig,
    metadata_service: Arc<MetadataService>,
    node_registry: Arc<NodeRegistry>,
    consensus: Arc<RwLock<RaftConsensus>>,
    routing_service: Arc<RoutingService>,
    shard_manager: Arc<ShardManager>,
    is_leader: Arc<RwLock<bool>>,
}

impl ClusterManager {
    /// Create a new cluster manager
    pub async fn new(config: ClusterConfig) -> Result<Self> {
        let metadata_service = Arc::new(MetadataService::new(config.metadata.clone())?);
        let node_registry = Arc::new(NodeRegistry::new(config.node_registry.clone())?);
        let consensus = Arc::new(RwLock::new(RaftConsensus::new(config.consensus.clone())?));
        let routing_service = Arc::new(RoutingService::new(config.routing.clone())?);
        let shard_manager = Arc::new(ShardManager::new(config.shard.clone())?);

        Ok(Self {
            config,
            metadata_service,
            node_registry,
            consensus,
            routing_service,
            shard_manager,
            is_leader: Arc::new(RwLock::new(false)),
        })
    }

    /// Start the cluster manager and all components
    pub async fn start(&self) -> Result<()> {
        tracing::info!(
            cluster_id = %self.config.cluster_id,
            node_id = %self.config.node_id,
            "Starting cluster manager"
        );

        // Register this node
        let node_info = NodeInfo {
            node_id: self.config.node_id.clone(),
            address: self.config.advertise_addr.clone(),
            role: NodeRole::Follower,
            status: NodeStatus::Starting,
            health: NodeHealth::Unknown,
            ..Default::default()
        };
        self.node_registry.register_node(node_info).await?;

        // Start consensus
        {
            let mut consensus = self.consensus.write().await;
            consensus.start().await?;
        }

        // Update node status
        self.node_registry
            .update_status(&self.config.node_id, NodeStatus::Running)
            .await?;

        tracing::info!("Cluster manager started successfully");
        Ok(())
    }

    /// Stop the cluster manager gracefully
    pub async fn stop(&self) -> Result<()> {
        tracing::info!("Stopping cluster manager");

        // Update node status
        self.node_registry
            .update_status(&self.config.node_id, NodeStatus::Stopping)
            .await?;

        // Stop consensus
        {
            let mut consensus = self.consensus.write().await;
            consensus.stop().await?;
        }

        // Deregister this node
        self.node_registry
            .deregister_node(&self.config.node_id)
            .await?;

        tracing::info!("Cluster manager stopped");
        Ok(())
    }

    /// Check if this node is the current leader
    pub async fn is_leader(&self) -> bool {
        *self.is_leader.read().await
    }

    /// Get the metadata service
    pub fn metadata_service(&self) -> &Arc<MetadataService> {
        &self.metadata_service
    }

    /// Get the node registry
    pub fn node_registry(&self) -> &Arc<NodeRegistry> {
        &self.node_registry
    }

    /// Get the routing service
    pub fn routing_service(&self) -> &Arc<RoutingService> {
        &self.routing_service
    }

    /// Get the shard manager
    pub fn shard_manager(&self) -> &Arc<ShardManager> {
        &self.shard_manager
    }

    /// Get cluster health summary
    pub async fn health(&self) -> ClusterHealth {
        let nodes = self.node_registry.list_nodes().await;
        let healthy_nodes = nodes.iter().filter(|n| n.health == NodeHealth::Healthy).count();
        let total_nodes = nodes.len();

        ClusterHealth {
            cluster_id: self.config.cluster_id.clone(),
            is_leader: self.is_leader().await,
            total_nodes,
            healthy_nodes,
            unhealthy_nodes: total_nodes - healthy_nodes,
            shard_count: self.shard_manager.shard_count().await,
        }
    }
}

/// Cluster health summary
#[derive(Debug, Clone)]
pub struct ClusterHealth {
    pub cluster_id: String,
    pub is_leader: bool,
    pub total_nodes: usize,
    pub healthy_nodes: usize,
    pub unhealthy_nodes: usize,
    pub shard_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cluster_config_default() {
        let config = ClusterConfig::default();
        assert_eq!(config.cluster_id, "proximadb-cluster");
        assert!(!config.node_id.is_empty());
    }

    #[tokio::test]
    async fn test_cluster_manager_creation() {
        let config = ClusterConfig::default();
        let manager = ClusterManager::new(config).await;
        assert!(manager.is_ok());
    }
}
