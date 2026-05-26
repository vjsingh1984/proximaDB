//! Cache storage backends for tiered caching
//!
//! This module provides storage backend implementations for different cache tiers:
//!
//! - **L1 (Memory)**: In-memory cache with fastest access
//! - **L2 (NVMe/SSD)**: Local disk cache with fast access
//! - **L3 (Network)**: Remote cache (Redis, Memcached, etc.)

pub mod memory_tier;
pub mod network_tier;
pub mod nvme_tier;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

pub use memory_tier::MemoryBackend;
pub use network_tier::NetworkBackend;
pub use nvme_tier::NvmeBackend;

/// Cache storage tier
///
/// Represents the three levels of cache in the hierarchical caching system,
/// from fastest (L1) to slowest (L3) but with increasing capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CacheTier {
    /// L1: In-memory cache (fastest, smallest capacity)
    L1,
    /// L2: NVMe/SSD cache (fast, medium capacity)
    L2,
    /// L3: Network/cloud cache (slower, largest capacity)
    L3,
}

impl CacheTier {
    /// Get the name of this cache tier
    ///
    /// # Returns
    ///
    /// A string representation of the tier name
    pub fn name(&self) -> &'static str {
        match self {
            CacheTier::L1 => "memory",
            CacheTier::L2 => "nvme",
            CacheTier::L3 => "network",
        }
    }

    /// Get the priority of this cache tier
    ///
    /// Lower values indicate higher priority (faster access)
    ///
    /// # Returns
    ///
    /// The priority value (1 = highest, 3 = lowest)
    pub fn priority(&self) -> u8 {
        match self {
            CacheTier::L1 => 1,
            CacheTier::L2 => 2,
            CacheTier::L3 => 3,
        }
    }
}

/// Storage backend trait for different cache tiers
///
/// This trait defines the interface for cache storage backends across
/// all tiers (L1 memory, L2 NVMe, L3 network). All backends must support
/// basic CRUD operations with async support.
#[async_trait]
pub trait StorageBackend: Send + Sync + Debug {
    /// Key type for this backend
    type Key: Clone + Send + Sync;
    /// Value type for this backend
    type Value: Clone + Send + Sync;

    /// Get a value from the storage backend
    ///
    /// # Arguments
    ///
    /// * `key` - The key to look up
    ///
    /// # Returns
    ///
    /// `Some(value)` if the key exists, `None` otherwise
    async fn get(&self, key: &Self::Key) -> Option<Self::Value>;

    /// Put a value into the storage backend
    ///
    /// # Arguments
    ///
    /// * `key` - The key to store
    /// * `value` - The value to store
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, `Err(StorageError)` on failure
    async fn put(&self, key: Self::Key, value: Self::Value) -> Result<(), StorageError>;

    /// Remove a value from the storage backend
    ///
    /// # Arguments
    ///
    /// * `key` - The key to remove
    ///
    /// # Returns
    ///
    /// `true` if the key was found and removed, `false` otherwise
    async fn remove(&self, key: &Self::Key) -> bool;

    /// Check if a key exists in the storage backend
    ///
    /// # Arguments
    ///
    /// * `key` - The key to check
    ///
    /// # Returns
    ///
    /// `true` if the key exists, `false` otherwise
    async fn contains(&self, key: &Self::Key) -> bool;

    /// Clear all entries in the storage backend
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, `Err(StorageError)` on failure
    async fn clear(&self) -> Result<(), StorageError>;

    /// Get the current size in bytes
    ///
    /// # Returns
    ///
    /// The total size of all stored values in bytes
    async fn size_bytes(&self) -> usize;

    /// Get the number of entries
    ///
    /// # Returns
    ///
    /// The number of key-value pairs stored
    async fn entry_count(&self) -> usize;

    /// Get the tier this backend represents
    ///
    /// # Returns
    ///
    /// The cache tier (L1, L2, or L3)
    fn tier(&self) -> CacheTier;
}

/// Re-export the canonical storage error from `proximadb_storage_common`.
///
/// Previously this module defined its own `StorageError` enum, but that was
/// a third copy alongside the canonical pair (kernel + storage_common).
/// The cache backend tier is a `proximadb_storage_common` consumer; callers
/// match on `err.kind == StorageErrorKind::CapacityExceeded` and construct
/// via the existing constructors (`capacity_exceeded`, `io`, etc.).
pub use proximadb_storage_common::{StorageError, StorageErrorKind};

/// Configuration for tiered storage
#[derive(Debug, Clone)]
pub struct TieredStorageConfig {
    /// L1 memory cache size in MB
    pub l1_memory_mb: usize,
    /// L2 NVMe cache size in GB
    pub l2_nvme_gb: Option<usize>,
    /// L3 network cache configuration
    pub l3_network: Option<NetworkCacheConfig>,
    /// Promotion threshold (number of accesses before promotion)
    pub promotion_threshold: u32,
    /// Demotion threshold (age in seconds before demotion)
    pub demotion_threshold_secs: u64,
}

#[derive(Debug, Clone)]
pub struct NetworkCacheConfig {
    pub endpoint: String,
    pub timeout_ms: u64,
    pub max_connections: usize,
}

impl Default for TieredStorageConfig {
    fn default() -> Self {
        Self {
            l1_memory_mb: 1024,
            l2_nvme_gb: None,
            l3_network: None,
            promotion_threshold: 3,
            demotion_threshold_secs: 3600,
        }
    }
}
