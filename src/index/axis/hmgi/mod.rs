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

//! # HMGI - Hierarchical Multi-modality Graph Indexing
//!
//! This module implements HMGI (arXiv:2510.10123) for per-modality HNSW partitioning
//! in ProximaDB. HMGI reduces search space by 70% and improves accuracy from 80% to 95%
//! compared to monolithic HNSW for multi-modality data.
//!
//! ## Architecture
//!
//! ```text
//! Collection → HMGI Partitions → Per-Modality HNSW Indexes
//!      ↓              ↓                    ↓
//!  Multi-Modal    (oid, variation,    Vector Search
//!    Data          modality_tag)       per Modality
//! ```
//!
//! ## Key Components
//!
//! - **`HmgiPartitionKey`**: Uniquely identifies a modality partition
//! - **`HmgiRegistry`**: Manages per-modality HNSW index lifecycle
//! - **`ModalityExtractor`**: Extracts modality tags from records
//! - **`HmgiRouter`**: Routes queries to relevant partitions
//! - **`HmgiTierPolicy`**: Per-modality storage tiering
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::index::axis::hmgi::{
//!     HmgiPartitionKey, HmgiRegistry, ModalityExtractor,
//! };
//!
//! // Create partition key
//! let key = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));
//!
//! // Get or create partition
//! let registry = HmgiRegistry::new();
//! let index = registry.get_or_create_partition(key, config).await?;
//! ```

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Number of virtual nodes per physical node for better distribution
const VIRTUAL_NODES_PER_NODE: u32 = 100;

/// HMGI consistent hash ring for partition distribution
///
/// Simple consistent hashing implementation for HMGI.
/// Uses SHA-256 for uniform distribution across cluster nodes.
#[derive(Debug, Clone)]
pub struct ConsistentHashRing {
    /// Hash ring with virtual nodes mapping to physical nodes
    ring: BTreeMap<u64, u64>,
    /// Number of physical nodes
    node_count: u32,
}

impl ConsistentHashRing {
    /// Create a new consistent hash ring
    pub fn new(node_count: u32) -> Self {
        let mut ring = BTreeMap::new();

        // Create virtual nodes for each physical node
        for node_id in 0..node_count {
            for virtual_id in 0..VIRTUAL_NODES_PER_NODE {
                let key = format!("node_{}_{}", node_id, virtual_id);
                let hash = Self::hash_key(&key);
                ring.insert(hash, node_id as u64);
            }
        }

        Self { ring, node_count }
    }

    /// Get node ID for a given partition key
    pub fn get_node_for_key(&self, oid: u64, variation_id: u32, modality_tag: &str) -> Option<u64> {
        if self.node_count == 0 {
            return None;
        }

        let key_string = format!("{}:{}:{}", oid, variation_id, modality_tag);
        let hash = Self::hash_key(&key_string);

        // Find the first node with hash >= partition hash
        if let Some((&_, &node_id)) = self.ring.range(hash..).next() {
            Some(node_id)
        } else {
            // Wrap around to the beginning
            self.ring.iter().next().map(|(_, &node_id)| node_id)
        }
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

    /// Get the number of nodes in the ring
    pub fn node_count(&self) -> u32 {
        self.node_count
    }
}

pub mod coordinator;
pub mod detection;
pub mod distributed;
pub mod extraction;
pub mod migration;
pub mod pruning;
pub mod registry;
pub mod router;
pub mod tiering;

// Re-exports for convenience
pub use coordinator::{
    HmgiQueryCoordinator, HmgiSearchRequest, MockNetworkService, NetworkService,
};
pub use detection::{
    CollectionTransition, DetectionResult, EnablementReason, ModalityDetector, VectorRecordSample,
};
pub use distributed::{
    ClusterMembership, ClusterNode, ClusterNodeId, DistributedPartitionLocator, NodeState,
};
pub use extraction::ModalityExtractor;
pub use migration::{
    HmgiMigrationEngine, HmgiMigrationPhase, MigrationConfig, MigrationResult, MigrationState,
    WritePauseToken,
};
pub use pruning::PartitionMetadata;
pub use registry::HmgiRegistry;
pub use router::{HmgiRouteStats, HmgiRouter, ResultMerger};
pub use tiering::{HmgiTierPolicy, TierChangeReason, TierChangeRecommendation, TierChangeResult};

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// HMGI partition key - uniquely identifies a modality partition
///
/// This key enables per-modality HNSW partitions as required by HMGI research.
/// The `(oid, variation, modality_tag)` tuple creates unique subspaces where
/// each HNSW index operates on a semantically coherent subset.
///
/// ## Example
///
/// ```
/// use proximadb::index::axis::hmgi::HmgiPartitionKey;
///
/// // Text documents partition
/// let text_key = HmgiPartitionKey::new(
///     123,  // oid: entity type for "documents"
///     1,    // variation: structural version
///     "text".to_string(),
///     Some(456), // tenant_id
/// );
///
/// // Image documents partition
/// let image_key = HmgiPartitionKey::new(
///     123,  // same oid
///     1,    // same variation
///     "image".to_string(),
///     Some(456), // same tenant
/// );
///
/// assert_ne!(text_key, image_key); // Different partitions
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HmgiPartitionKey {
    /// Entity type ID from catalog (e.g., "documents", "users", "products")
    pub oid: u64,

    /// Structural variation within the entity type
    pub variation_id: u32,

    /// Modality tag (e.g., "text", "image", "audio", "video", "graph", "time-series")
    pub modality_tag: String,

    /// Optional tenant ID for multi-tenant isolation
    pub tenant_id: Option<u64>,
}

impl HmgiPartitionKey {
    /// Create a new HMGI partition key
    ///
    /// ## Arguments
    ///
    /// - `oid`: Entity type ID from catalog
    /// - `variation_id`: Structural variation within entity type
    /// - `modality_tag`: Modality identifier
    /// - `tenant_id`: Optional tenant for multi-tenancy
    #[must_use]
    pub const fn new(
        oid: u64,
        variation_id: u32,
        modality_tag: String,
        tenant_id: Option<u64>,
    ) -> Self {
        Self {
            oid,
            variation_id,
            modality_tag,
            tenant_id,
        }
    }

    /// Create a partition key without tenant isolation
    #[must_use]
    pub const fn without_tenant(oid: u64, variation_id: u32, modality_tag: String) -> Self {
        Self {
            oid,
            variation_id,
            modality_tag,
            tenant_id: None,
        }
    }

    /// Get the hash of this partition key for consistent routing
    ///
    /// Uses FxHash for fast hash computation suitable for routing decisions.
    pub fn routing_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        self.oid.hash(&mut hasher);
        self.variation_id.hash(&mut hasher);
        self.modality_tag.hash(&mut hasher);
        self.tenant_id.hash(&mut hasher);
        hasher.finish()
    }

    /// Create a collection-scoped partition key (no tenant)
    pub fn for_collection(oid: u64, variation_id: u32, modality_tag: String) -> Self {
        Self::without_tenant(oid, variation_id, modality_tag)
    }

    /// Check if this partition is within the given tenant scope
    pub fn matches_tenant(&self, tenant_id: Option<u64>) -> bool {
        match (self.tenant_id, tenant_id) {
            (None, _) => true,        // Public partition matches any query
            (Some(_), None) => false, // Tenant-scoped partition doesn't match public query
            (Some(key_tenant), Some(query_tenant)) => key_tenant == query_tenant,
        }
    }
}

impl std::fmt::Display for HmgiPartitionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(tenant) = self.tenant_id {
            write!(
                f,
                "oid:{}/var:{}/mod:{}/tenant:{}",
                self.oid, self.variation_id, self.modality_tag, tenant
            )
        } else {
            write!(
                f,
                "oid:{}/var:{}/mod:{}",
                self.oid, self.variation_id, self.modality_tag
            )
        }
    }
}

