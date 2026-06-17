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

// Config consolidated into proximadb-config (TD-107, seam S4); re-exported
// so existing `crate::cluster::...` import paths keep resolving.
pub use proximadb_config::cluster_config::{PartitionConfig, PartitionStrategy, ShardConfig};

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

/// Metadata bounds for a shard - enables metadata-aware routing and shard pruning
///
/// This structure tracks which tenant/domain data exists within a shard,
/// allowing the routing layer to skip shards that don't contain relevant data
/// for a given query's metadata filters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetadataBounds {
    /// Set of tenant IDs with data in this shard
    pub tenant_ids: std::collections::HashSet<String>,
    /// Set of domain IDs with data in this shard
    pub domain_ids: std::collections::HashSet<String>,
    /// Field value ranges for indexed metadata columns
    /// Key: field name, Value: (min_value, max_value) as JSON
    pub field_ranges: HashMap<String, (serde_json::Value, serde_json::Value)>,
    /// Bloom filter bytes for quick membership testing on high-cardinality fields
    /// Key: field name, Value: bloom filter bytes
    pub bloom_filters: HashMap<String, Vec<u8>>,
    /// Partition key values present in this shard (for hash/range partitioning)
    pub partition_values: std::collections::HashSet<String>,
    /// Last time bounds were updated
    pub last_updated: i64,
    /// Approximate record count per tenant
    pub tenant_record_counts: HashMap<String, u64>,
}

impl MetadataBounds {
    /// Create new empty metadata bounds
    pub fn new() -> Self {
        Self {
            last_updated: chrono::Utc::now().timestamp(),
            ..Default::default()
        }
    }

    /// Check if this shard might contain data for the given tenant
    pub fn may_contain_tenant(&self, tenant_id: &str) -> bool {
        // If we have no tenant tracking, assume it might contain any tenant
        self.tenant_ids.is_empty() || self.tenant_ids.contains(tenant_id)
    }

    /// Check if this shard might contain data for the given domain
    pub fn may_contain_domain(&self, domain_id: &str) -> bool {
        self.domain_ids.is_empty() || self.domain_ids.contains(domain_id)
    }

    /// Check if this shard might contain data matching the given partition key
    pub fn may_contain_partition(&self, partition_key: &str) -> bool {
        self.partition_values.is_empty() || self.partition_values.contains(partition_key)
    }

    /// Check if a field value might be within this shard's bounds
    pub fn may_contain_field_value(&self, field: &str, value: &serde_json::Value) -> bool {
        if let Some((min, max)) = self.field_ranges.get(field) {
            // Perform range check based on value type
            match (min, max, value) {
                (
                    serde_json::Value::Number(min_n),
                    serde_json::Value::Number(max_n),
                    serde_json::Value::Number(v),
                ) => {
                    if let (Some(min_f), Some(max_f), Some(v_f)) =
                        (min_n.as_f64(), max_n.as_f64(), v.as_f64())
                    {
                        return v_f >= min_f && v_f <= max_f;
                    }
                }
                (
                    serde_json::Value::String(min_s),
                    serde_json::Value::String(max_s),
                    serde_json::Value::String(v),
                ) => {
                    return v >= min_s && v <= max_s;
                }
                _ => {}
            }
        }
        // If no bounds tracked or incompatible types, assume it might contain the value
        true
    }

    /// Update bounds with a new record's metadata
    pub fn update_with_record(
        &mut self,
        metadata: &HashMap<String, serde_json::Value>,
        partition_key: Option<&str>,
    ) {
        // Extract tenant_id if present
        if let Some(serde_json::Value::String(tid)) = metadata.get("tenant_id") {
            self.tenant_ids.insert(tid.clone());
            *self.tenant_record_counts.entry(tid.clone()).or_insert(0) += 1;
        }

        // Extract domain_id if present
        if let Some(serde_json::Value::String(did)) = metadata.get("domain_id") {
            self.domain_ids.insert(did.clone());
        }

        // Track partition key
        if let Some(pk) = partition_key {
            self.partition_values.insert(pk.to_string());
        }

        // Update field ranges for numeric/string fields
        for (field, value) in metadata {
            // Skip special fields
            if field == "tenant_id" || field == "domain_id" {
                continue;
            }

            match value {
                serde_json::Value::Number(_) | serde_json::Value::String(_) => {
                    self.field_ranges
                        .entry(field.clone())
                        .and_modify(|(min, max)| {
                            // Update min/max bounds
                            if compare_json_values(value, min) == std::cmp::Ordering::Less {
                                *min = value.clone();
                            }
                            if compare_json_values(value, max) == std::cmp::Ordering::Greater {
                                *max = value.clone();
                            }
                        })
                        .or_insert((value.clone(), value.clone()));
                }
                _ => {}
            }
        }

        self.last_updated = chrono::Utc::now().timestamp();
    }

