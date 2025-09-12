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

//! # PULSAR Sharding Module
//!
//! Implements consistent hashing for distributed node and edge placement across shards.
//! Uses SHA-256 for hash function to ensure uniform distribution.

use crate::core::error::ProximaDBError;
type Result<T> = std::result::Result<T, ProximaDBError>;
use crate::graph::{EdgeId, NodeId};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Number of virtual nodes per physical shard for better distribution
const VIRTUAL_NODES_PER_SHARD: u32 = 100;

/// Consistent hash ring for shard distribution
#[derive(Debug)]
pub struct ConsistentHashRing {
    /// Hash ring with virtual nodes mapping to physical shards
    ring: BTreeMap<u64, u32>,
    /// Number of physical shards
    shard_count: u32,
}

impl ConsistentHashRing {
    /// Create a new consistent hash ring
    pub fn new(shard_count: u32) -> Self {
        let mut ring = BTreeMap::new();

        // Create virtual nodes for each physical shard
        for shard_id in 0..shard_count {
            for virtual_id in 0..VIRTUAL_NODES_PER_SHARD {
                let key = format!("shard_{}_{}", shard_id, virtual_id);
                let hash = Self::hash_key(&key);
                ring.insert(hash, shard_id);
            }
        }

        Self { ring, shard_count }
    }

    /// Get shard ID for a given node
    pub fn get_shard(&self, node_id: &NodeId) -> u32 {
        let hash = Self::hash_key(node_id);

        // Find the first shard with hash >= node hash
        if let Some((&_, &shard_id)) = self.ring.range(hash..).next() {
            shard_id
        } else {
            // Wrap around to the beginning
            self.ring
                .iter()
                .next()
                .map(|(_, &shard_id)| shard_id)
                .unwrap_or(0)
        }
    }

    /// Get shard ID for an edge based on source node
    pub fn get_shard_for_edge(&self, from_node_id: &NodeId, _to_node_id: &NodeId) -> u32 {
        // For simplicity, use source node's shard for the edge
        // More sophisticated strategies could consider both nodes
        self.get_shard(from_node_id)
    }

    /// Hash function using SHA-256
    fn hash_key(key: &str) -> u64 {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        let result = hasher.finalize();

        // Take first 8 bytes of hash as u64
        u64::from_be_bytes([
            result[0], result[1], result[2], result[3], result[4], result[5], result[6], result[7],
        ])
    }

    /// Add a new shard to the ring (for dynamic scaling)
    pub fn add_shard(&mut self, shard_id: u32) -> Result<()> {
        if shard_id >= self.shard_count {
            self.shard_count = shard_id + 1;
        }

        // Add virtual nodes for the new shard
        for virtual_id in 0..VIRTUAL_NODES_PER_SHARD {
            let key = format!("shard_{}_{}", shard_id, virtual_id);
            let hash = Self::hash_key(&key);
            self.ring.insert(hash, shard_id);
        }

        Ok(())
    }

    /// Remove a shard from the ring (for scaling down)
    pub fn remove_shard(&mut self, shard_id: u32) -> Result<Vec<(u64, NodeId)>> {
        let mut moved_keys = Vec::new();

        // Remove all virtual nodes for this shard
        let keys_to_remove: Vec<_> = self
            .ring
            .iter()
            .filter_map(|(&hash, &sid)| if sid == shard_id { Some(hash) } else { None })
            .collect();

        for key in keys_to_remove {
            self.ring.remove(&key);
            // In a real implementation, we would track which actual keys need to be moved
            moved_keys.push((key, format!("virtual_node_{}", key)));
        }

        Ok(moved_keys)
    }

    /// Get all shards in the ring
    pub fn get_all_shards(&self) -> Vec<u32> {
        self.ring
            .values()
            .cloned()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect()
    }

    /// Get shard distribution statistics
    pub fn get_distribution_stats(&self) -> ShardDistributionStats {
        let mut shard_virtual_count = std::collections::HashMap::new();

        for &shard_id in self.ring.values() {
            *shard_virtual_count.entry(shard_id).or_insert(0) += 1;
        }

        let total_virtual_nodes = self.ring.len() as u32;
        let expected_per_shard = total_virtual_nodes / self.shard_count;

        let mut min_virtual_nodes = u32::MAX;
        let mut max_virtual_nodes = 0;

        for &count in shard_virtual_count.values() {
            min_virtual_nodes = min_virtual_nodes.min(count);
            max_virtual_nodes = max_virtual_nodes.max(count);
        }

        ShardDistributionStats {
            total_shards: self.shard_count,
            total_virtual_nodes,
            expected_virtual_nodes_per_shard: expected_per_shard,
            min_virtual_nodes_per_shard: min_virtual_nodes,
            max_virtual_nodes_per_shard: max_virtual_nodes,
            distribution_variance: max_virtual_nodes.saturating_sub(min_virtual_nodes),
        }
    }
}

/// Statistics about shard distribution in the hash ring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardDistributionStats {
    pub total_shards: u32,
    pub total_virtual_nodes: u32,
    pub expected_virtual_nodes_per_shard: u32,
    pub min_virtual_nodes_per_shard: u32,
    pub max_virtual_nodes_per_shard: u32,
    pub distribution_variance: u32,
}

/// Shard load balancer for detecting and handling hot shards
#[derive(Debug)]
pub struct ShardLoadBalancer {
    /// Load statistics per shard
    load_stats: std::collections::HashMap<u32, ShardLoadStats>,
    /// Threshold for considering a shard "hot"
    hot_threshold: f64,
}

