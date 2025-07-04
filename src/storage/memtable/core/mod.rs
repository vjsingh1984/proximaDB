//! Core Memtable Traits and Abstractions
//! 
//! Provides unified interfaces for memtable implementations used by both
//! WAL (Write-Ahead Log) and LSM (Log-Structured Merge-tree) engines.
//! 
//! Architecture:
//! - WAL uses BTree for ordered writes and optimal compression
//! - LSM uses SkipList for concurrent access and range queries
//! - Common traits enable polymorphic usage and testing

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;

/// Core memtable trait for unified storage operations
/// 
/// Generic over key and value types to support:
/// - WAL: K=u64 (sequence), V=WalEntry
/// - LSM: K=VectorId, V=LsmEntry
#[async_trait]
pub trait MemtableCore<K, V>: Send + Sync + Debug
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    /// Insert a key-value pair, returning the memory size delta
    async fn insert(&self, key: K, value: V) -> Result<u64>;
    
    /// Get value by key
    async fn get(&self, key: &K) -> Result<Option<V>>;
    
    /// Range scan from key with optional limit
    async fn range_scan(&self, from: K, limit: Option<usize>) -> Result<Vec<(K, V)>>;
    
    /// Current memory usage in bytes
    async fn size_bytes(&self) -> usize;
    
    /// Get approximate number of entries
    async fn len(&self) -> usize;
    
    /// Check if memtable is empty
    async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
    
    /// Clear entries up to (and including) threshold key
    /// Returns number of entries removed
    async fn clear_up_to(&self, threshold: K) -> Result<usize>;
    
    /// Clear all entries (for testing/reset)
    async fn clear(&self) -> Result<()>;
    
    /// Get all entries in key order (for flushing)
    async fn get_all_ordered(&self) -> Result<Vec<(K, V)>>;
}

/// MVCC (Multi-Version Concurrency Control) support for memtables
/// 
/// Enables multiple versions of the same logical key to coexist
/// Essential for WAL recovery and LSM compaction operations
#[async_trait]
pub trait MemtableMVCC<K, V>: MemtableCore<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    /// Get all versions of a logical key
    async fn get_versions(&self, logical_key: &str) -> Result<Vec<(K, V)>>;
    
    /// Get latest version of a logical key
    async fn get_latest(&self, logical_key: &str) -> Result<Option<(K, V)>>;
    
    /// Cleanup old versions (keep only N latest)
    async fn cleanup_versions(&self, logical_key: &str, keep_count: usize) -> Result<usize>;
}

/// Memory and performance metrics for memtables
#[derive(Debug, Clone, Default)]
pub struct MemtableMetrics {
    pub size_bytes: usize,
    pub entry_count: usize,
    pub insert_count: u64,
    pub get_count: u64,
    pub scan_count: u64,
    pub flush_count: u64,
    pub mvcc_versions_total: usize,
}

/// Memtable configuration options
#[derive(Debug, Clone)]
pub struct MemtableConfig {
    pub max_size_bytes: usize,
    pub flush_threshold_bytes: usize,
    pub enable_mvcc: bool,
    pub mvcc_cleanup_interval_secs: u64,
    pub max_versions_per_key: usize,
}

impl Default for MemtableConfig {
    fn default() -> Self {
        Self {
            max_size_bytes: 64 * 1024 * 1024,  // 64MB
            flush_threshold_bytes: 16 * 1024 * 1024,  // 16MB
            enable_mvcc: true,
            mvcc_cleanup_interval_secs: 300,  // 5 minutes
            max_versions_per_key: 10,
        }
    }
}

// Re-export common types for convenience (these may not exist yet)
// pub use crate::core::{VectorId, VectorRecord};
// pub use crate::storage::persistence::wal::WalEntry;
// pub use crate::storage::engines::lsm::LsmEntry;