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

//! HMGI Partition Registry
//!
//! Central registry mapping partition keys to their HNSW index instances.

#![allow(dead_code)] // TODO: Remove as implementation progresses

use super::ConsistentHashRing;
use crate::index::axis::indexes::hnsw_index::{AxisHnswConfig, AxisHnswIndex};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::HmgiPartitionKey;

/// HMGI partition registry - manages per-modality HNSW indexes
pub struct HmgiRegistry {
    /// Map from partition key to HNSW index
    partitions: Arc<RwLock<HashMap<HmgiPartitionKey, Arc<AxisHnswIndex>>>>,

    /// Reverse map from collection_id to partition keys
    collection_partitions: Arc<RwLock<HashMap<String, HashSet<HmgiPartitionKey>>>>,

    /// Consistent hash ring for distributed placement
    hash_ring: Arc<RwLock<ConsistentHashRing>>,
}

impl HmgiRegistry {
    /// Create a new HMGI registry
    pub fn new() -> Self {
        Self {
            partitions: Arc::new(RwLock::new(HashMap::new())),
            collection_partitions: Arc::new(RwLock::new(HashMap::new())),
            hash_ring: Arc::new(RwLock::new(ConsistentHashRing::new(3))),
        }
    }

    /// Create a new HMGI registry with custom shard count
    pub fn with_shards(shard_count: u32) -> Self {
        Self {
            partitions: Arc::new(RwLock::new(HashMap::new())),
            collection_partitions: Arc::new(RwLock::new(HashMap::new())),
            hash_ring: Arc::new(RwLock::new(ConsistentHashRing::new(shard_count))),
        }
    }

    /// Get or create a partition for the given key
    pub async fn get_or_create_partition(
        &self,
        key: HmgiPartitionKey,
        config: AxisHnswConfig,
        dimension: usize,
    ) -> Result<Arc<AxisHnswIndex>> {
        // Check if partition exists
        {
            let partitions = self.partitions.read().await;
            if let Some(index) = partitions.get(&key) {
                return Ok(index.clone());
            }
        }

        // Create new partition
        let index = Arc::new(AxisHnswIndex::new_with_collection(
            Some(key.to_string()),
            config,
            dimension,
        )?);

        let mut partitions = self.partitions.write().await;
        partitions.insert(key.clone(), index.clone());

        // Update reverse map
        // For now, use a simple collection identifier from the key
        let collection_id = format!("oid_{}_var_{}", key.oid, key.variation_id);
        let mut collection_partitions = self.collection_partitions.write().await;
        collection_partitions
            .entry(collection_id)
            .or_default()
            .insert(key);

        Ok(index)
    }

    /// Get an existing partition by key
    pub async fn get_partition(&self, key: &HmgiPartitionKey) -> Option<Arc<AxisHnswIndex>> {
        let partitions = self.partitions.read().await;
        partitions.get(key).cloned()
    }

    /// Get all partitions for a collection
    pub async fn get_partitions_for_collection(
        &self,
        collection_id: &str,
    ) -> Vec<HmgiPartitionKey> {
        let collection_partitions = self.collection_partitions.read().await;
        collection_partitions
            .get(collection_id)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Register an existing partition key under an explicit collection ID.
    pub async fn register_collection_partition(&self, collection_id: &str, key: HmgiPartitionKey) {
        let mut collection_partitions = self.collection_partitions.write().await;
        collection_partitions
            .entry(collection_id.to_string())
            .or_default()
            .insert(key);
    }

    /// Drop all partitions for a collection
    pub async fn drop_collection_partitions(&self, collection_id: &str) -> Result<usize> {
        let mut collection_partitions = self.collection_partitions.write().await;

        if let Some(keys) = collection_partitions.remove(collection_id) {
            let count = keys.len();
            let mut partitions = self.partitions.write().await;
            for key in keys {
                partitions.remove(&key);
            }
            Ok(count)
        } else {
            Ok(0)
        }
    }

    /// Get total number of partitions
    pub async fn len(&self) -> usize {
        let partitions = self.partitions.read().await;
        partitions.len()
    }

    /// Check if registry is empty
    pub async fn is_empty(&self) -> bool {
        let partitions = self.partitions.read().await;
        partitions.is_empty()
    }
}

impl Default for HmgiRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::distance_computation::DistanceMetric;

    #[tokio::test]
    async fn test_registry_create_partition() {
        let registry = HmgiRegistry::new();
        let key = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));
        let config = AxisHnswConfig {
            distance_metric: DistanceMetric::Cosine,
            ..Default::default()
        };

        let _index = registry
            .get_or_create_partition(key.clone(), config, 128)
            .await
            .unwrap();

        assert_eq!(registry.len().await, 1);
        assert!(registry.get_partition(&key).await.is_some());
    }

    #[tokio::test]
    async fn test_registry_get_existing_partition() {
        let registry = HmgiRegistry::new();
        let key = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));

        let index1 = registry
            .get_or_create_partition(key.clone(), AxisHnswConfig::default(), 128)
            .await
            .unwrap();
        let index2 = registry
            .get_or_create_partition(key.clone(), AxisHnswConfig::default(), 128)
            .await
            .unwrap();

        // Should return the same index (Arc::ptr_eq)
        assert!(Arc::ptr_eq(&index1, &index2));
        assert_eq!(registry.len().await, 1); // Not duplicated
    }

    #[tokio::test]
    async fn test_registry_collection_isolation() {
        let registry = HmgiRegistry::new();
        let key1 = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));
        let key2 = HmgiPartitionKey::new(124, 1, "text".to_string(), Some(456));

        registry
            .get_or_create_partition(key1, AxisHnswConfig::default(), 128)
            .await
            .unwrap();
        registry
            .get_or_create_partition(key2, AxisHnswConfig::default(), 128)
            .await
            .unwrap();

        // Different oid means different collection
        assert_eq!(registry.len().await, 2);
    }

    #[tokio::test]
    async fn test_registry_drop_collection() {
        let registry = HmgiRegistry::new();
        let key = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));

        registry
            .get_or_create_partition(key, AxisHnswConfig::default(), 128)
            .await
            .unwrap();

        let collection_id = format!("oid_{}_var_{}", 123, 1);
        let dropped = registry
            .drop_collection_partitions(&collection_id)
            .await
            .unwrap();
        assert_eq!(dropped, 1);
        assert_eq!(registry.len().await, 0);
    }
}