/// Load statistics for a single shard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardLoadStats {
    pub requests_per_second: f64,
    pub average_response_time_ms: f64,
    pub memory_usage_mb: u64,
    pub cpu_utilization: f64,
}

impl ShardLoadBalancer {
    /// Create a new load balancer
    pub fn new(hot_threshold: f64) -> Self {
        Self {
            load_stats: std::collections::HashMap::new(),
            hot_threshold,
        }
    }

    /// Update load statistics for a shard
    pub fn update_shard_load(&mut self, shard_id: u32, stats: ShardLoadStats) {
        self.load_stats.insert(shard_id, stats);
    }

    /// Get hot shards that exceed the threshold
    pub fn get_hot_shards(&self) -> Vec<u32> {
        self.load_stats
            .iter()
            .filter_map(|(&shard_id, stats)| {
                if stats.requests_per_second > self.hot_threshold
                    || stats.cpu_utilization > 80.0
                    || stats.average_response_time_ms > 100.0
                {
                    Some(shard_id)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get load statistics for all shards
    pub fn get_all_load_stats(&self) -> &std::collections::HashMap<u32, ShardLoadStats> {
        &self.load_stats
    }

    /// Calculate optimal shard count based on current load
    pub fn calculate_optimal_shard_count(&self, current_shards: u32) -> u32 {
        let hot_shards = self.get_hot_shards();
        let hot_ratio = hot_shards.len() as f64 / current_shards as f64;

        if hot_ratio > 0.3 {
            // More than 30% of shards are hot, consider scaling up
            (current_shards as f64 * 1.5) as u32
        } else if hot_ratio < 0.1 && current_shards > 4 {
            // Less than 10% hot and we have room to scale down
            (current_shards as f64 * 0.8) as u32
        } else {
            current_shards
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consistent_hash_ring_creation() {
        let ring = ConsistentHashRing::new(4);
        assert_eq!(ring.shard_count, 4);
        assert_eq!(ring.ring.len(), 4 * VIRTUAL_NODES_PER_SHARD as usize);
    }

    #[test]
    fn test_node_distribution() {
        let ring = ConsistentHashRing::new(4);

        // Test that different nodes map to different shards (most of the time)
        let node1_shard = ring.get_shard("node1");
        let node2_shard = ring.get_shard("node2");
        let node3_shard = ring.get_shard("node3");
        let node4_shard = ring.get_shard("node4");

        // All shards should be within expected range
        assert!(node1_shard < 4);
        assert!(node2_shard < 4);
        assert!(node3_shard < 4);
        assert!(node4_shard < 4);

        // At least some nodes should be on different shards
        let shards = vec![node1_shard, node2_shard, node3_shard, node4_shard];
        let unique_shards: std::collections::HashSet<_> = shards.into_iter().collect();
        assert!(
            unique_shards.len() > 1,
            "Nodes should distribute across multiple shards"
        );
    }

    #[test]
    fn test_consistent_mapping() {
        let ring = ConsistentHashRing::new(8);

        let node_id = "test_node_123";
        let shard1 = ring.get_shard(node_id);
        let shard2 = ring.get_shard(node_id);
        let shard3 = ring.get_shard(node_id);

        // Same node should always map to same shard
        assert_eq!(shard1, shard2);
        assert_eq!(shard2, shard3);
    }

    #[test]
    fn test_hash_function() {
        // Test that hash function produces consistent results
        let hash1 = ConsistentHashRing::hash_key("test_key");
        let hash2 = ConsistentHashRing::hash_key("test_key");
        assert_eq!(hash1, hash2);

        // Test that different keys produce different hashes
        let hash3 = ConsistentHashRing::hash_key("different_key");
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_shard_addition() {
        let mut ring = ConsistentHashRing::new(2);
        let original_shards = ring.get_all_shards();
        assert_eq!(original_shards.len(), 2);

        // Add a new shard
        ring.add_shard(2).unwrap();
        let new_shards = ring.get_all_shards();
        assert!(new_shards.len() >= 3);
        assert!(new_shards.contains(&2));
    }

    #[test]
    fn test_distribution_stats() {
        let ring = ConsistentHashRing::new(4);
        let stats = ring.get_distribution_stats();

        assert_eq!(stats.total_shards, 4);
        assert_eq!(stats.total_virtual_nodes, 4 * VIRTUAL_NODES_PER_SHARD);
        assert_eq!(
            stats.expected_virtual_nodes_per_shard,
            VIRTUAL_NODES_PER_SHARD
        );

        // With proper consistent hashing, distribution should be fairly even
        assert!(stats.distribution_variance <= VIRTUAL_NODES_PER_SHARD / 10);
    }

    #[test]
    fn test_load_balancer() {
        let mut balancer = ShardLoadBalancer::new(1000.0);

        // Add normal load shard
        balancer.update_shard_load(
            0,
            ShardLoadStats {
                requests_per_second: 500.0,
                average_response_time_ms: 50.0,
                memory_usage_mb: 1024,
                cpu_utilization: 60.0,
            },
        );

        // Add hot shard
        balancer.update_shard_load(
            1,
            ShardLoadStats {
                requests_per_second: 1500.0,     // Above threshold
                average_response_time_ms: 120.0, // High response time
                memory_usage_mb: 2048,
                cpu_utilization: 85.0, // High CPU
            },
        );

        let hot_shards = balancer.get_hot_shards();
        assert_eq!(hot_shards.len(), 1);
        assert_eq!(hot_shards[0], 1);

        // Test optimal shard count calculation
        let optimal = balancer.calculate_optimal_shard_count(4);
        assert!(optimal > 4); // Should scale up due to hot shard
    }
}
