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

//! Shard Management Module
//!
//! Provides shard management, placement, and rebalancing for distributed collections.
//! Handles shard lifecycle, placement decisions, and replication management.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Configuration for shard management
#[derive(Debug, Clone)]
pub struct ShardConfig {
    /// Default number of shards per collection
    pub default_shard_count: u32,
    /// Default replication factor
    pub default_replication_factor: u32,
    /// Minimum shards per collection
    pub min_shards: u32,
    /// Maximum shards per collection
    pub max_shards: u32,
    /// Enable automatic shard rebalancing
    pub auto_rebalance: bool,
    /// Rebalance threshold (load imbalance percentage)
    pub rebalance_threshold: f32,
    /// Maximum concurrent rebalance operations
    pub max_concurrent_rebalance: u32,
}

impl Default for ShardConfig {
    fn default() -> Self {
        Self {
            default_shard_count: 3,
            default_replication_factor: 2,
            min_shards: 1,
            max_shards: 256,
            auto_rebalance: true,
            rebalance_threshold: 0.2,
            max_concurrent_rebalance: 2,
        }
    }
}

/// Unique identifier for a shard
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ShardId(String);

impl ShardId {
    /// Create a new shard ID
    pub fn new(id: String) -> Self {
        Self(id)
    }

    /// Generate a shard ID for a collection and shard number
    pub fn generate(collection_id: &str, shard_number: u32) -> Self {
        Self(format!("{}_{:04}", collection_id, shard_number))
    }

    /// Get the shard ID as a string
    pub fn id(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ShardId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// State of a shard
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ShardState {
    /// Shard is being initialized
    Initializing,
    /// Shard is active and serving requests
    Active,
    /// Shard is being rebalanced to another node
    Rebalancing,
    /// Shard is being recovered from failure
    Recovering,
    /// Shard is being deleted
    Deleting,
    /// Shard is offline
    Offline,
}

/// Shard placement information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardPlacement {
    /// Node hosting the shard
    pub node_id: String,
    /// Whether this is the primary replica
    pub is_primary: bool,
    /// Replica priority (lower = higher priority for promotion)
    pub priority: u32,
    /// Data synchronization lag in milliseconds (for replicas)
    pub lag_ms: Option<u64>,
}

/// A shard in the distributed system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shard {
    /// Shard identifier
    pub id: ShardId,
    /// Collection this shard belongs to
    pub collection_id: String,
    /// Shard number within the collection
    pub shard_number: u32,
    /// Current state
    pub state: ShardState,
    /// Placement of all replicas
    pub placements: Vec<ShardPlacement>,
    /// Key range start (for range-based sharding)
    pub key_range_start: Option<String>,
    /// Key range end (for range-based sharding)
    pub key_range_end: Option<String>,
    /// Vector count in this shard
    pub vector_count: u64,
    /// Size in bytes
    pub size_bytes: u64,
    /// Creation timestamp
    pub created_at: i64,
    /// Last modified timestamp
    pub updated_at: i64,
}

impl Shard {
    /// Create a new shard
    pub fn new(collection_id: &str, shard_number: u32) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: ShardId::generate(collection_id, shard_number),
            collection_id: collection_id.to_string(),
            shard_number,
            state: ShardState::Initializing,
            placements: Vec::new(),
            key_range_start: None,
            key_range_end: None,
            vector_count: 0,
            size_bytes: 0,
            created_at: now,
            updated_at: now,
        }
    }

    /// Get the primary node for this shard
    pub fn primary_node(&self) -> Option<&str> {
        self.placements
            .iter()
            .find(|p| p.is_primary)
            .map(|p| p.node_id.as_str())
    }

    /// Get all replica nodes for this shard
    pub fn replica_nodes(&self) -> Vec<&str> {
        self.placements
            .iter()
            .filter(|p| !p.is_primary)
            .map(|p| p.node_id.as_str())
            .collect()
    }

    /// Add a placement for this shard
    pub fn add_placement(&mut self, placement: ShardPlacement) {
        self.placements.push(placement);
        self.updated_at = chrono::Utc::now().timestamp();
    }

    /// Remove a placement by node ID
    pub fn remove_placement(&mut self, node_id: &str) -> Option<ShardPlacement> {
        if let Some(idx) = self.placements.iter().position(|p| p.node_id == node_id) {
            self.updated_at = chrono::Utc::now().timestamp();
            Some(self.placements.remove(idx))
        } else {
            None
        }
    }

    /// Promote a replica to primary
    pub fn promote_replica(&mut self, node_id: &str) -> Result<()> {
        // Demote current primary
        for placement in &mut self.placements {
            if placement.is_primary {
                placement.is_primary = false;
            }
        }

        // Promote the specified replica
        if let Some(placement) = self.placements.iter_mut().find(|p| p.node_id == node_id) {
            placement.is_primary = true;
            self.updated_at = chrono::Utc::now().timestamp();
            Ok(())
        } else {
            Err(anyhow::anyhow!("Node not found in shard placements"))
        }
    }
}