/// Set of partition keys for routing decisions
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionSet {
    partitions: HashSet<HmgiPartitionKey>,
}

impl PartitionSet {
    /// Create an empty partition set
    pub fn new() -> Self {
        Self {
            partitions: HashSet::new(),
        }
    }

    /// Add a partition to the set
    pub fn insert(&mut self, key: HmgiPartitionKey) -> bool {
        self.partitions.insert(key)
    }

    /// Check if a partition is in the set
    pub fn contains(&self, key: &HmgiPartitionKey) -> bool {
        self.partitions.contains(key)
    }

    /// Get all partitions
    pub fn iter(&self) -> impl Iterator<Item = &HmgiPartitionKey> {
        self.partitions.iter()
    }

    /// Get number of partitions
    pub fn len(&self) -> usize {
        self.partitions.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.partitions.is_empty()
    }

    /// Filter partitions by tenant
    pub fn for_tenant(&self, tenant_id: Option<u64>) -> PartitionSet {
        PartitionSet {
            partitions: self
                .partitions
                .iter()
                .filter(|k| k.matches_tenant(tenant_id))
                .cloned()
                .collect(),
        }
    }

    /// Filter partitions by modality tag
    pub fn for_modality(&self, modality_tag: &str) -> PartitionSet {
        PartitionSet {
            partitions: self
                .partitions
                .iter()
                .filter(|k| k.modality_tag == modality_tag)
                .cloned()
                .collect(),
        }
    }

    /// Filter partitions by multiple modality tags
    pub fn for_modalities(&self, modality_tags: &[String]) -> PartitionSet {
        let tag_set: HashSet<_> = modality_tags.iter().cloned().collect();
        PartitionSet {
            partitions: self
                .partitions
                .iter()
                .filter(|k| tag_set.contains(&k.modality_tag))
                .cloned()
                .collect(),
        }
    }
}

impl Default for PartitionSet {
    fn default() -> Self {
        Self::new()
    }
}

