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

pub mod wal_behavior;


use crate::storage::memtable::core::MemtableConfig;

/// Factory methods that use actual data structure names
pub struct SpecializedMemtableFactory;

impl SpecializedMemtableFactory {
    /// Create global partitioned memtable with WAL-specific behavior
    pub fn create_global_partitioned_for_wal(config: MemtableConfig) -> wal_behavior::WALBehaviorWrapper {
        wal_behavior::WALBehaviorWrapper::new(config)
    }
}