/// Shard manager for distributed shard management
pub struct ShardManager {
    config: ShardConfig,
    /// All shards by ID
    shards: Arc<RwLock<HashMap<ShardId, Shard>>>,
    /// Shards by collection
    collection_shards: Arc<RwLock<HashMap<String, Vec<ShardId>>>>,
    /// Shards by node
    node_shards: Arc<RwLock<HashMap<String, Vec<ShardId>>>>,
}

impl ShardManager {
    /// Create a new shard manager
    pub fn new(config: ShardConfig) -> Result<Self> {
        Ok(Self {
            config,
            shards: Arc::new(RwLock::new(HashMap::new())),
            collection_shards: Arc::new(RwLock::new(HashMap::new())),
            node_shards: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Create shards for a new collection
    pub async fn create_shards_for_collection(
        &self,
        collection_id: &str,
        shard_count: Option<u32>,
        replication_factor: Option<u32>,
        available_nodes: &[String],
    ) -> Result<Vec<Shard>> {
        let shard_count = shard_count.unwrap_or(self.config.default_shard_count);
        let replication_factor = replication_factor.unwrap_or(self.config.default_replication_factor);

        if shard_count < self.config.min_shards || shard_count > self.config.max_shards {
            return Err(anyhow::anyhow!(
                "Shard count must be between {} and {}",
                self.config.min_shards,
                self.config.max_shards
            ));
        }

        if available_nodes.len() < replication_factor as usize {
            return Err(anyhow::anyhow!(
                "Not enough nodes ({}) for replication factor {}",
                available_nodes.len(),
                replication_factor
            ));
        }

        let mut created_shards = Vec::new();

        for shard_num in 0..shard_count {
            let mut shard = Shard::new(collection_id, shard_num);

            // Assign placements using round-robin with offset for distribution
            for rep in 0..replication_factor {
                let node_idx = ((shard_num + rep) as usize) % available_nodes.len();
                let placement = ShardPlacement {
                    node_id: available_nodes[node_idx].clone(),
                    is_primary: rep == 0,
                    priority: rep,
                    lag_ms: None,
                };
                shard.add_placement(placement);
            }

            shard.state = ShardState::Active;
            created_shards.push(shard);
        }

        // Store shards
        {
            let mut shards = self.shards.write().await;
            let mut collection_shards = self.collection_shards.write().await;
            let mut node_shards = self.node_shards.write().await;

            let shard_ids: Vec<ShardId> = created_shards.iter().map(|s| s.id.clone()).collect();
            collection_shards.insert(collection_id.to_string(), shard_ids);

            for shard in &created_shards {
                shards.insert(shard.id.clone(), shard.clone());

                // Update node -> shard mapping
                for placement in &shard.placements {
                    node_shards
                        .entry(placement.node_id.clone())
                        .or_default()
                        .push(shard.id.clone());
                }
            }
        }

        tracing::info!(
            collection_id = %collection_id,
            shard_count = shard_count,
            replication_factor = replication_factor,
            "Created shards for collection"
        );

        Ok(created_shards)
    }

    /// Get a shard by ID
    pub async fn get_shard(&self, shard_id: &ShardId) -> Option<Shard> {
        let shards = self.shards.read().await;
        shards.get(shard_id).cloned()
    }

    /// Get all shards for a collection
    pub async fn get_collection_shards(&self, collection_id: &str) -> Vec<Shard> {
        let shards = self.shards.read().await;
        let collection_shards = self.collection_shards.read().await;

        collection_shards
            .get(collection_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| shards.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all shards on a node
    pub async fn get_node_shards(&self, node_id: &str) -> Vec<Shard> {
        let shards = self.shards.read().await;
        let node_shards = self.node_shards.read().await;

        node_shards
            .get(node_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| shards.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Update shard state
    pub async fn update_shard_state(&self, shard_id: &ShardId, state: ShardState) -> Result<()> {
        let mut shards = self.shards.write().await;

        if let Some(shard) = shards.get_mut(shard_id) {
            shard.state = state;
            shard.updated_at = chrono::Utc::now().timestamp();
            Ok(())
        } else {
            Err(anyhow::anyhow!("Shard not found: {}", shard_id))
        }
    }

    /// Update shard statistics
    pub async fn update_shard_stats(
        &self,
        shard_id: &ShardId,
        vector_count: u64,
        size_bytes: u64,
    ) -> Result<()> {
        let mut shards = self.shards.write().await;

        if let Some(shard) = shards.get_mut(shard_id) {
            shard.vector_count = vector_count;
            shard.size_bytes = size_bytes;
            shard.updated_at = chrono::Utc::now().timestamp();
            Ok(())
        } else {
            Err(anyhow::anyhow!("Shard not found: {}", shard_id))
        }
    }

    /// Delete all shards for a collection
    pub async fn delete_collection_shards(&self, collection_id: &str) -> Result<()> {
        let mut shards = self.shards.write().await;
        let mut collection_shards = self.collection_shards.write().await;
        let mut node_shards = self.node_shards.write().await;

        if let Some(shard_ids) = collection_shards.remove(collection_id) {
            for shard_id in &shard_ids {
                if let Some(shard) = shards.remove(shard_id) {
                    // Remove from node mappings
                    for placement in &shard.placements {
                        if let Some(node_shard_list) = node_shards.get_mut(&placement.node_id) {
                            node_shard_list.retain(|id| id != shard_id);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Get total shard count
    pub async fn shard_count(&self) -> usize {
        self.shards.read().await.len()
    }

    /// Get shard distribution statistics
    pub async fn get_distribution_stats(&self) -> ShardDistributionStats {
        let node_shards = self.node_shards.read().await;
        let shards = self.shards.read().await;

        let shard_counts: Vec<usize> = node_shards.values().map(|v| v.len()).collect();
        let total_shards = shards.len();
        let node_count = node_shards.len();

        let avg_shards_per_node = if node_count > 0 {
            total_shards as f64 / node_count as f64
        } else {
            0.0
        };

        let max_shards = shard_counts.iter().max().copied().unwrap_or(0);
        let min_shards = shard_counts.iter().min().copied().unwrap_or(0);

        let imbalance = if avg_shards_per_node > 0.0 && node_count > 1 {
            let variance: f64 = shard_counts
                .iter()
                .map(|&c| (c as f64 - avg_shards_per_node).powi(2))
                .sum::<f64>()
                / node_count as f64;
            variance.sqrt() / avg_shards_per_node
        } else {
            0.0
        };

        ShardDistributionStats {
            total_shards,
            node_count,
            avg_shards_per_node,
            max_shards_per_node: max_shards,
            min_shards_per_node: min_shards,
            imbalance_ratio: imbalance,
        }
    }
}

/// Statistics about shard distribution
#[derive(Debug, Clone)]
pub struct ShardDistributionStats {
    pub total_shards: usize,
    pub node_count: usize,
    pub avg_shards_per_node: f64,
    pub max_shards_per_node: usize,
    pub min_shards_per_node: usize,
    pub imbalance_ratio: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shard_id_generation() {
        let id = ShardId::generate("test-collection", 5);
        assert_eq!(id.id(), "test-collection_0005");
    }

    #[tokio::test]
    async fn test_shard_manager_creation() {
        let config = ShardConfig::default();
        let manager = ShardManager::new(config);
        assert!(manager.is_ok());
    }

    #[tokio::test]
    async fn test_create_shards() {
        let manager = ShardManager::new(ShardConfig::default()).unwrap();

        let nodes = vec![
            "node-1".to_string(),
            "node-2".to_string(),
            "node-3".to_string(),
        ];

        let shards = manager
            .create_shards_for_collection("test-collection", Some(3), Some(2), &nodes)
            .await
            .unwrap();

        assert_eq!(shards.len(), 3);

        // Each shard should have 2 placements (replication factor 2)
        for shard in &shards {
            assert_eq!(shard.placements.len(), 2);
            assert!(shard.primary_node().is_some());
        }

        // Verify shards are stored
        let retrieved = manager.get_collection_shards("test-collection").await;
        assert_eq!(retrieved.len(), 3);
    }

    #[tokio::test]
    async fn test_shard_state_update() {
        let manager = ShardManager::new(ShardConfig::default()).unwrap();

        let nodes = vec!["node-1".to_string(), "node-2".to_string()];
        let shards = manager
            .create_shards_for_collection("test", Some(1), Some(1), &nodes)
            .await
            .unwrap();

        let shard_id = shards[0].id.clone();

        manager
            .update_shard_state(&shard_id, ShardState::Rebalancing)
            .await
            .unwrap();

        let shard = manager.get_shard(&shard_id).await.unwrap();
        assert_eq!(shard.state, ShardState::Rebalancing);
    }

    #[test]
    fn test_shard_primary_promotion() {
        let mut shard = Shard::new("test", 0);

        shard.add_placement(ShardPlacement {
            node_id: "node-1".to_string(),
            is_primary: true,
            priority: 0,
            lag_ms: None,
        });
        shard.add_placement(ShardPlacement {
            node_id: "node-2".to_string(),
            is_primary: false,
            priority: 1,
            lag_ms: Some(10),
        });

        assert_eq!(shard.primary_node(), Some("node-1"));

        shard.promote_replica("node-2").unwrap();

        assert_eq!(shard.primary_node(), Some("node-2"));
    }
}