impl FromIterator<HmgiPartitionKey> for PartitionSet {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = HmgiPartitionKey>,
    {
        PartitionSet {
            partitions: iter.into_iter().collect(),
        }
    }
}

impl From<Vec<HmgiPartitionKey>> for PartitionSet {
    fn from(vec: Vec<HmgiPartitionKey>) -> Self {
        PartitionSet {
            partitions: vec.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_partition_key_equality() {
        let key1 = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));
        let key2 = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));
        let key3 = HmgiPartitionKey::new(123, 1, "image".to_string(), Some(456));

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_partition_key_hashing() {
        let key1 = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));
        let key2 = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));
        let key3 = HmgiPartitionKey::new(124, 1, "text".to_string(), Some(456));

        // Equal keys should have equal hashes
        assert_eq!(key1.routing_hash(), key2.routing_hash());

        // Different keys should (very likely) have different hashes
        assert_ne!(key1.routing_hash(), key3.routing_hash());
    }

    #[test]
    fn test_partition_key_serialization() {
        let key = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));

        // JSON round-trip
        let json = serde_json::to_string(&key).unwrap();
        let deserialized: HmgiPartitionKey = serde_json::from_str(&json).unwrap();
        assert_eq!(key, deserialized);

        // Bincode round-trip
        let bytes = bincode::serialize(&key).unwrap();
        let deserialized: HmgiPartitionKey = bincode::deserialize(&bytes).unwrap();
        assert_eq!(key, deserialized);
    }

    #[test]
    fn test_partition_key_display() {
        let key_with_tenant = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));
        assert_eq!(
            format!("{}", key_with_tenant),
            "oid:123/var:1/mod:text/tenant:456"
        );

        let key_without_tenant = HmgiPartitionKey::without_tenant(123, 1, "text".to_string());
        assert_eq!(format!("{}", key_without_tenant), "oid:123/var:1/mod:text");
    }

    #[test]
    fn test_partition_key_matches_tenant() {
        let public_key = HmgiPartitionKey::without_tenant(123, 1, "text".to_string());
        let tenant_key = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));
        let other_tenant_key = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(789));

        // Public partition matches any query
        assert!(public_key.matches_tenant(None));
        assert!(public_key.matches_tenant(Some(456)));

        // Tenant partition only matches its tenant
        assert!(!tenant_key.matches_tenant(None));
        assert!(tenant_key.matches_tenant(Some(456)));
        assert!(!tenant_key.matches_tenant(Some(789)));

        // Different tenant doesn't match
        assert!(!other_tenant_key.matches_tenant(Some(456)));
    }

    #[test]
    fn test_partition_set_basic() {
        let mut set = PartitionSet::new();
        let key1 = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));
        let key2 = HmgiPartitionKey::new(123, 1, "image".to_string(), Some(456));

        assert!(set.is_empty());
        assert_eq!(set.len(), 0);

        set.insert(key1.clone());
        assert!(!set.is_empty());
        assert_eq!(set.len(), 1);
        assert!(set.contains(&key1));

        set.insert(key2);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_partition_set_for_modality() {
        let mut set = PartitionSet::new();
        set.insert(HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456)));
        set.insert(HmgiPartitionKey::new(
            123,
            1,
            "image".to_string(),
            Some(456),
        ));
        set.insert(HmgiPartitionKey::new(
            123,
            1,
            "video".to_string(),
            Some(456),
        ));

        let text_only = set.for_modality("text");
        assert_eq!(text_only.len(), 1);
        assert!(text_only.contains(&HmgiPartitionKey::new(
            123,
            1,
            "text".to_string(),
            Some(456)
        )));

        let multi = set.for_modalities(&["text".to_string(), "image".to_string()]);
        assert_eq!(multi.len(), 2);
    }

    #[test]
    fn test_partition_set_for_tenant() {
        let mut set = PartitionSet::new();
        set.insert(HmgiPartitionKey::without_tenant(
            123,
            1,
            "public".to_string(),
        ));
        set.insert(HmgiPartitionKey::new(
            123,
            1,
            "tenant1".to_string(),
            Some(456),
        ));
        set.insert(HmgiPartitionKey::new(
            123,
            1,
            "tenant2".to_string(),
            Some(789),
        ));

        // Query without tenant gets public partitions only
        let all = set.for_tenant(None);
        assert_eq!(all.len(), 1);

        // Query with tenant gets tenant + public
        let tenant456 = set.for_tenant(Some(456));
        assert_eq!(tenant456.len(), 2); // public + tenant1
    }

    #[test]
    fn test_partition_set_from_iterator() {
        let keys = vec![
            HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456)),
            HmgiPartitionKey::new(123, 1, "image".to_string(), Some(456)),
        ];

        let set: PartitionSet = keys.clone().into_iter().collect();
        assert_eq!(set.len(), 2);

        let set2 = PartitionSet::from(keys);
        assert_eq!(set2.len(), 2);
    }
}
