//! Specialized Wrappers for Memtable Implementations
//!
//! Provides behavior-specific wrappers around core data structures using OOP principles:
//! - Generic wrappers for common patterns
//! - Specialized wrappers for WAL and LSM use cases
//! - Inheritance-like composition for code reuse
//!
//! ## Architecture:
//! - Core data structures remain in their original files (btree.rs, skiplist.rs, etc.)
//! - Specialized wrappers extend core functionality for specific use cases
//! - Type aliases provide convenient naming for common patterns

pub mod generic_wrappers;
pub mod lsm_behavior;
pub mod wal_behavior;

// Type aliases for convenience - these expose the actual data structures
/// 🚫 DEPRECATED: WalMemtable uses individual-entry paradigm
/// Use WalBehaviorWrapper directly for batch-oriented operations
#[deprecated(note = "Use WalBehaviorWrapper directly for batch-oriented operations")]
pub type WalMemtable = wal_behavior::WalBehaviorWrapper;
pub type LsmMemtable<K, V> = lsm_behavior::LsmBehaviorWrapper<
    crate::storage::memtable::implementations::skiplist::SkipListMemtable<K, V>,
>;

// Additional aliases for different use cases
pub type ConcurrentHashMap<K, V> =
    crate::storage::memtable::implementations::dashmap::DashMapMemtable<K, V>;
// Temporarily using BTree instead of ART due to type inference issues
pub type MemoryEfficientMap<K, V> =
    crate::storage::memtable::implementations::btree::BTreeMemtable<K, V>;
pub type FastLookupMap<K, V> =
    crate::storage::memtable::implementations::hashmap::HashMapMemtable<K, V>;

use crate::storage::memtable::core::MemtableConfig;
use anyhow::Result;

/// Factory methods that use actual data structure names
pub struct SpecializedMemtableFactory;

impl SpecializedMemtableFactory {
    /// Create global partitioned memtable with WAL-specific behavior
    pub fn create_global_partitioned_for_wal(config: MemtableConfig) -> WalMemtable {
        wal_behavior::WalBehaviorWrapper::new(config)
    }

    /// Create SkipList with LSM-specific behavior
    pub fn create_skiplist_for_lsm<K, V>(config: MemtableConfig) -> LsmMemtable<K, V>
    where
        K: Clone + Ord + std::hash::Hash + Send + Sync + std::fmt::Debug,
        V: Clone + Send + Sync + std::fmt::Debug,
    {
        let skiplist = crate::storage::memtable::implementations::skiplist::SkipListMemtable::new();
        lsm_behavior::LsmBehaviorWrapper::new(skiplist, config)
    }

    /// Create DashMap for high-concurrency workloads
    pub fn create_dashmap_concurrent<K, V>(shard_count: usize) -> ConcurrentHashMap<K, V>
    where
        K: Clone + Ord + std::hash::Hash + Send + Sync + std::fmt::Debug,
        V: Clone + Send + Sync + std::fmt::Debug,
    {
        crate::storage::memtable::implementations::dashmap::DashMapMemtable::with_shard_count(
            shard_count,
        )
    }

    /// Create ART for memory-constrained environments
    pub fn create_art_memory_efficient<K, V>() -> MemoryEfficientMap<K, V>
    where
        K: Clone + Ord + std::hash::Hash + Send + Sync + std::fmt::Debug + AsRef<[u8]>,
        V: Clone + Send + Sync + std::fmt::Debug,
    {
        crate::storage::memtable::implementations::btree::BTreeMemtable::new(false)
    }

    /// Create HashMap for point lookup workloads
    pub fn create_hashmap_fast_lookup<K, V>(capacity: Option<usize>) -> FastLookupMap<K, V>
    where
        K: Clone + Ord + std::hash::Hash + Send + Sync + std::fmt::Debug,
        V: Clone + Send + Sync + std::fmt::Debug,
    {
        match capacity {
            Some(cap) => {
                crate::storage::memtable::implementations::hashmap::HashMapMemtable::with_capacity(
                    cap,
                )
            }
            None => crate::storage::memtable::implementations::hashmap::HashMapMemtable::new(),
        }
    }
}
