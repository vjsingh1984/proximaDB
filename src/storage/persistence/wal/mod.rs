// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Comprehensive Write-Ahead Log System with Strategy Pattern
//!
//! This module provides a high-performance WAL system supporting:
//! - Multiple serialization strategies (Avro with schema evolution, Bincode for speed)
//! - Memory + Disk organization by collection
//! - Atomic operations with MVCC and TTL support
//! - Multi-disk support for sequential I/O optimization
//! - Configurable compression and smart defaults
//! - Batch operations for optimal performance

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::debug;

use crate::compute::distance::DistanceMetric as CoreDistanceMetric;
use crate::compute::unified_distance::{DistanceComputeProvider, UnifiedDistanceCompute};
use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::services::collection_service::CollectionService;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::UnifiedStorageEngine;

// Sub-modules
pub mod avro;
pub mod background_manager;
pub mod bincode;
pub mod config;
pub mod disk;
pub mod factory;
pub mod flush_coordinator;
// pub mod memtable;  // Moved to obsolete - using new unified memtable system
pub mod schema;

// Unit tests
#[cfg(test)]
mod tests;

// Re-exports
pub use background_manager::{
    BackgroundMaintenanceManager, BackgroundMaintenanceStats, BackgroundTaskStatus,
};
pub use config::WalStrategyType;
pub use config::{CompressionConfig, PerformanceConfig, WalConfig};
pub use disk::WalDiskManager;
pub use factory::WalFactory;
pub use flush_coordinator::{
    CleanupInstructions, FlushCoordinatorCallbacks, FlushDataSource, FlushState, PendingFlush,
    WalFlushCoordinator,
};
// pub use memtable::WalMemTable;  // Moved to obsolete - using new unified memtable system

/// WAL operation types - simplified to zero-copy Avro payloads only
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WalOperation {
    /// Legacy insert operation (being phased out in favor of AvroPayload)
    Insert {
        vector_id: VectorId,
        record: VectorRecord,
    },
    /// Soft delete with TTL - uses typed data for precise control
    Delete {
        vector_id: VectorId,
        expires_at: Option<DateTime<Utc>>, // Soft delete with TTL
    },
    /// Flush operation
    Flush,
    /// Checkpoint operation
    Checkpoint,
    /// Binary Avro payload operation (zero-copy) - handles upsert/batch operations
    AvroPayload {
        operation_type: String, // "upsert", "delete_batch", etc.
        avro_data: Vec<u8>,
    },
}

/// Batch coordination identifier for WAL disk ↔ Memtable mapping
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BatchId {
    /// Collection ID this batch belongs to
    pub collection_id: String,
    /// Sequence range this batch covers
    pub sequence_range: (u64, u64), // (start_seq, end_seq)
    /// Unique batch identifier
    pub batch_uuid: String,
    /// Timestamp when batch was created
    pub created_at: DateTime<Utc>,
}

impl BatchId {
    /// Create new batch ID
    pub fn new(collection_id: String, start_seq: u64, end_seq: u64) -> Self {
        Self {
            collection_id,
            sequence_range: (start_seq, end_seq),
            batch_uuid: uuid::Uuid::new_v4().to_string(),
            created_at: Utc::now(),
        }
    }

    /// Check if sequence is within this batch range
    pub fn contains_sequence(&self, sequence: u64) -> bool {
        sequence >= self.sequence_range.0 && sequence <= self.sequence_range.1
    }

    /// Get batch size (number of sequences)
    pub fn batch_size(&self) -> u64 {
        self.sequence_range.1 - self.sequence_range.0 + 1
    }
}

/// WAL entry with MVCC versioning and batch coordination
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    /// Vector ID (client-provided or system-generated)
    pub entry_id: String,

    /// Collection this entry belongs to
    pub collection_id: CollectionId,

    /// Operation being logged
    pub operation: WalOperation,

    /// Entry timestamp
    pub timestamp: DateTime<Utc>,

    /// Sequence number for ordering (per collection)
    pub sequence: u64,

    /// Global sequence number across all collections
    pub global_sequence: u64,

    /// Entry expires at (for TTL and soft deletes)
    pub expires_at: Option<DateTime<Utc>>,

    /// Entry version for MVCC
    pub version: u64,

    /// Batch coordination ID for disk ↔ memtable mapping
    pub batch_id: Option<BatchId>,
}

impl WalEntry {
    /// Generate deterministic content-based key from vector data
    ///
    /// Uses full precision f32 values to ensure identical vectors have same key
    /// while different vectors (even slightly different) get unique keys
    pub fn content_key_from_vector(vector: &[f32]) -> String {
        let mut hasher = blake3::Hasher::new();

        // Hash vector data with full f32 precision (no rounding)
        // This ensures identical vectors get same key, different vectors get different keys
        for &value in vector {
            hasher.update(&value.to_le_bytes());
        }

        // Use 16 hex chars (64 bits) for strong collision resistance
        let hex_str = hasher.finalize().to_hex();
        format!("vec_{}", &hex_str[..16])
    }

