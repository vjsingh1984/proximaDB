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

//! HMGI Distributed Partition Locator
//!
//! Locates partitions across a distributed cluster using consistent hashing.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::{ConsistentHashRing, HmgiPartitionKey};

/// Node identifier in the cluster
pub type ClusterNodeId = u64;

/// HMGI distributed partition locator
///
/// Determines which node owns which partition using consistent hashing.
/// Uses the shared ConsistentHashRing for uniform distribution.
pub struct DistributedPartitionLocator {
    /// Consistent hash ring for partition placement
    ring: Arc<RwLock<ConsistentHashRing>>,

    /// Cluster membership
    cluster: Arc<RwLock<ClusterMembership>>,

    /// Local partitions owned by this node
    local_partitions: Arc<RwLock<HashMap<String, HmgiPartitionKey>>>,

    /// Local node ID
    local_node_id: ClusterNodeId,
}

/// Cluster membership tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMembership {
    /// All nodes in the cluster
    nodes: Vec<ClusterNode>,

    /// Current generation (increments on topology change)
    generation: u64,
}

/// Information about a cluster node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNode {
    /// Node identifier
    pub id: ClusterNodeId,

    /// Network address
    pub address: String,

    /// Capacity (number of partitions this node can host)
    pub capacity: usize,

    /// Current load (number of partitions hosted)
    pub load: usize,

    /// Node state
    pub state: NodeState,
}

/// Node state in the cluster
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeState {
    /// Node is joining
    Joining,

    /// Node is active
    Active,

    /// Node is leaving
    Leaving,

    /// Node is offline
    Offline,
}

impl ClusterMembership {
    /// Create new cluster membership
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            generation: 0,
        }
    }

    /// Add a node to the cluster
    pub fn add_node(&mut self, node: ClusterNode) {
        self.nodes.push(node);
        self.generation += 1;
    }

    /// Remove a node from the cluster
    pub fn remove_node(&mut self, node_id: ClusterNodeId) -> Option<ClusterNode> {
        let pos = self.nodes.iter().position(|n| n.id == node_id)?;
        let node = self.nodes.remove(pos);
        self.generation += 1;
        Some(node)
    }

    /// Get node by ID
    pub fn get_node(&self, node_id: ClusterNodeId) -> Option<&ClusterNode> {
        self.nodes.iter().find(|n| n.id == node_id)
    }

    /// Get all active nodes
    pub fn active_nodes(&self) -> Vec<&ClusterNode> {
        self.nodes
            .iter()
            .filter(|n| n.state == NodeState::Active)
            .collect()
    }

    /// Get cluster generation
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl Default for ClusterMembership {
    fn default() -> Self {
        Self::new()
    }
}

impl DistributedPartitionLocator {
    /// Create a new distributed partition locator
    pub fn new(node_count: u32, local_node_id: ClusterNodeId) -> Self {
        Self {
            ring: Arc::new(RwLock::new(ConsistentHashRing::new(node_count))),
            cluster: Arc::new(RwLock::new(ClusterMembership::new())),
            local_partitions: Arc::new(RwLock::new(HashMap::new())),
            local_node_id,
        }
    }

    /// Find which node owns a partition
    ///
    /// Uses consistent hashing on the partition key to determine ownership.
    pub async fn locate_partition(&self, key: &HmgiPartitionKey) -> Option<ClusterNodeId> {
        let cluster = self.cluster.read().await;
        let mut active_nodes: Vec<&ClusterNode> = cluster.active_nodes();
        active_nodes.sort_by_key(|node| node.id);

        let ring = self.ring.read().await;
        let ring_node = ring.get_node_for_key(key.oid, key.variation_id, &key.modality_tag)?;

        if active_nodes.is_empty() {
            return Some(ring_node);
        }

        let index = (ring_node as usize) % active_nodes.len();
        Some(active_nodes[index].id)
    }

    /// Check if a partition is local to this node
    pub async fn is_partition_local(&self, key: &HmgiPartitionKey) -> bool {
        {
            let local = self.local_partitions.read().await;
            if local.contains_key(&key.to_string()) {
                return true;
            }
        }

        match self.locate_partition(key).await {
            Some(node_id) => node_id == self.local_node_id,
            None => false,
        }
    }

    /// Register a partition as local
    pub async fn register_local_partition(&self, key: HmgiPartitionKey) -> Result<()> {
        let mut local = self.local_partitions.write().await;
        local.insert(key.to_string(), key);
        Ok(())
    }

    /// Unregister a local partition
    pub async fn unregister_local_partition(&self, key: &HmgiPartitionKey) -> Result<()> {
        let mut local = self.local_partitions.write().await;
        local.remove(&key.to_string());
        Ok(())
    }

    /// Get all local partitions
    pub async fn get_local_partitions(&self) -> Vec<HmgiPartitionKey> {
        let local = self.local_partitions.read().await;
        local.values().cloned().collect()
    }

    /// Add a node to the cluster
    pub async fn add_node(&self, node: ClusterNode) -> Result<()> {
        {
            let mut cluster = self.cluster.write().await;
            cluster.add_node(node);
        }
        self.rebuild_ring().await;
        Ok(())
    }

