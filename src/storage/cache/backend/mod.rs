pub mod memory;
pub mod nvme;
pub mod network;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

pub use memory::MemoryBackend;
pub use nvme::NvmeBackend;
pub use network::NetworkBackend;

/// Cache storage tier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CacheTier {
    /// L1: In-memory cache (fastest)
    L1,
    /// L2: NVMe/SSD cache (fast)
    L2,
    /// L3: Network/cloud cache (slower)
    L3,
}

impl CacheTier {
    pub fn name(&self) -> &'static str {
        match self {
            CacheTier::L1 => "memory",
            CacheTier::L2 => "nvme",
            CacheTier::L3 => "network",
        }
    }
    
    pub fn priority(&self) -> u8 {
        match self {
            CacheTier::L1 => 1,
            CacheTier::L2 => 2,
            CacheTier::L3 => 3,
        }
    }
}

/// Storage backend trait for different cache tiers
#[async_trait]
pub trait StorageBackend: Send + Sync + Debug {
    type Key: Clone + Send + Sync;
    type Value: Clone + Send + Sync;
    
    /// Get a value from the storage backend
    async fn get(&self, key: &Self::Key) -> Option<Self::Value>;
    
    /// Put a value into the storage backend
    async fn put(&self, key: Self::Key, value: Self::Value) -> Result<(), StorageError>;
    
    /// Remove a value from the storage backend
    async fn remove(&self, key: &Self::Key) -> bool;
    
    /// Check if a key exists in the storage backend
    async fn contains(&self, key: &Self::Key) -> bool;
    
    /// Clear all entries in the storage backend
    async fn clear(&self) -> Result<(), StorageError>;
    
    /// Get the current size in bytes
    async fn size_bytes(&self) -> usize;
    
    /// Get the number of entries
    async fn entry_count(&self) -> usize;
    
    /// Get the tier this backend represents
    fn tier(&self) -> CacheTier;
}

/// Errors that can occur in storage backends
#[derive(Debug, Clone)]
pub enum StorageError {
    IoError(String),
    SerializationError(String),
    CapacityExceeded,
    NetworkError(String),
    Other(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::IoError(msg) => write!(f, "IO error: {}", msg),
            StorageError::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
            StorageError::CapacityExceeded => write!(f, "Storage capacity exceeded"),
            StorageError::NetworkError(msg) => write!(f, "Network error: {}", msg),
            StorageError::Other(msg) => write!(f, "Storage error: {}", msg),
        }
    }
}

impl std::error::Error for StorageError {}

/// Configuration for tiered storage
#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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