    /// Extract vector data from WAL operation and generate content key
    pub fn content_key(&self) -> Result<String, anyhow::Error> {
        match &self.operation {
            WalOperation::Insert { record, .. } => {
                Ok(Self::content_key_from_vector(&record.vector))
            }
            WalOperation::Update { record, .. } => {
                Ok(Self::content_key_from_vector(&record.vector))
            }
            WalOperation::AvroPayload { avro_data, .. } => {
                // For Avro payload, we need to extract vector from binary data
                // For now, use a hash of the entire payload as key
                let mut hasher = blake3::Hasher::new();
                hasher.update(avro_data);
                let hex_str = hasher.finalize().to_hex();
                Ok(format!("avro_{}", &hex_str[..16]))
            }
            WalOperation::Delete { vector_id, .. } => {
                // For deletes, use the vector_id directly since we don't have vector content
                Ok(format!("del_{}", vector_id))
            }
            WalOperation::Flush => Ok("flush_op".to_string()),
            WalOperation::Checkpoint => Ok("checkpoint_op".to_string()),
        }
    }

    /// Calculate the actual memory size of this WAL entry including vector data
    pub fn actual_size_bytes(&self) -> usize {
        let mut size = 0;

        // Fixed fields
        size += std::mem::size_of::<u64>() * 3; // sequence, global_sequence, version
        size += std::mem::size_of::<DateTime<Utc>>(); // timestamp
        size += std::mem::size_of::<Option<DateTime<Utc>>>(); // expires_at

        // Variable size fields
        size += self.entry_id.len();
        size += self.collection_id.len(); // CollectionId is a String

        // Operation size (this is the critical part that includes vector data)
        size += self.operation.actual_size_bytes();

        // Add some overhead for struct padding and heap allocations
        size += 64; // Conservative overhead estimate

        size
    }
}

impl WalOperation {
    /// Calculate the actual memory size of this WAL operation including vector data
    pub fn actual_size_bytes(&self) -> usize {
        let mut size = std::mem::size_of::<u8>(); // Enum discriminant

        match self {
            WalOperation::Insert {
                vector_id,
                record,
                expires_at,
            } => {
                size += vector_id.len();
                size += record.actual_size_bytes(); // This includes the vector data!
                size += std::mem::size_of::<Option<DateTime<Utc>>>();
            }
            WalOperation::Update {
                vector_id,
                record,
                expires_at,
            } => {
                size += vector_id.len();
                size += record.actual_size_bytes(); // This includes the vector data!
                size += std::mem::size_of::<Option<DateTime<Utc>>>();
            }
            WalOperation::Delete {
                vector_id,
                expires_at,
            } => {
                size += vector_id.len();
                size += std::mem::size_of::<Option<DateTime<Utc>>>();
            }
            WalOperation::Flush | WalOperation::Checkpoint => {
                // No additional data
            }
            WalOperation::AvroPayload {
                operation_type,
                avro_data,
            } => {
                size += operation_type.len();
                size += avro_data.len(); // Actual byte data size
            }
        }

        size
    }
}

/// WAL statistics for monitoring
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WalStats {
    pub total_entries: u64,
    pub memory_entries: u64,
    pub disk_segments: u64,
    pub total_disk_size_bytes: u64,
    pub memory_size_bytes: u64,
    pub collections_count: usize,
    pub last_flush_time: Option<DateTime<Utc>>,
    pub write_throughput_entries_per_sec: f64,
    pub read_throughput_entries_per_sec: f64,
    pub compression_ratio: f64,
}

/// WAL flush result
#[derive(Debug, Clone)]
pub struct FlushResult {
    pub entries_flushed: u64,
    pub bytes_written: u64,
    pub segments_created: u64,
    pub collections_affected: Vec<CollectionId>,
    pub flush_duration_ms: u64,
}

/// Atomic flush cycle for consistent WAL→Storage operations
#[derive(Debug, Clone)]
pub struct FlushCycle {
    /// Unique identifier for this flush operation
    pub flush_id: String,
    /// Collection being flushed
    pub collection_id: CollectionId,
    /// WAL entries marked for flush
    pub entries: Vec<WalEntry>,
    /// Extracted vector records ready for storage
    pub vector_records: Vec<VectorRecord>,
    /// Disk segments marked as flush-pending
    pub marked_segments: Vec<String>,
    /// Sequence ranges marked as flush-pending
    pub marked_sequences: Vec<(u64, u64)>, // (start_seq, end_seq) pairs
    /// Current state of the flush cycle
    pub state: FlushCycleState,
}

/// State of a flush cycle operation
#[derive(Debug, Clone, PartialEq)]
pub enum FlushCycleState {
    /// Flush cycle is active and entries are marked
    Active,
    /// Flush cycle completed successfully
    Completed,
    /// Flush cycle was aborted and entries restored
    Aborted,
}

/// Result of completing a flush cycle
#[derive(Debug, Clone)]
pub struct FlushCompletionResult {
    /// Number of entries permanently removed
    pub entries_removed: usize,
    /// Number of disk segments cleaned up
    pub segments_cleaned: usize,
    /// Bytes reclaimed from cleanup
    pub bytes_reclaimed: u64,
}

// Distance computation functions moved to unified distance system
// These functions are now available through UnifiedDistanceCompute

/// Main WAL strategy trait
#[async_trait]
pub trait WalStrategy: Send + Sync + DistanceComputeProvider {
    /// Strategy name for identification
    fn strategy_name(&self) -> &'static str;

    /// Initialize the strategy with configuration and optional storage engine
    async fn initialize(
        &mut self,
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<()>;

    /// Set storage engine for delegated flush/compaction operations
    fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>);

    /// Serialize entries to bytes (strategy-specific format)
    async fn serialize_entries(&self, entries: &[WalEntry]) -> Result<Vec<u8>>;

    /// Deserialize entries from bytes (strategy-specific format)
    async fn deserialize_entries(&self, data: &[u8]) -> Result<Vec<WalEntry>>;