    /// Remove a node from the cluster
    pub async fn remove_node(&self, node_id: ClusterNodeId) -> Result<()> {
        {
            let mut cluster = self.cluster.write().await;
            cluster.remove_node(node_id);
        }
        self.rebuild_ring().await;
        Ok(())
    }

    /// Get cluster membership
    pub async fn get_cluster(&self) -> ClusterMembership {
        self.cluster.read().await.clone()
    }

    /// Rebuild the consistent hash ring after topology change
    async fn rebuild_ring(&self) {
        let cluster = self.cluster.read().await;
        let active_count = cluster.active_nodes().len() as u32;
        drop(cluster);

        if active_count > 0 {
            let mut ring = self.ring.write().await;
            *ring = ConsistentHashRing::new(active_count);
        }
    }

    /// Group partitions by owning node
    ///
    /// Returns a map from node_id to the partitions it owns.
    pub async fn group_partitions_by_node(
        &self,
        partitions: &[HmgiPartitionKey],
    ) -> HashMap<ClusterNodeId, Vec<HmgiPartitionKey>> {
        let mut grouped = HashMap::new();

        for partition in partitions {
            if let Some(node_id) = self.locate_partition(partition).await {
                grouped
                    .entry(node_id)
                    .or_insert_with(Vec::new)
                    .push(partition.clone());
            }
        }

        grouped
    }

    /// Split partitions into local and remote
    pub async fn split_local_remote(
        &self,
        partitions: Vec<HmgiPartitionKey>,
    ) -> (
        Vec<HmgiPartitionKey>,
        HashMap<ClusterNodeId, Vec<HmgiPartitionKey>>,
    ) {
        let mut local = Vec::new();
        let mut remote: HashMap<ClusterNodeId, Vec<HmgiPartitionKey>> = HashMap::new();

        for partition in partitions {
            if self.is_partition_local(&partition).await {
                local.push(partition);
            } else if let Some(node_id) = self.locate_partition(&partition).await {
                remote
                    .entry(node_id)
                    .or_insert_with(Vec::new)
                    .push(partition);
            }
        }

        (local, remote)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_partition_registration() {
        let locator = DistributedPartitionLocator::new(3, 1);

        let key = HmgiPartitionKey::new(123, 1, "text".to_string(), None);

        locator.register_local_partition(key.clone()).await.unwrap();

        let local_partitions = locator.get_local_partitions().await;
        assert_eq!(local_partitions.len(), 1);
        assert_eq!(local_partitions[0], key);
    }

    #[tokio::test]
    async fn test_unregister_local_partition() {
        let locator = DistributedPartitionLocator::new(3, 1);

        let key = HmgiPartitionKey::new(123, 1, "text".to_string(), None);

        locator.register_local_partition(key.clone()).await.unwrap();
        locator.unregister_local_partition(&key).await.unwrap();

        let local_partitions = locator.get_local_partitions().await;
        assert_eq!(local_partitions.len(), 0);
    }

    #[test]
    fn test_cluster_membership() {
        let mut membership = ClusterMembership::new();

        assert_eq!(membership.generation(), 0);

        let node = ClusterNode {
            id: 1,
            address: "localhost:8080".to_string(),
            capacity: 1000,
            load: 0,
            state: NodeState::Active,
        };

        membership.add_node(node);
        assert_eq!(membership.generation(), 1);
        assert_eq!(membership.active_nodes().len(), 1);
    }

    #[test]
    fn test_cluster_remove_node() {
        let mut membership = ClusterMembership::new();

        let node = ClusterNode {
            id: 1,
            address: "localhost:8080".to_string(),
            capacity: 1000,
            load: 0,
            state: NodeState::Active,
        };

        membership.add_node(node.clone());
        let removed = membership.remove_node(1);

        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, 1);
        assert_eq!(membership.active_nodes().len(), 0);
        assert_eq!(membership.generation(), 2); // Incremented twice
    }

    #[tokio::test]
    async fn test_add_cluster_node() {
        let locator = DistributedPartitionLocator::new(3, 1);

        let node = ClusterNode {
            id: 2,
            address: "localhost:8081".to_string(),
            capacity: 1000,
            load: 0,
            state: NodeState::Active,
        };

        locator.add_node(node).await.unwrap();

        let cluster = locator.get_cluster().await;
        assert_eq!(cluster.active_nodes().len(), 1);
    }

    #[tokio::test]
    async fn test_split_local_remote_partitions() {
        let locator = DistributedPartitionLocator::new(3, 1);

        // Register one local partition
        let local_key = HmgiPartitionKey::new(123, 1, "text".to_string(), None);
        locator
            .register_local_partition(local_key.clone())
            .await
            .unwrap();

        let partitions = vec![
            local_key,
            HmgiPartitionKey::new(123, 1, "image".to_string(), None),
            HmgiPartitionKey::new(123, 1, "video".to_string(), None),
        ];

        let (local, _remote) = locator.split_local_remote(partitions).await;

        // One is local (registered), others are remote (not local node)
        assert_eq!(local.len(), 1);
        // Remote grouping depends on hash-based location
    }
}
