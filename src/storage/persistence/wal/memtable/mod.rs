// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Pluggable Memtable Structures for WAL

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;

use super::{WalConfig, WalEntry};
use crate::core::{CollectionId, VectorId};

// Memtable implementations
pub mod art; // Adaptive Radix Tree
pub mod btree;
pub mod hashmap;
pub mod skiplist;

// Re-exports
pub use art::ArtMemTable;
pub use btree::BTreeMemTable;
pub use hashmap::HashMapMemTable;
pub use skiplist::SkipListMemTable;

/// Memtable strategy type selection
#[derive(Debug, Clone, PartialEq)]
pub enum MemTableType {
    /// Skip List - High write throughput, ordered data (RocksDB/LevelDB default)
    SkipList,
    /// B+ Tree - Stable inserts/queries, general use, memory efficient
    BTree,
    /// ART - Concurrent Adaptive Radix Tree, high performance for range queries
    Art,
    /// Hash Map - Write-heavy, unordered ingestion, point lookups only
    HashMap,
}

impl Default for MemTableType {
    fn default() -> Self {
        // Skip List is the proven default for LSM-based systems
        Self::SkipList
    }
}

/// Statistics for memtable monitoring
#[derive(Debug, Clone)]
pub struct MemTableStats {
    pub total_entries: u64,
    pub memory_bytes: usize,
    pub lookup_performance_ms: f64,
    pub insert_performance_ms: f64,
    pub range_scan_performance_ms: f64,
}

/// Collection-specific statistics
#[derive(Debug, Clone, Default)]
pub struct CollectionStats {
    pub entry_count: u64,
    pub memory_usage_bytes: usize,
}

/// Maintenance statistics for background operations
#[derive(Debug, Clone, Default)]
pub struct MemTableMaintenanceStats {
    pub mvcc_versions_cleaned: u64,
    pub ttl_entries_expired: u64,
    pub memory_compacted_bytes: usize,
}

/// Generic memtable trait for pluggable implementations
#[async_trait]
pub trait MemTableStrategy: Send + Sync + std::fmt::Debug {
    /// Strategy name for identification
    fn strategy_name(&self) -> &'static str;

    /// Initialize the memtable with configuration
    async fn initialize(&mut self, config: &WalConfig) -> Result<()>;

    /// Insert a single entry and return sequence number
    async fn insert_entry(&self, entry: WalEntry) -> Result<u64>;

    /// Insert multiple entries in batch for optimization
    async fn insert_batch(&self, entries: Vec<WalEntry>) -> Result<Vec<u64>>;

