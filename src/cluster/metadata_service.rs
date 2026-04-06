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

//! Cluster Metadata Service
//!
//! Provides distributed metadata management for ProximaDB clusters.
//! Handles collection metadata, shard assignments, and cluster-wide configuration.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Configuration for the metadata service
#[derive(Debug, Clone)]
pub struct MetadataServiceConfig {
    /// Maximum entries in metadata cache
    pub cache_size: usize,
    /// TTL for cached entries in seconds
    pub cache_ttl_secs: u64,
    /// Enable persistent storage of metadata
    pub persistent: bool,
    /// Storage path for persistent metadata
    pub storage_path: Option<String>,
}

impl Default for MetadataServiceConfig {
    fn default() -> Self {
        Self {
            cache_size: 10000,
            cache_ttl_secs: 300,
            persistent: true,
            storage_path: None,
        }
    }
}

/// Cluster-wide metadata
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClusterMetadata {
    /// Cluster version (incremented on each change)
    pub version: u64,
    /// Collection metadata map
    pub collections: HashMap<String, CollectionMetadata>,
    /// Shard placement information
    pub shard_placements: HashMap<String, ShardPlacement>,
    /// Cluster configuration
    pub config: ClusterConfiguration,
}

/// Metadata for a collection in the cluster
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionMetadata {
    /// Collection unique identifier
    pub collection_id: String,
    /// Collection name
    pub name: String,
    /// Vector dimension
    pub dimension: u32,
    /// Number of shards
    pub shard_count: u32,
    /// Replication factor
    pub replication_factor: u32,
    /// Storage engine type
    pub engine: String,
    /// Creation timestamp
    pub created_at: i64,
    /// Last modified timestamp
    pub updated_at: i64,
}

/// Shard placement information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardPlacement {
    /// Shard identifier
    pub shard_id: String,
    /// Collection this shard belongs to
    pub collection_id: String,
    /// Primary node for this shard
    pub primary_node: String,
    /// Replica nodes for this shard
    pub replica_nodes: Vec<String>,
    /// Shard state
    pub state: ShardState,
}

/// State of a shard
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ShardState {
    /// Shard is being created
    Creating,
    /// Shard is active and serving requests
    Active,
    /// Shard is being rebalanced
    Rebalancing,
    /// Shard is being migrated to another node
    Migrating,
    /// Shard is offline
    Offline,
}

/// Cluster-wide configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfiguration {
    /// Default replication factor for new collections
    pub default_replication_factor: u32,
    /// Default shard count for new collections
    pub default_shard_count: u32,
    /// Enable automatic rebalancing
    pub auto_rebalance: bool,
    /// Rebalance threshold (load difference percentage)
    pub rebalance_threshold: f32,
}

impl Default for ClusterConfiguration {
    fn default() -> Self {
        Self {
            default_replication_factor: 1,
            default_shard_count: 1,
            auto_rebalance: true,
            rebalance_threshold: 0.2,
        }
    }
}

/// Metadata service for cluster-wide metadata management
pub struct MetadataService {
    #[allow(dead_code)] // Config reserved for future use (TTL, replication settings)
    config: MetadataServiceConfig,
    metadata: Arc<RwLock<ClusterMetadata>>,
    version_counter: Arc<RwLock<u64>>,
}

impl MetadataService {
    /// Create a new metadata service
    pub fn new(config: MetadataServiceConfig) -> Result<Self> {
        Ok(Self {
            config,
            metadata: Arc::new(RwLock::new(ClusterMetadata::default())),
            version_counter: Arc::new(RwLock::new(0)),
        })
    }

    /// Get the current cluster metadata
    pub async fn get_metadata(&self) -> ClusterMetadata {
        self.metadata.read().await.clone()
    }

    /// Get metadata for a specific collection
    pub async fn get_collection(&self, collection_id: &str) -> Option<CollectionMetadata> {
        let metadata = self.metadata.read().await;
        metadata.collections.get(collection_id).cloned()
    }

    /// Register a new collection
    pub async fn register_collection(&self, collection: CollectionMetadata) -> Result<()> {
        let mut metadata = self.metadata.write().await;
        let mut version = self.version_counter.write().await;

        *version += 1;
        metadata.version = *version;
        metadata
            .collections
            .insert(collection.collection_id.clone(), collection);

        tracing::info!(
            version = metadata.version,
            "Collection registered in cluster metadata"
        );

        Ok(())
    }

    /// Update collection metadata
    pub async fn update_collection(&self, collection: CollectionMetadata) -> Result<()> {
        let mut metadata = self.metadata.write().await;
        let mut version = self.version_counter.write().await;

        if !metadata.collections.contains_key(&collection.collection_id) {
            return Err(anyhow::anyhow!(
                "Collection not found: {}",
                collection.collection_id
            ));
        }

        *version += 1;
        metadata.version = *version;
        metadata
            .collections
            .insert(collection.collection_id.clone(), collection);

        Ok(())
    }

    /// Remove a collection from metadata
    pub async fn remove_collection(&self, collection_id: &str) -> Result<()> {
        let mut metadata = self.metadata.write().await;
        let mut version = self.version_counter.write().await;

        if metadata.collections.remove(collection_id).is_none() {
            return Err(anyhow::anyhow!("Collection not found: {}", collection_id));
        }

        // Also remove associated shard placements
        metadata
            .shard_placements
            .retain(|_, v| v.collection_id != collection_id);

        *version += 1;
        metadata.version = *version;

        Ok(())
    }

    /// Get shard placement for a collection
    pub async fn get_shard_placements(&self, collection_id: &str) -> Vec<ShardPlacement> {
        let metadata = self.metadata.read().await;
        metadata
            .shard_placements
            .values()
            .filter(|p| p.collection_id == collection_id)
            .cloned()
            .collect()
    }

    /// Update shard placement
    pub async fn update_shard_placement(&self, placement: ShardPlacement) -> Result<()> {
        let mut metadata = self.metadata.write().await;
        let mut version = self.version_counter.write().await;

        *version += 1;
        metadata.version = *version;
        metadata
            .shard_placements
            .insert(placement.shard_id.clone(), placement);

        Ok(())
    }

    /// Get the current metadata version
    pub async fn version(&self) -> u64 {
        *self.version_counter.read().await
    }

    /// List all collections
    pub async fn list_collections(&self) -> Vec<CollectionMetadata> {
        let metadata = self.metadata.read().await;
        metadata.collections.values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_metadata_service_creation() {
        let config = MetadataServiceConfig::default();
        let service = MetadataService::new(config);
        assert!(service.is_ok());
    }

    #[tokio::test]
    async fn test_collection_registration() {
        let service = MetadataService::new(MetadataServiceConfig::default()).unwrap();

        let collection = CollectionMetadata {
            collection_id: "test-collection".to_string(),
            name: "Test Collection".to_string(),
            dimension: 128,
            shard_count: 3,
            replication_factor: 2,
            engine: "SST".to_string(),
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
        };

        service
            .register_collection(collection.clone())
            .await
            .unwrap();

        let retrieved = service.get_collection("test-collection").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "Test Collection");
    }

    #[tokio::test]
    async fn test_version_increment() {
        let service = MetadataService::new(MetadataServiceConfig::default()).unwrap();

        assert_eq!(service.version().await, 0);

        let collection = CollectionMetadata {
            collection_id: "test".to_string(),
            name: "Test".to_string(),
            dimension: 64,
            shard_count: 1,
            replication_factor: 1,
            engine: "SST".to_string(),
            created_at: 0,
            updated_at: 0,
        };

        service.register_collection(collection).await.unwrap();
        assert_eq!(service.version().await, 1);
    }
}