    /// Write single entry atomically (memory + disk)
    async fn write_entry(&self, entry: WalEntry) -> Result<u64>;

    /// Write batch of entries atomically (default implementation using single writes)
    async fn write_batch(&self, entries: Vec<WalEntry>) -> Result<Vec<u64>> {
        let mut sequences = Vec::with_capacity(entries.len());
        for entry in entries {
            sequences.push(self.write_entry(entry).await?);
        }
        Ok(sequences)
    }

    /// Write batch of entries with immediate disk sync for durability
    async fn write_batch_with_sync(
        &self,
        entries: Vec<WalEntry>,
        immediate_sync: bool,
    ) -> Result<Vec<u64>> {
        // Default implementation falls back to regular write_batch
        // Individual strategies can override for optimized immediate sync
        self.write_batch(entries).await
    }

    /// Force immediate sync of in-memory data to disk
    async fn force_sync(&self, collection_id: Option<&CollectionId>) -> Result<()> {
        // Default implementation performs a flush
        self.flush(collection_id).await.map(|_| ())
    }

    /// Read entries for a collection starting from sequence
    async fn read_entries(
        &self,
        collection_id: &CollectionId,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<WalEntry>>;

    /// Search entries by vector ID (checks both memory and disk)
    async fn search_by_vector_id(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<WalEntry>>;

    /// Similarity search for unflushed vectors in WAL with configurable distance metric
    async fn search_vectors_similarity(
        &self,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
        distance_metric: Option<CoreDistanceMetric>,
    ) -> Result<Vec<(VectorId, f32, WalEntry)>> {
        tracing::info!(
            "🔍 WAL_SEARCH: Starting similarity search for collection '{}', k={}",
            collection_id,
            k
        );

        // Resolve distance metric using unified distance system
        let resolved_metric = self.resolve_metric(distance_metric, collection_id).await;
        tracing::debug!(
            "🔍 WAL_SEARCH: Using distance metric: {:?}",
            resolved_metric
        );

        if let Some(memtable) = self.memtable() {
            // Use the specialized memtable's search method which properly handles distance metrics
            let wal_results = memtable
                .search_unflushed_vectors(query_vector, k, collection_id, resolved_metric)
                .await?;

            // Convert from memtable format (f32, WalEntry) to trait format (VectorId, f32, WalEntry)
            let mut results = Vec::new();
            for (score, entry) in wal_results {
                // Extract vector ID from the WAL entry
                let vector_id = match &entry.operation {
                    WalOperation::Insert { vector_id, .. }
                    | WalOperation::Update { vector_id, .. } => vector_id.clone(),
                    WalOperation::AvroPayload { .. } => {
                        // For Avro payloads, try to extract the ID from the entry_id or deserialized content
                        entry.entry_id.clone()
                    }
                    _ => {
                        // For other operations, use the entry_id
                        entry.entry_id.clone()
                    }
                };

                results.push((vector_id, score, entry));
            }

            tracing::info!(
                "🔍 WAL_SEARCH: Memtable search returned {} results for collection '{}'",
                results.len(),
                collection_id
            );
            for (i, (vector_id, score, _)) in results.iter().enumerate() {
                tracing::info!(
                    "🔍 WAL_SEARCH: Result {}: {} (score: {:.4})",
                    i + 1,
                    vector_id,
                    score
                );
            }

            Ok(results)
        } else {
            tracing::warn!("🔍 WAL_SEARCH: No memtable available for similarity search");
            Ok(Vec::new())
        }
    }

    /// Get memtable reference for similarity search - using new unified memtable system
    fn memtable(&self) -> Option<&crate::storage::memtable::specialized::WalMemtable>;

    /// Get latest entry for a vector (for MVCC)
    async fn get_latest_entry(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<WalEntry>>;

    /// Get all entries for a collection from memtable (for similarity search)
    async fn get_collection_entries(&self, collection_id: &CollectionId) -> Result<Vec<WalEntry>>;

    /// Flush memory entries to disk - delegates to storage engine if available
    async fn flush(&self, collection_id: Option<&CollectionId>) -> Result<FlushResult>;

    /// Delegate flush to storage engine (common implementation for all strategies)
    async fn delegate_to_storage_engine_flush(
        &self,
        _collection_id: &CollectionId,
    ) -> Result<crate::storage::traits::FlushResult> {
        // Default implementation - override in implementing strategies
        Err(anyhow::anyhow!(
            "Storage engine not available for flush delegation"
        ))
    }

    /// Delegate compaction to storage engine (common implementation for all strategies)  
    async fn delegate_to_storage_engine_compact(
        &self,
        _collection_id: &CollectionId,
    ) -> Result<crate::storage::traits::CompactionResult> {
        // Default implementation - override in implementing strategies
        Err(anyhow::anyhow!(
            "Storage engine not available for compaction delegation"
        ))
    }

    /// Compact entries for a collection (MVCC cleanup)
    async fn compact_collection(&self, collection_id: &CollectionId) -> Result<u64>;

    /// Drop all entries for a collection
    async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()>;

    /// Get WAL statistics
    async fn get_stats(&self) -> Result<WalStats>;

    /// Atomically retrieve and mark WAL entries for flush operation
    ///
    /// This method provides atomic access to WAL entries that need to be flushed to storage.
    /// It ensures no gaps or duplicates by:
    /// 1. Atomically retrieving entries from memtable and disk segments
    /// 2. Marking them as "flush-pending" to prevent concurrent modifications
    /// 3. Returning entries in sequence order for consistent flush
    ///
    /// The caller MUST call `complete_flush_cycle()` after successful storage write
    /// to permanently remove the entries, or `abort_flush_cycle()` on failure
    /// to restore them for retry.
    ///
    /// **Memtable-Specific Optimizations:**
    /// - **BTree**: Provides ordered retrieval for consistent flush order
    /// - **HashMap**: O(1) retrieval per entry, best for large collections
    /// - **SkipList**: Probabilistic ordering with good concurrency
    /// - **ART**: Space-efficient retrieval, ideal for sparse collections
    async fn atomic_retrieve_for_flush(
        &self,
        collection_id: &CollectionId,
        flush_id: &str,
    ) -> Result<FlushCycle> {
        // Default implementation - get all entries for the collection
        let entries = self.get_collection_entries(collection_id).await?;

        // Extract VectorRecord data from WAL entries
        let mut vector_records = Vec::new();
        let mut marked_sequences = Vec::new();

        if !entries.is_empty() {
            let min_seq = entries.iter().map(|e| e.sequence).min().unwrap_or(0);
            let max_seq = entries.iter().map(|e| e.sequence).max().unwrap_or(0);
            marked_sequences.push((min_seq, max_seq));

            // Extract vector records from Insert/Update operations
            for entry in &entries {
                match &entry.operation {
                    WalOperation::Insert {
                        vector_id: _,
                        record,
                        expires_at: _,
                    } => {
                        vector_records.push(record.clone());
                    }
                    WalOperation::Update {
                        vector_id: _,
                        record,
                        expires_at: _,
                    } => {
                        vector_records.push(record.clone());
                    }
                    _ => {
                        // Skip other operation types but include in flush cycle
                    }
                }
            }
        }

        tracing::info!("🔄 {}: Default atomic flush retrieval - {} entries, {} vector records (collection: {}, flush_id: {})",
                      self.strategy_name(), entries.len(), vector_records.len(), collection_id, flush_id);

        Ok(FlushCycle {
            flush_id: flush_id.to_string(),
            collection_id: collection_id.clone(),
            entries,
            vector_records,
            marked_segments: Vec::new(), // Default: no disk segments
            marked_sequences,
            state: FlushCycleState::Active,
        })
    }

    /// Complete flush cycle - permanently remove flushed entries
    ///
    /// This method is called after successful storage engine flush to:
    /// 1. Remove entries from memtable and disk segments
    /// 2. Update WAL sequence tracking
    /// 3. Trigger cleanup of obsolete segments
    ///
    /// **Memtable-Specific Optimizations:**
    /// - **BTree**: Batch removal in sequence order for optimal B+ tree performance
    /// - **HashMap**: O(1) removal per entry, fastest for large flush cycles
    /// - **SkipList**: Maintains ordering during removal, good for concurrent access
    /// - **ART**: Space-efficient removal with prefix compression maintenance
    async fn complete_flush_cycle(&self, flush_cycle: FlushCycle) -> Result<FlushCompletionResult> {
        // Default implementation - remove all entries for the collection
        // Individual strategies can override for optimized removal
        tracing::info!(
            "✅ {}: Default flush completion - {} entries for collection {} (flush_id: {})",
            self.strategy_name(),
            flush_cycle.entries.len(),
            flush_cycle.collection_id,
            flush_cycle.flush_id
        );

        // Drop collection data (simplified default)
        self.drop_collection(&flush_cycle.collection_id).await?;

        Ok(FlushCompletionResult {
            entries_removed: flush_cycle.entries.len(),
            segments_cleaned: 0,
            bytes_reclaimed: flush_cycle
                .entries
                .iter()
                .map(|entry| std::mem::size_of_val(entry))
                .sum::<usize>() as u64,
        })
    }

    /// Abort flush cycle - restore entries for retry
    ///
    /// This method is called when storage engine flush fails to:
    /// 1. Restore entries to active state in memtable
    /// 2. Clear "flush-pending" marks
    /// 3. Allow retry of flush operation
    ///
    /// **Memtable-Specific Optimizations:**
    /// - **BTree**: Ordered restoration maintains B+ tree structure
    /// - **HashMap**: O(1) restoration per entry, fastest recovery
    /// - **SkipList**: Probabilistic restoration with good concurrent access
    /// - **ART**: Space-efficient restoration preserving prefix compression
    async fn abort_flush_cycle(&self, flush_cycle: FlushCycle, reason: &str) -> Result<()> {
        // Default implementation - log the abort
        // Individual strategies can override for advanced restoration logic
        tracing::warn!("❌ {}: Default flush abort - {} entries restored for collection {} (flush_id: {}, reason: {})", 
                      self.strategy_name(), flush_cycle.entries.len(), flush_cycle.collection_id, flush_cycle.flush_id, reason);

        // In the default implementation, entries are never actually marked as pending,
        // so there's nothing to restore. Advanced strategies can implement proper restoration.
        Ok(())
    }

    /// Assignment Service Integration (Base Implementation for All WAL Strategies)

    /// Get the assignment service used by this WAL strategy
    fn get_assignment_service(
        &self,
    ) -> &Arc<dyn crate::storage::assignment_service::AssignmentService>;

    /// Select WAL directory URL for a collection using assignment service
    /// This method provides consistent assignment logic across all WAL implementations
    async fn select_wal_url_for_collection(
        &self,
        collection_id: &str,
        config: &WalConfig,
    ) -> Result<String> {
        use crate::storage::assignment_service::{StorageAssignmentConfig, StorageComponentType};

        // Check if collection already has an assignment
        if let Some(assignment) = self
            .get_assignment_service()
            .get_assignment(
                &CollectionId::from(collection_id.to_string()),
                StorageComponentType::Wal,
            )
            .await
        {
            return Ok(assignment.storage_url);
        }

        // Create new assignment using service
        let assignment_config = StorageAssignmentConfig {
            storage_urls: config.multi_disk.data_directories.clone(),
            component_type: StorageComponentType::Wal,
            collection_affinity: config.multi_disk.collection_affinity,
        };

        let assignment_result = self
            .get_assignment_service()
            .assign_storage_url(
                &CollectionId::from(collection_id.to_string()),
                &assignment_config,
            )
            .await?;

        tracing::info!(
            "📍 Assigned collection '{}' to WAL directory '{}'",
            collection_id,
            assignment_result.storage_url
        );

        Ok(assignment_result.storage_url)
    }

    /// Discover existing collections from all configured WAL directories
    /// This method provides consistent discovery logic across all WAL implementations
    async fn discover_existing_assignments(
        &self,
        config: &WalConfig,
        filesystem: &Arc<FilesystemFactory>,
    ) -> Result<usize> {
        use crate::storage::assignment_service::{AssignmentDiscovery, StorageComponentType};

        AssignmentDiscovery::discover_and_record_assignments(
            StorageComponentType::Wal,
            &config.multi_disk.data_directories,
            filesystem,
            self.get_assignment_service(),
        )
        .await
    }

    /// Recover from disk on startup
    async fn recover(&self) -> Result<u64>;

    /// Close and cleanup resources
    async fn close(&self) -> Result<()>;

    /// Force flush all collections - FOR TESTING ONLY
    /// WARNING: This method should only be used for testing and debugging
    async fn force_flush_all(&self) -> Result<()> {
        tracing::warn!(
            "⚠️ {}: FORCE FLUSH ALL - TESTING ONLY",
            self.strategy_name()
        );
        // Default implementation - trigger flush for all known collections
        // Individual strategies can override with more efficient bulk operations
        // For now, just return success as a placeholder
        Ok(())
    }

    /// Force flush specific collection - FOR TESTING ONLY
    /// WARNING: This method should only be used for testing and debugging
    async fn force_flush_collection(
        &self,
        collection_id: &str,
        storage_engine: Option<&str>,
    ) -> Result<()> {
        tracing::warn!(
            "⚠️ {}: FORCE FLUSH COLLECTION {} - TESTING ONLY",
            self.strategy_name(),
            collection_id
        );
        // Default implementation - trigger immediate flush for this collection
        // Individual strategies can override with collection-specific logic
        // For now, just return success as a placeholder
        Ok(())
    }

    /// Register storage engine with the strategy's flush coordinator
    async fn register_storage_engine(
        &self,
        engine_name: &str,
        engine: Arc<dyn UnifiedStorageEngine>,
    ) -> Result<()> {
        tracing::warn!(
            "⚠️ {}: register_storage_engine not implemented for {}",
            self.strategy_name(),
            engine_name
        );
        Ok(())
    }
}

/// High-level WAL manager that uses strategies
pub struct WalManager {
    strategy: Box<dyn WalStrategy>,
    config: WalConfig,
    stats: Arc<tokio::sync::RwLock<WalStats>>,
    atomicity_manager: Option<Arc<crate::storage::atomicity::AtomicityManager>>,
    distance_compute: UnifiedDistanceCompute,
}

impl WalManager {
    /// Create new WAL manager with specified strategy
    pub async fn new(strategy: Box<dyn WalStrategy>, config: WalConfig) -> Result<Self> {
        tracing::debug!(
            "🚀 Creating WalManager with strategy: {}",
            strategy.strategy_name()
        );
        tracing::debug!(
            "📋 WAL Config: strategy_type={:?}, memtable_type={:?}",
            config.strategy_type,
            config.memtable.memtable_type
        );
        tracing::debug!(
            "💾 Multi-disk config: {} directories, distribution={:?}",
            config.multi_disk.data_directories.len(),
            config.multi_disk.distribution_strategy
        );

        let stats = Arc::new(tokio::sync::RwLock::new(WalStats {
            total_entries: 0,
            memory_entries: 0,
            disk_segments: 0,
            total_disk_size_bytes: 0,
            memory_size_bytes: 0,
            collections_count: 0,
            last_flush_time: None,
            write_throughput_entries_per_sec: 0.0,
            read_throughput_entries_per_sec: 0.0,
            compression_ratio: 1.0,
        }));

        Ok(Self {
            strategy,
            config,
            stats,
            atomicity_manager: None,
            distance_compute: UnifiedDistanceCompute::default(),
        })
    }

    /// Set atomicity manager for transaction support
    pub fn set_atomicity_manager(
        &mut self,
        atomicity_manager: Arc<crate::storage::atomicity::AtomicityManager>,
    ) {
        self.atomicity_manager = Some(atomicity_manager);
        tracing::info!("🔒 Atomicity manager attached to WAL manager");
    }

    /// Set storage engine for delegated flush/compaction operations
    pub fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>) {
        self.strategy.set_storage_engine(storage_engine);
        tracing::info!("🏗️ Storage engine attached to WAL manager for delegated operations");
    }

    /// Get WAL configuration (read-only access)
    pub fn get_config(&self) -> &WalConfig {
        &self.config
    }

    /// Execute atomic operation with transaction support
    pub async fn execute_atomic_operation(
        &self,
        operation: Box<dyn crate::storage::atomicity::AtomicOperation>,
    ) -> Result<crate::storage::atomicity::OperationResult> {
        if let Some(atomicity_manager) = &self.atomicity_manager {
            // Begin transaction
            let transaction_id = atomicity_manager
                .begin_transaction(None, None)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to begin transaction: {}", e))?;

            // Execute operation
            match atomicity_manager
                .execute_operation(transaction_id, operation)
                .await
            {
                Ok(result) => {
                    // Commit transaction
                    atomicity_manager
                        .commit_transaction(transaction_id)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to commit transaction: {}", e))?;
                    Ok(result)
                }
                Err(e) => {
                    // Rollback transaction
                    let _ = atomicity_manager
                        .rollback_transaction(transaction_id, format!("Operation failed: {}", e))
                        .await;
                    Err(e)
                }
            }
        } else {
            Err(anyhow::anyhow!("Atomicity manager not configured"))
        }
    }

    /// Execute atomic operation within existing transaction
    pub async fn execute_in_transaction(
        &self,
        transaction_id: crate::storage::atomicity::TransactionId,
        operation: Box<dyn crate::storage::atomicity::AtomicOperation>,
    ) -> Result<crate::storage::atomicity::OperationResult> {
        if let Some(atomicity_manager) = &self.atomicity_manager {
            atomicity_manager
                .execute_operation(transaction_id, operation)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to execute operation in transaction: {}", e))
        } else {
            Err(anyhow::anyhow!("Atomicity manager not configured"))
        }
    }

    /// Begin a new transaction
    pub async fn begin_transaction(&self) -> Result<crate::storage::atomicity::TransactionId> {
        if let Some(atomicity_manager) = &self.atomicity_manager {
            atomicity_manager
                .begin_transaction(None, None)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to begin transaction: {}", e))
        } else {
            Err(anyhow::anyhow!("Atomicity manager not configured"))
        }
    }

    /// Commit a transaction
    pub async fn commit_transaction(
        &self,
        transaction_id: crate::storage::atomicity::TransactionId,
    ) -> Result<()> {
        if let Some(atomicity_manager) = &self.atomicity_manager {
            atomicity_manager
                .commit_transaction(transaction_id)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to commit transaction: {}", e))
        } else {
            Err(anyhow::anyhow!("Atomicity manager not configured"))
        }
    }

    /// Rollback a transaction
    pub async fn rollback_transaction(
        &self,
        transaction_id: crate::storage::atomicity::TransactionId,
        reason: String,
    ) -> Result<()> {
        if let Some(atomicity_manager) = &self.atomicity_manager {
            atomicity_manager
                .rollback_transaction(transaction_id, reason)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to rollback transaction: {}", e))
        } else {
            Err(anyhow::anyhow!("Atomicity manager not configured"))
        }
    }

    /// Insert single vector record (converted to upsert via AvroPayload)
    pub async fn insert(
        &self,
        collection_id: CollectionId,
        vector_id: VectorId,
        record: VectorRecord,
    ) -> Result<u64> {
        let start_time = std::time::Instant::now();

        debug!(
            "📝 [WAL_UPSERT] Starting upsert for collection: {}, vector_id: {}, vector_size: {} dims",
            collection_id,
            vector_id,
            record.vector.len()
        );

        // Convert VectorRecord to Avro bytes for zero-copy processing
        let avro_data = record.to_avro_bytes()
            .context("Failed to serialize VectorRecord to Avro")?;

        let entry = WalEntry {
            entry_id: vector_id.clone(),
            collection_id: collection_id.clone(),
            operation: WalOperation::AvroPayload {
                operation_type: "upsert".to_string(),
                avro_data,
            },
            timestamp: Utc::now(),
            sequence: 0,        // Will be set by strategy
            global_sequence: 0, // Will be set by strategy
            expires_at: None,
            version: 1,
            batch_id: None,
        };

        let result = self.strategy.write_entry(entry).await;
        let duration = start_time.elapsed();

        match &result {
            Ok(sequence) => {
                debug!(
                    "📝 [WAL_UPSERT] Successfully upserted vector {} in collection {} (sequence: {}) in {:?}",
                    vector_id,
                    collection_id,
                    sequence,
                    duration
                );
            }
            Err(e) => {
                debug!(
                    "📝 [WAL_UPSERT] Failed to upsert vector {} in collection {}: {} (duration: {:?})",
                    vector_id,
                    collection_id,
                    e,
                    duration
                );
            }
        }

        result
    }

    /// Insert batch of vector records (converted to upsert via single AvroPayload)
    pub async fn insert_batch(
        &self,
        collection_id: CollectionId,
        records: Vec<(VectorId, VectorRecord)>,
    ) -> Result<Vec<u64>> {
        // Convert batch to single Avro payload for efficient processing
        let vector_records: Vec<VectorRecord> = records.into_iter().map(|(_, record)| record).collect();
        let avro_data = crate::storage::persistence::wal::schema::serialize_vector_batch(&vector_records)
            .context("Failed to serialize vector batch to Avro")?;

        let entry = WalEntry {
            entry_id: format!("batch_{}", uuid::Uuid::new_v4()),
            collection_id: collection_id.clone(),
            operation: WalOperation::AvroPayload {
                operation_type: "upsert_batch".to_string(),
                avro_data,
            },
            timestamp: Utc::now(),
            sequence: 0,        // Will be set by strategy
            global_sequence: 0, // Will be set by strategy
            expires_at: None,
            version: 1,
            batch_id: None,
        };

        // Write single batch entry - strategy will handle batch processing
        let sequence = self.strategy.write_entry(entry).await?;
        
        // Return sequences for all vectors in batch (strategy generates these)
        Ok(vec![sequence; vector_records.len()])
    }

    /// Insert batch of vector records with immediate sync option (converted to upsert via AvroPayload)
    pub async fn insert_batch_with_sync(
        &self,
        collection_id: CollectionId,
        records: Vec<(VectorId, VectorRecord)>,
        immediate_sync: bool,
    ) -> Result<Vec<u64>> {
        // Convert batch to single Avro payload for efficient processing
        let vector_records: Vec<VectorRecord> = records.into_iter().map(|(_, record)| record).collect();
        let avro_data = crate::storage::persistence::wal::schema::serialize_vector_batch(&vector_records)
            .context("Failed to serialize vector batch to Avro")?;

        let entry = WalEntry {
            entry_id: format!("batch_sync_{}", uuid::Uuid::new_v4()),
            collection_id: collection_id.clone(),
            operation: WalOperation::AvroPayload {
                operation_type: "upsert_batch".to_string(),
                avro_data,
            },
            timestamp: Utc::now(),
            sequence: 0,        // Will be set by strategy
            global_sequence: 0, // Will be set by strategy
            expires_at: None,
            version: 1,
            batch_id: None,
        };

        // Write with sync option
        let sequence = if immediate_sync {
            let sequences = self.strategy.write_batch_with_sync(vec![entry], true).await?;
            sequences.into_iter().next().unwrap_or(0)
        } else {
            self.strategy.write_entry(entry).await?
        };
        
        // Return sequences for all vectors in batch
        Ok(vec![sequence; vector_records.len()])
    }

    /// Force immediate sync of WAL data to disk
    pub async fn force_sync(&self, collection_id: Option<&CollectionId>) -> Result<()> {
        self.strategy.force_sync(collection_id).await
    }

    /// Update vector record (redirects to upsert for consistency)
    pub async fn update(
        &self,
        collection_id: CollectionId,
        vector_id: VectorId,
        mut record: VectorRecord,
    ) -> Result<u64> {
        // Get current version for MVCC (optional optimization)
        if let Ok(Some(current)) = self.strategy.get_latest_entry(&collection_id, &vector_id).await {
            record.version = (current.version + 1) as i64;
        } else {
            record.version = 1;
        }

        // Redirect to insert (which is now upsert)
        self.insert(collection_id, vector_id, record).await
    }

    /// Delete vector record (soft delete with TTL)
    pub async fn delete(&self, collection_id: CollectionId, vector_id: VectorId) -> Result<u64> {
        let entry = WalEntry {
            entry_id: vector_id.clone(),
            collection_id: collection_id.clone(),
            operation: WalOperation::Delete {
                vector_id: vector_id.clone(),
                expires_at: Some(Utc::now() + chrono::Duration::days(30)), // 30-day soft delete
            },
            timestamp: Utc::now(),
            sequence: 0,
            global_sequence: 0,
            expires_at: Some(Utc::now() + chrono::Duration::days(30)),
            version: 1,
            batch_id: None,
        };

        self.strategy.write_entry(entry).await
    }

    // Note: Collection lifecycle operations (create/drop) are handled by CollectionService
    // WAL only handles vector-level operations (insert/update/delete/flush/checkpoint)

    /// Search for vector entries (for queries that need to check WAL)
    pub async fn search(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<WalEntry>> {
        self.strategy
            .search_by_vector_id(collection_id, vector_id)
            .await
    }

    /// Similarity search for unflushed vectors in WAL with configurable distance metric
    pub async fn search_vectors_similarity(
        &self,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
        distance_metric: Option<CoreDistanceMetric>,
    ) -> Result<Vec<(VectorId, f32, WalEntry)>> {
        self.strategy
            .search_vectors_similarity(collection_id, query_vector, k, distance_metric)
            .await
    }

    /// Read entries for recovery or replication
    pub async fn read_entries(
        &self,
        collection_id: &CollectionId,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<WalEntry>> {
        self.strategy
            .read_entries(collection_id, from_sequence, limit)
            .await
    }

    /// Force flush to disk
    pub async fn flush(&self, collection_id: Option<&CollectionId>) -> Result<FlushResult> {
        let result = self.strategy.flush(collection_id).await?;

        // Update stats
        let mut stats = self.stats.write().await;
        stats.last_flush_time = Some(Utc::now());

        Ok(result)
    }

    /// Compact collection (clean up old MVCC versions)
    pub async fn compact(&self, collection_id: &CollectionId) -> Result<u64> {
        self.strategy.compact_collection(collection_id).await
    }

    /// Append binary Avro entry directly (zero-copy WAL operation)
    ///
    /// RESILIENCE GUARANTEES:
    /// - Treats vector data as opaque binary blobs (never parsed)
    /// - Only extracts metadata fields (id, expires_at) for compaction
    /// - Corrupted vector data CANNOT crash server or compaction
    /// - Invalid payloads are logged and skipped during background processing
    /// - WAL writes succeed regardless of vector content validity
    pub async fn append_avro_entry(
        &self,
        collection_id: &str,
        operation_type: &str,
        avro_payload: &[u8],
    ) -> Result<u64> {
        // Create WAL entry with raw Avro payload (NO VALIDATION)
        // Vector content is stored as opaque bytes
        let entry = WalEntry {
            entry_id: format!(
                "avro_{}_{}",
                operation_type,
                Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ),
            collection_id: collection_id.to_string(), // Use actual collection_id from request
            operation: WalOperation::AvroPayload {
                operation_type: operation_type.to_string(),
                avro_data: avro_payload.to_vec(), // Raw bytes - never parsed
            },
            timestamp: Utc::now(),
            sequence: 0,        // Will be set by strategy
            global_sequence: 0, // Will be set by strategy
            expires_at: None,   // Extracted during compaction if needed
            version: 1,
            batch_id: None,
        };

        // Write to WAL immediately - no validation, maximum throughput
        self.strategy.write_entry(entry).await
    }

    /// Read binary Avro entries by operation type
    pub async fn read_avro_entries(
        &self,
        collection_id: &str,
        operation_type: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<u8>>> {
        let entries = self
            .strategy
            .read_entries(&collection_id.to_string(), 0, limit)
            .await?;

        let mut avro_payloads = Vec::new();
        for entry in entries {
            if let WalOperation::AvroPayload {
                operation_type: op_type,
                avro_data,
            } = entry.operation
            {
                if op_type == operation_type {
                    avro_payloads.push(avro_data);
                }
            }
        }

        Ok(avro_payloads)
    }

    /// Append batch entry with strategy-specific payload handling (unified for AVRO/BINCODE)
    ///
    /// This method provides a unified interface for both AVRO and BINCODE strategies
    /// to store batch data as single WAL entries, maintaining consistency in batch processing.
    pub async fn append_batch_entry(
        &self,
        collection_id: &str,
        operation_type: &str,
        payload: &[u8],
        immediate_sync: bool,
    ) -> Result<u64> {
        // Create WAL entry with binary payload (strategy-agnostic)
        let entry = WalEntry {
            entry_id: format!(
                "batch_{}_{}",
                operation_type,
                Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ),
            collection_id: collection_id.to_string(),
            operation: WalOperation::AvroPayload {
                operation_type: operation_type.to_string(),
                avro_data: payload.to_vec(), // Generic binary data (AVRO, Bincode, etc.)
            },
            timestamp: Utc::now(),
            sequence: 0,        // Will be set by strategy
            global_sequence: 0, // Will be set by strategy
            expires_at: None,
            version: 1,
            batch_id: None,
        };

        // Use write_batch_with_sync for immediate_sync support
        let sequences = self.strategy.write_batch_with_sync(vec![entry], immediate_sync).await?;
        Ok(sequences[0])
    }

    /// Get all entries for a collection from memtable (for similarity search)
    pub async fn get_collection_entries(
        &self,
        collection_id: &CollectionId,
    ) -> Result<Vec<WalEntry>> {
        self.strategy.get_collection_entries(collection_id).await
    }

    /// Get WAL statistics
    pub async fn stats(&self) -> Result<WalStats> {
        self.strategy.get_stats().await
    }

    /// Recover WAL from disk on startup
    pub async fn recover(&self) -> Result<u64> {
        self.strategy.recover().await
    }

    /// Graceful shutdown
    pub async fn close(&self) -> Result<()> {
        self.strategy.close().await
    }

    /// Atomically retrieve and mark WAL entries for flush operation
    pub async fn atomic_retrieve_for_flush(
        &self,
        collection_id: &CollectionId,
        flush_id: &str,
    ) -> Result<FlushCycle> {
        self.strategy
            .atomic_retrieve_for_flush(collection_id, flush_id)
            .await
    }

    /// Complete flush cycle - permanently remove flushed entries
    pub async fn complete_flush_cycle(
        &self,
        flush_cycle: FlushCycle,
    ) -> Result<FlushCompletionResult> {
        self.strategy.complete_flush_cycle(flush_cycle).await
    }

    /// Abort flush cycle - restore entries for retry
    pub async fn abort_flush_cycle(&self, flush_cycle: FlushCycle, reason: &str) -> Result<()> {
        self.strategy.abort_flush_cycle(flush_cycle, reason).await
    }

    /// Force flush all collections - FOR TESTING ONLY
    /// WARNING: This method should only be used for testing and debugging
    pub async fn force_flush_all(&self) -> Result<()> {
        tracing::warn!("⚠️ WAL MANAGER: FORCE FLUSH ALL - TESTING ONLY");
        self.strategy.force_flush_all().await
    }

    /// Force flush specific collection - FOR TESTING ONLY
    /// WARNING: This method should only be used for testing and debugging
    pub async fn force_flush_collection(
        &self,
        collection_id: &str,
        storage_engine: Option<&str>,
    ) -> Result<()> {
        tracing::warn!(
            "⚠️ WAL MANAGER: FORCE FLUSH COLLECTION {} - TESTING ONLY",
            collection_id
        );
        self.strategy
            .force_flush_collection(collection_id, storage_engine)
            .await
    }

    /// Register storage engine with the WAL's flush coordinator
    pub async fn register_storage_engine(
        &self,
        engine_name: &str,
        engine: Arc<dyn UnifiedStorageEngine>,
    ) -> Result<()> {
        self.strategy
            .register_storage_engine(engine_name, engine)
            .await
    }
}

impl DistanceComputeProvider for WalManager {
    fn distance_compute(&self) -> &UnifiedDistanceCompute {
        &self.distance_compute
    }
}

impl std::fmt::Debug for WalManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalManager")
            .field("strategy", &self.strategy.strategy_name())
            .field("config", &self.config)
            .finish()
    }
}