    /// Get latest entry for a vector ID (MVCC support)
    async fn get_latest_entry(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<WalEntry>>;

    /// Get all entries for a vector ID (MVCC history)
    async fn get_entry_history(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Vec<WalEntry>>;

    /// Get entries from a sequence number (range scan)
    async fn get_entries_from(
        &self,
        collection_id: &CollectionId,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<WalEntry>>;

    /// Get all entries for a collection (for flushing)
    async fn get_all_entries(&self, collection_id: &CollectionId) -> Result<Vec<WalEntry>>;

    /// Search for specific vector entry (optimized lookup)
    async fn search_vector(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<WalEntry>>;

    /// Clear entries up to sequence after flush
    async fn clear_flushed(&self, collection_id: &CollectionId, up_to_sequence: u64) -> Result<()>;

    /// Drop entire collection from memtable
    async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()>;

    /// Check which collections need flushing
    async fn collections_needing_flush(&self) -> Result<Vec<CollectionId>>;

    /// Check if global flush is needed
    async fn needs_global_flush(&self) -> Result<bool>;

    /// Get performance and memory statistics
    async fn get_stats(&self) -> Result<HashMap<CollectionId, MemTableStats>>;

    /// Get collection-specific statistics
    async fn get_collection_stats(&self, collection_id: &CollectionId) -> Result<CollectionStats>;

    /// Perform maintenance (MVCC cleanup, TTL expiration)
    async fn maintenance(&self) -> Result<MemTableMaintenanceStats>;

    /// Close and cleanup resources
    async fn close(&self) -> Result<()>;

    /// Downcast support for trait objects
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Factory for creating memtable strategies
pub struct MemTableFactory;

impl MemTableFactory {
    /// Create memtable strategy based on type and configuration
    pub async fn create_strategy(
        memtable_type: MemTableType,
        config: &WalConfig,
    ) -> Result<Box<dyn MemTableStrategy>> {
        let mut strategy: Box<dyn MemTableStrategy> = match memtable_type {
            MemTableType::SkipList => Box::new(SkipListMemTable::new()),
            MemTableType::BTree => Box::new(BTreeMemTable::new()),
            MemTableType::Art => Box::new(ArtMemTable::new()),
            MemTableType::HashMap => Box::new(HashMapMemTable::new()),
        };

        strategy.initialize(config).await?;
        Ok(strategy)
    }

    /// Create strategy from configuration
    pub async fn create_from_config(config: &WalConfig) -> Result<Box<dyn MemTableStrategy>> {
        let memtable_type = match config.memtable.memtable_type {
            crate::storage::persistence::wal::config::MemTableType::SkipList => MemTableType::SkipList,
            crate::storage::persistence::wal::config::MemTableType::BTree => MemTableType::BTree,
            crate::storage::persistence::wal::config::MemTableType::Art => MemTableType::Art,
            crate::storage::persistence::wal::config::MemTableType::HashMap => MemTableType::HashMap,
        };
        Self::create_strategy(memtable_type, config).await
    }

    /// Get available memtable types
    pub fn available_types() -> Vec<MemTableType> {
        vec![
            MemTableType::SkipList,
            MemTableType::BTree,
            MemTableType::Art,
            MemTableType::HashMap,
        ]
    }

    /// Get performance characteristics for selection
    pub fn get_characteristics(memtable_type: &MemTableType) -> MemTableCharacteristics {
        match memtable_type {
            MemTableType::SkipList => MemTableCharacteristics {
                name: "Skip List",
                description: "Probabilistic balanced tree, excellent for write-heavy workloads",
                write_performance: PerformanceRating::Excellent,
                read_performance: PerformanceRating::Good,
                range_scan_performance: PerformanceRating::Excellent,
                memory_efficiency: PerformanceRating::Good,
                concurrency: PerformanceRating::Excellent,
                ordered: true,
                best_for: &[
                    "High write throughput",
                    "LSM-style storage",
                    "Mixed workloads",
                ],
            },
            MemTableType::BTree => MemTableCharacteristics {
                name: "B+ Tree",
                description: "Classic balanced tree, stable performance across operations",
                write_performance: PerformanceRating::Good,
                read_performance: PerformanceRating::Excellent,
                range_scan_performance: PerformanceRating::Excellent,
                memory_efficiency: PerformanceRating::Excellent,
                concurrency: PerformanceRating::Good,
                ordered: true,
                best_for: &["Range queries", "Memory efficiency", "Read-heavy workloads"],
            },
            MemTableType::Art => MemTableCharacteristics {
                name: "Adaptive Radix Tree",
                description: "Space-efficient radix tree with adaptive node types",
                write_performance: PerformanceRating::Good,
                read_performance: PerformanceRating::Excellent,
                range_scan_performance: PerformanceRating::Excellent,
                memory_efficiency: PerformanceRating::Excellent,
                concurrency: PerformanceRating::Excellent,
                ordered: true,
                best_for: &["String keys", "Memory efficiency", "High concurrency"],
            },
            MemTableType::HashMap => MemTableCharacteristics {
                name: "Hash Map",
                description: "Hash-based unordered map, optimized for point lookups",
                write_performance: PerformanceRating::Excellent,
                read_performance: PerformanceRating::Excellent,
                range_scan_performance: PerformanceRating::Poor,
                memory_efficiency: PerformanceRating::Good,
                concurrency: PerformanceRating::Good,
                ordered: false,
                best_for: &["Point lookups", "Unordered ingestion", "Write-heavy loads"],
            },
        }
    }
}

/// Performance characteristics for memtable selection
#[derive(Debug, Clone)]
pub struct MemTableCharacteristics {
    pub name: &'static str,
    pub description: &'static str,
    pub write_performance: PerformanceRating,
    pub read_performance: PerformanceRating,
    pub range_scan_performance: PerformanceRating,
    pub memory_efficiency: PerformanceRating,
    pub concurrency: PerformanceRating,
    pub ordered: bool,
    pub best_for: &'static [&'static str],
}

/// Performance rating scale
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PerformanceRating {
    Poor,
    Fair,
    Good,
    Excellent,
}

/// Workload-based memtable selector
pub struct MemTableSelector;

impl MemTableSelector {
    /// Recommend memtable type based on workload characteristics
    pub fn recommend_memtable(workload: &WorkloadCharacteristics) -> MemTableType {
        match workload {
            // Write-heavy unordered workloads
            WorkloadCharacteristics {
                write_heavy: true,
                ordered_access: false,
                range_queries: false,
                ..
            } => MemTableType::HashMap,

            // Memory-constrained with range queries
            WorkloadCharacteristics {
                memory_constrained: true,
                range_queries: true,
                ..
            } => MemTableType::BTree,

            // High concurrency with string-like keys
            WorkloadCharacteristics {
                high_concurrency: true,
                string_keys: true,
                ..
            } => MemTableType::Art,

            // Default: balanced write-heavy workload (LSM style)
            _ => MemTableType::SkipList,
        }
    }
}

/// Workload characteristics for memtable selection
#[derive(Debug, Clone)]
pub struct WorkloadCharacteristics {
    pub write_heavy: bool,
    pub read_heavy: bool,
    pub range_queries: bool,
    pub ordered_access: bool,
    pub memory_constrained: bool,
    pub high_concurrency: bool,
    pub string_keys: bool,
    pub point_lookups_only: bool,
}

impl Default for WorkloadCharacteristics {
    fn default() -> Self {
        Self {
            write_heavy: true, // Default for WAL workloads
            read_heavy: false,
            range_queries: true, // Common for vector databases
            ordered_access: true,
            memory_constrained: false,
            high_concurrency: true,
            string_keys: false,
            point_lookups_only: false,
        }
    }
}

/// Unified WAL MemTable that wraps a MemTableStrategy
/// This provides a simplified interface for WAL strategies to use
#[derive(Debug)]
pub struct WalMemTable {
    strategy: Box<dyn MemTableStrategy>,
}

impl WalMemTable {
    /// Create new WalMemTable with specified configuration
    pub async fn new(config: WalConfig) -> Result<Self> {
        let strategy = MemTableFactory::create_from_config(&config).await?;
        Ok(Self { strategy })
    }

    /// Insert a single entry and return sequence number
    pub async fn insert_entry(&self, entry: WalEntry) -> Result<u64> {
        self.strategy.insert_entry(entry).await
    }

    /// Insert multiple entries in batch
    pub async fn insert_batch(&self, entries: Vec<WalEntry>) -> Result<Vec<u64>> {
        self.strategy.insert_batch(entries).await
    }

    /// Get entries for a collection
    pub async fn get_entries(
        &self,
        collection_id: &CollectionId,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<WalEntry>> {
        self.strategy
            .get_entries_from(collection_id, from_sequence, limit)
            .await
    }

    /// Get all entries for a collection
    pub async fn get_all_entries(&self, collection_id: &CollectionId) -> Result<Vec<WalEntry>> {
        self.strategy.get_all_entries(collection_id).await
    }

    /// Search for specific vector
    pub async fn search_vector(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<WalEntry>> {
        self.strategy.search_vector(collection_id, vector_id).await
    }

    /// Clear flushed entries
    pub async fn clear_flushed(
        &self,
        collection_id: &CollectionId,
        up_to_sequence: u64,
    ) -> Result<()> {
        self.strategy
            .clear_flushed(collection_id, up_to_sequence)
            .await
    }

    /// Drop collection
    pub async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()> {
        self.strategy.drop_collection(collection_id).await
    }

    /// Get collections needing flush
    pub async fn collections_needing_flush(&self) -> Result<Vec<CollectionId>> {
        self.strategy.collections_needing_flush().await
    }

    /// Check if global flush needed
    pub async fn needs_global_flush(&self) -> Result<bool> {
        self.strategy.needs_global_flush().await
    }

    /// Get statistics
    pub async fn get_stats(&self) -> Result<HashMap<CollectionId, MemTableStats>> {
        self.strategy.get_stats().await
    }

    /// Get collection-specific statistics
    pub async fn get_collection_stats(&self, collection_id: &CollectionId) -> Result<CollectionStats> {
        self.strategy.get_collection_stats(collection_id).await
    }

    /// Perform maintenance
    pub async fn maintenance(&self) -> Result<MemTableMaintenanceStats> {
        self.strategy.maintenance().await
    }
}

/// Clone implementation for Arc-wrapped WalMemTable
impl Clone for WalMemTable {
    fn clone(&self) -> Self {
        panic!("WalMemTable should be wrapped in Arc for sharing across tasks")
    }
}