    /// Merge bounds from another MetadataBounds (e.g., during compaction)
    pub fn merge(&mut self, other: &MetadataBounds) {
        self.tenant_ids.extend(other.tenant_ids.iter().cloned());
        self.domain_ids.extend(other.domain_ids.iter().cloned());
        self.partition_values
            .extend(other.partition_values.iter().cloned());

        // Merge field ranges
        for (field, (other_min, other_max)) in &other.field_ranges {
            self.field_ranges
                .entry(field.clone())
                .and_modify(|(min, max)| {
                    if compare_json_values(other_min, min) == std::cmp::Ordering::Less {
                        *min = other_min.clone();
                    }
                    if compare_json_values(other_max, max) == std::cmp::Ordering::Greater {
                        *max = other_max.clone();
                    }
                })
                .or_insert((other_min.clone(), other_max.clone()));
        }

        // Merge tenant record counts
        for (tenant, count) in &other.tenant_record_counts {
            *self.tenant_record_counts.entry(tenant.clone()).or_insert(0) += count;
        }

        self.last_updated = chrono::Utc::now().timestamp();
    }
}

/// Compare two JSON values for ordering (standalone function to avoid borrow issues)
fn compare_json_values(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
    match (a, b) {
        (serde_json::Value::Number(a_n), serde_json::Value::Number(b_n)) => {
            let a_f = a_n.as_f64().unwrap_or(0.0);
            let b_f = b_n.as_f64().unwrap_or(0.0);
            a_f.partial_cmp(&b_f).unwrap_or(std::cmp::Ordering::Equal)
        }
        (serde_json::Value::String(a_s), serde_json::Value::String(b_s)) => a_s.cmp(b_s),
        _ => std::cmp::Ordering::Equal,
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
    /// Metadata bounds for shard pruning (tenant/domain/field ranges)
    pub metadata_bounds: Option<MetadataBounds>,
    /// Partition configuration for this shard
    pub partition_config: Option<PartitionConfig>,
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
            metadata_bounds: None,
            partition_config: None,
            vector_count: 0,
            size_bytes: 0,
            created_at: now,
            updated_at: now,
        }
    }

    /// Create a new shard with partition configuration
    pub fn with_partition_config(
        collection_id: &str,
        shard_number: u32,
        config: PartitionConfig,
    ) -> Self {
        let mut shard = Self::new(collection_id, shard_number);
        shard.partition_config = Some(config);
        if shard
            .partition_config
            .as_ref()
            .is_some_and(|c| c.track_metadata_bounds)
        {
            shard.metadata_bounds = Some(MetadataBounds::new());
        }
        shard
    }

    /// Update metadata bounds with a record's metadata
    pub fn update_metadata_bounds(&mut self, metadata: &HashMap<String, serde_json::Value>) {
        if let Some(ref mut bounds) = self.metadata_bounds {
            let partition_key = self
                .partition_config
                .as_ref()
                .and_then(|c| c.extract_partition_key(metadata));
            bounds.update_with_record(metadata, partition_key.as_deref());
        }
    }

    /// Enable metadata bounds tracking
    pub fn enable_metadata_bounds(&mut self) {
        if self.metadata_bounds.is_none() {
            self.metadata_bounds = Some(MetadataBounds::new());
        }
    }

    /// Check if this shard might contain data matching the filter context
    pub fn may_contain_data(&self, tenant_id: Option<&str>, domain_id: Option<&str>) -> bool {
        if let Some(ref bounds) = self.metadata_bounds {
            // Check tenant filter
            if let Some(tid) = tenant_id
                && !bounds.may_contain_tenant(tid)
            {
                return false;
            }
            // Check domain filter
            if let Some(did) = domain_id
                && !bounds.may_contain_domain(did)
            {
                return false;
            }
        }
        // If no bounds or no filters, assume it might contain relevant data
        true
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
        let replication_factor =
            replication_factor.unwrap_or(self.config.default_replication_factor);

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

    /// Update shard metadata bounds with records' metadata
    ///
    /// This method should be called after successful writes to update the
    /// shard's metadata bounds for tenant/domain/field tracking, enabling
    /// efficient shard pruning on subsequent queries.
    pub async fn update_shard_metadata_bounds(
        &self,
        shard_id: &ShardId,
        records_metadata: &[HashMap<String, serde_json::Value>],
    ) -> Result<()> {
        let mut shards = self.shards.write().await;

        if let Some(shard) = shards.get_mut(shard_id) {
            // Initialize metadata bounds if not present
            if shard.metadata_bounds.is_none() {
                shard.metadata_bounds = Some(MetadataBounds::new());
            }

            if let Some(ref mut bounds) = shard.metadata_bounds {
                for metadata in records_metadata {
                    let partition_key = shard
                        .partition_config
                        .as_ref()
                        .and_then(|c| c.extract_partition_key(metadata));
                    bounds.update_with_record(metadata, partition_key.as_deref());
                }
            }

            shard.updated_at = chrono::Utc::now().timestamp();

            tracing::debug!(
                shard_id = %shard_id,
                records_count = records_metadata.len(),
                "Updated shard metadata bounds"
            );

            Ok(())
        } else {
            Err(anyhow::anyhow!("Shard not found: {}", shard_id))
        }
    }

    /// Enable metadata bounds tracking for a shard
    pub async fn enable_metadata_bounds(&self, shard_id: &ShardId) -> Result<()> {
        let mut shards = self.shards.write().await;

        if let Some(shard) = shards.get_mut(shard_id) {
            shard.enable_metadata_bounds();
            shard.updated_at = chrono::Utc::now().timestamp();
            Ok(())
        } else {
            Err(anyhow::anyhow!("Shard not found: {}", shard_id))
        }
    }

    /// Get metadata bounds for a shard
    pub async fn get_metadata_bounds(&self, shard_id: &ShardId) -> Option<MetadataBounds> {
        let shards = self.shards.read().await;
        shards.get(shard_id).and_then(|s| s.metadata_bounds.clone())
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
    /// Total number of shards across the entire cluster
    pub total_shards: usize,
    /// Number of nodes participating in shard distribution
    pub node_count: usize,
    /// Average number of shards assigned per node
    pub avg_shards_per_node: f64,
    /// Maximum number of shards assigned to any single node
    pub max_shards_per_node: usize,
    /// Minimum number of shards assigned to any single node
    pub min_shards_per_node: usize,
    /// Ratio indicating shard distribution imbalance (0.0 = perfectly balanced)
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

    // ========================================================================
    // MetadataBounds Tests
    // ========================================================================

    #[test]
    fn test_metadata_bounds_new() {
        let bounds = MetadataBounds::new();
        assert!(bounds.tenant_ids.is_empty());
        assert!(bounds.domain_ids.is_empty());
        assert!(bounds.field_ranges.is_empty());
        assert!(bounds.partition_values.is_empty());
        assert!(bounds.last_updated > 0);
    }

    #[test]
    fn test_metadata_bounds_tenant_tracking() {
        let mut bounds = MetadataBounds::new();

        // Empty bounds should match any tenant
        assert!(bounds.may_contain_tenant("tenant-1"));

        // Add a tenant
        let mut metadata = HashMap::new();
        metadata.insert("tenant_id".to_string(), serde_json::json!("tenant-1"));
        bounds.update_with_record(&metadata, None);

        // Should match tracked tenant
        assert!(bounds.may_contain_tenant("tenant-1"));
        // Should NOT match other tenants
        assert!(!bounds.may_contain_tenant("tenant-2"));

        // Add another tenant
        metadata.insert("tenant_id".to_string(), serde_json::json!("tenant-2"));
        bounds.update_with_record(&metadata, None);

        // Should match both
        assert!(bounds.may_contain_tenant("tenant-1"));
        assert!(bounds.may_contain_tenant("tenant-2"));
        assert!(!bounds.may_contain_tenant("tenant-3"));
    }

    #[test]
    fn test_metadata_bounds_domain_tracking() {
        let mut bounds = MetadataBounds::new();

        let mut metadata = HashMap::new();
        metadata.insert("domain_id".to_string(), serde_json::json!("sales"));
        bounds.update_with_record(&metadata, None);

        assert!(bounds.may_contain_domain("sales"));
        assert!(!bounds.may_contain_domain("marketing"));
    }

    #[test]
    fn test_metadata_bounds_partition_key() {
        let mut bounds = MetadataBounds::new();

        let metadata = HashMap::new();
        bounds.update_with_record(&metadata, Some("partition-a"));

        assert!(bounds.may_contain_partition("partition-a"));
        assert!(!bounds.may_contain_partition("partition-b"));
    }

    #[test]
    fn test_metadata_bounds_field_ranges_numeric() {
        let mut bounds = MetadataBounds::new();

        // Add records with numeric field
        let mut metadata = HashMap::new();
        metadata.insert("price".to_string(), serde_json::json!(100.0));
        bounds.update_with_record(&metadata, None);

        metadata.insert("price".to_string(), serde_json::json!(500.0));
        bounds.update_with_record(&metadata, None);

        // Check range bounds
        assert!(bounds.may_contain_field_value("price", &serde_json::json!(100.0)));
        assert!(bounds.may_contain_field_value("price", &serde_json::json!(300.0)));
        assert!(bounds.may_contain_field_value("price", &serde_json::json!(500.0)));

        // Out of range
        assert!(!bounds.may_contain_field_value("price", &serde_json::json!(50.0)));
        assert!(!bounds.may_contain_field_value("price", &serde_json::json!(600.0)));

        // Unknown field should always return true
        assert!(bounds.may_contain_field_value("unknown", &serde_json::json!(100)));
    }

    #[test]
    fn test_metadata_bounds_field_ranges_string() {
        let mut bounds = MetadataBounds::new();

        let mut metadata = HashMap::new();
        metadata.insert("category".to_string(), serde_json::json!("apple"));
        bounds.update_with_record(&metadata, None);

        metadata.insert("category".to_string(), serde_json::json!("orange"));
        bounds.update_with_record(&metadata, None);

        // In range (alphabetically between apple and orange)
        assert!(bounds.may_contain_field_value("category", &serde_json::json!("banana")));
        assert!(bounds.may_contain_field_value("category", &serde_json::json!("apple")));
        assert!(bounds.may_contain_field_value("category", &serde_json::json!("orange")));

        // Out of range
        assert!(!bounds.may_contain_field_value("category", &serde_json::json!("zebra")));
    }

    #[test]
    fn test_metadata_bounds_merge() {
        let mut bounds1 = MetadataBounds::new();
        let mut bounds2 = MetadataBounds::new();

        // Populate bounds1
        let mut meta1 = HashMap::new();
        meta1.insert("tenant_id".to_string(), serde_json::json!("tenant-1"));
        meta1.insert("price".to_string(), serde_json::json!(100.0));
        bounds1.update_with_record(&meta1, Some("pk-1"));

        // Populate bounds2
        let mut meta2 = HashMap::new();
        meta2.insert("tenant_id".to_string(), serde_json::json!("tenant-2"));
        meta2.insert("price".to_string(), serde_json::json!(200.0));
        bounds2.update_with_record(&meta2, Some("pk-2"));

        // Merge bounds2 into bounds1
        bounds1.merge(&bounds2);

        // Should have both tenants
        assert!(bounds1.may_contain_tenant("tenant-1"));
        assert!(bounds1.may_contain_tenant("tenant-2"));

        // Should have both partition keys
        assert!(bounds1.may_contain_partition("pk-1"));
        assert!(bounds1.may_contain_partition("pk-2"));

        // Price range should be expanded
        assert!(bounds1.may_contain_field_value("price", &serde_json::json!(100.0)));
        assert!(bounds1.may_contain_field_value("price", &serde_json::json!(150.0)));
        assert!(bounds1.may_contain_field_value("price", &serde_json::json!(200.0)));
    }

    #[test]
    fn test_metadata_bounds_tenant_record_counts() {
        let mut bounds = MetadataBounds::new();

        let mut metadata = HashMap::new();
        metadata.insert("tenant_id".to_string(), serde_json::json!("tenant-1"));

        // Add 3 records for tenant-1
        bounds.update_with_record(&metadata, None);
        bounds.update_with_record(&metadata, None);
        bounds.update_with_record(&metadata, None);

        assert_eq!(bounds.tenant_record_counts.get("tenant-1"), Some(&3));
    }

    // ========================================================================
    // PartitionStrategy Tests
    // ========================================================================

    #[test]
    fn test_partition_strategy_default() {
        let strategy = PartitionStrategy::default();
        assert!(matches!(strategy, PartitionStrategy::HashId));
    }

    #[test]
    fn test_partition_config_extract_key_hash_id() {
        let config = PartitionConfig {
            strategy: PartitionStrategy::HashId,
            partition_key_fields: vec![],
            track_metadata_bounds: false,
        };

        let metadata = HashMap::new();
        assert!(config.extract_partition_key(&metadata).is_none());
    }

    #[test]
    fn test_partition_config_extract_key_tenant() {
        let config = PartitionConfig {
            strategy: PartitionStrategy::Tenant,
            partition_key_fields: vec![],
            track_metadata_bounds: true,
        };

        let mut metadata = HashMap::new();
        metadata.insert("tenant_id".to_string(), serde_json::json!("acme-corp"));

        let key = config.extract_partition_key(&metadata);
        assert_eq!(key, Some("acme-corp".to_string()));
    }

    #[test]
    fn test_partition_config_extract_key_domain() {
        let config = PartitionConfig {
            strategy: PartitionStrategy::Domain,
            partition_key_fields: vec![],
            track_metadata_bounds: true,
        };

        let mut metadata = HashMap::new();
        metadata.insert("domain_id".to_string(), serde_json::json!("sales-team"));

        let key = config.extract_partition_key(&metadata);
        assert_eq!(key, Some("sales-team".to_string()));
    }

    #[test]
    fn test_partition_config_extract_key_hash_metadata() {
        let config = PartitionConfig {
            strategy: PartitionStrategy::HashMetadata {
                fields: vec!["region".to_string(), "year".to_string()],
            },
            partition_key_fields: vec![],
            track_metadata_bounds: true,
        };

        let mut metadata = HashMap::new();
        metadata.insert("region".to_string(), serde_json::json!("us-west"));
        metadata.insert("year".to_string(), serde_json::json!(2024));

        let key = config.extract_partition_key(&metadata);
        assert_eq!(key, Some("us-west:2024".to_string()));
    }

    #[test]
    fn test_partition_config_extract_key_range() {
        let config = PartitionConfig {
            strategy: PartitionStrategy::Range {
                field: "timestamp".to_string(),
                boundaries: vec![],
            },
            partition_key_fields: vec![],
            track_metadata_bounds: true,
        };

        let mut metadata = HashMap::new();
        metadata.insert("timestamp".to_string(), serde_json::json!("2024-01-15"));

        let key = config.extract_partition_key(&metadata);
        assert_eq!(key, Some("2024-01-15".to_string()));
    }

    // ========================================================================
    // Shard with MetadataBounds Tests
    // ========================================================================

    #[test]
    fn test_shard_with_partition_config() {
        let config = PartitionConfig {
            strategy: PartitionStrategy::Tenant,
            partition_key_fields: vec![],
            track_metadata_bounds: true,
        };

        let shard = Shard::with_partition_config("test-collection", 0, config);

        assert!(shard.partition_config.is_some());
        assert!(shard.metadata_bounds.is_some());
    }

    #[test]
    fn test_shard_update_metadata_bounds() {
        let config = PartitionConfig {
            strategy: PartitionStrategy::Tenant,
            partition_key_fields: vec![],
            track_metadata_bounds: true,
        };

        let mut shard = Shard::with_partition_config("test", 0, config);

        let mut metadata = HashMap::new();
        metadata.insert("tenant_id".to_string(), serde_json::json!("tenant-a"));
        metadata.insert("domain_id".to_string(), serde_json::json!("domain-x"));

        shard.update_metadata_bounds(&metadata);

        let bounds = shard.metadata_bounds.as_ref().unwrap();
        assert!(bounds.tenant_ids.contains("tenant-a"));
        assert!(bounds.domain_ids.contains("domain-x"));
    }

    #[test]
    fn test_shard_may_contain_data() {
        let config = PartitionConfig {
            strategy: PartitionStrategy::Tenant,
            partition_key_fields: vec![],
            track_metadata_bounds: true,
        };

        let mut shard = Shard::with_partition_config("test", 0, config);

        // Initially (empty bounds), should match everything
        assert!(shard.may_contain_data(Some("any-tenant"), None));

        // Add some data
        let mut metadata = HashMap::new();
        metadata.insert("tenant_id".to_string(), serde_json::json!("tenant-a"));
        shard.update_metadata_bounds(&metadata);

        // Should match tenant-a
        assert!(shard.may_contain_data(Some("tenant-a"), None));
        // Should NOT match tenant-b
        assert!(!shard.may_contain_data(Some("tenant-b"), None));
        // No tenant filter should match
        assert!(shard.may_contain_data(None, None));
    }

    #[test]
    fn test_shard_enable_metadata_bounds() {
        let mut shard = Shard::new("test", 0);
        assert!(shard.metadata_bounds.is_none());

        shard.enable_metadata_bounds();
        assert!(shard.metadata_bounds.is_some());

        // Enabling again should not reset
        let mut metadata = HashMap::new();
        metadata.insert("tenant_id".to_string(), serde_json::json!("test"));
        shard.update_metadata_bounds(&metadata);

        shard.enable_metadata_bounds();
        assert!(
            shard
                .metadata_bounds
                .as_ref()
                .unwrap()
                .tenant_ids
                .contains("test")
        );
    }
}
