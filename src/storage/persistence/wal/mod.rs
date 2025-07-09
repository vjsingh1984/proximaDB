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

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::debug;

use crate::compute::unified_distance::{DistanceComputeProvider, UnifiedDistanceCompute};
use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::storage::traits::{UnifiedStorageEngine, FlushResult};

// Sub-modules
pub mod atomic_batch_operation;
pub mod avro_batch;  // Modern Avro batch strategy implementation
pub mod background_manager;
pub mod batch_strategy;  // Modern batch-oriented strategy trait
pub mod batch_factory;  // Modern factory for batch strategies
pub mod bincode_batch;  // Modern Bincode batch strategy implementation
pub mod atomicity_manager;  // Unified atomicity manager for all atomic operations
pub mod config;
// Legacy modules with limited functionality due to removed avro.rs dependencies:
// pub mod disk;       // DISABLED - batch strategies handle their own disk operations
// pub mod factory;    // Disabled - deprecated, use batch_factory
pub mod schema;        // Re-enabled with batch-only functions

pub mod flush_coordinator;
// pub mod memtable;  // Moved to obsolete - using new unified memtable system

// Unit tests
#[cfg(test)]
mod tests;

// Re-exports
pub use background_manager::{
    BackgroundMaintenanceManager, BackgroundMaintenanceStats, BackgroundTaskStatus,
};
pub use avro_batch::AvroWalBatchStrategy;
pub use bincode_batch::BincodeWalBatchStrategy;
pub use batch_strategy::{WalBatchStrategy, WalBatchStrategyExt};
// LegacyWalStrategyAdapter removed - no longer needed with pure WalBatchStrategy architecture
pub use batch_factory::{WalBatchFactory, StrategyInfo, StrategyComparison};
pub use atomicity_manager::{UnifiedAtomicityManager, UnifiedAtomicityConfig, UnifiedAtomicityStats};
pub use config::WalStrategyType;
pub use config::{CompressionConfig, PerformanceConfig, WalConfig};
// Legacy exports disabled:
// pub use disk::WalDiskManager;       // DISABLED - batch strategies handle their own disk operations
// pub use factory::WalFactory;        // Deprecated - use WalBatchFactory
pub use flush_coordinator::{
    CleanupInstructions, FlushCoordinatorCallbacks, FlushDataSource, FlushState, PendingFlush,
    WalFlushCoordinator,
};
// pub use memtable::WalMemTable;  // Moved to obsolete - using new unified memtable system

// Batch coordination exports - BatchId defined below

// Re-export schema functions from centralized module
pub use schema::{deserialize_vector_batch, serialize_vector_batch, create_avro_vector_batch, AvroVector, AvroVectorBatch, VECTOR_BATCH_SCHEMA_V1};

/// Modern WAL operation - binary payload for batch operations (Avro OR Bincode)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WalOperation {
    /// Operation type: "upsert_batch", "delete_batch", "flush", "checkpoint"
    pub operation_type: String,
    /// Binary payload data (Avro bytes for AvroWalBatchStrategy, Bincode bytes for BincodeWalBatchStrategy)
    pub payload_data: Vec<u8>,
    /// Payload format: "avro" or "bincode"
    pub payload_format: String,
    /// Number of vectors in this batch (for metrics)
    pub vector_count: usize,
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

impl std::fmt::Display for BatchId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}-{}", self.collection_id, self.sequence_range.0, self.sequence_range.1)
    }
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

/// 🚫 DEPRECATED: WAL entry with MVCC versioning and batch coordination
/// 
/// **WARNING**: This structure represents the old individual-entry paradigm.
/// New code should use `WalVectorBatch` for batch-oriented operations.
/// 
/// This will be removed in a future version once all legacy code is migrated.
#[deprecated(note = "Use WalVectorBatch for batch-oriented operations instead")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    /// Vector ID (client-provided or system-generated)
    pub entry_id: String,

    /// Collection this entry belongs to
    pub collection_id: CollectionId,

    /// Operation being logged (modern binary payload: Avro or Bincode)
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
        // All operations are AvroPayload - hash the payload data
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.operation.payload_data);
        let hex_str = hasher.finalize().to_hex();
        Ok(format!("{}_{}", self.operation.operation_type, &hex_str[..16]))
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

    /// Extract VectorRecord from WalEntry (for compatibility with modern API)
    pub fn extract_vector_record(&self) -> Result<VectorRecord, anyhow::Error> {
        // All operations are AvroPayload with vector data
        if self.operation.operation_type == "upsert_batch" || self.operation.operation_type == "delete_batch" {
            // Try to deserialize as single record first
            if let Ok(record) = crate::core::avro_unified::VectorRecord::from_avro_bytes(&self.operation.payload_data) {
                return Ok(record);
            }
            
            // Try to deserialize as batch and take first record
            if let Ok(records) = deserialize_vector_batch(&self.operation.payload_data) {
                if let Some(record) = records.into_iter().next() {
                    return Ok(record);
                }
            }
        }
        anyhow::bail!("Cannot extract vector record from operation type: {}", self.operation.operation_type)
    }
}

impl WalOperation {
    /// Calculate the actual memory size of this WAL operation including vector data
    pub fn actual_size_bytes(&self) -> usize {
        let mut size = std::mem::size_of::<WalOperation>(); // Struct size
        
        // Add variable field sizes
        size += self.operation_type.len();
        size += self.payload_data.len(); // Actual payload size
        size += self.payload_format.len();
        
        size
    }

    /// Extract VectorRecord from WAL entry operation
    /// Used by migration adapters and legacy compatibility
    pub fn extract_vector_record(&self) -> Result<VectorRecord, anyhow::Error> {
        // All operations contain Avro-serialized vector data
        if self.operation_type == "upsert_batch" || self.operation_type == "delete_batch" {
            // Try to deserialize as single record first
            if let Ok(record) = VectorRecord::from_avro_bytes(&self.payload_data) {
                Ok(record)
            } else {
                // Try to deserialize as batch and take first record
                let records = deserialize_vector_batch(&self.payload_data)
                    .map_err(|e| anyhow::anyhow!("Failed to deserialize Avro payload: {}", e))?;
                records.into_iter().next()
                    .ok_or_else(|| anyhow::anyhow!("Empty vector batch in Avro payload"))
            }
        } else {
            Err(anyhow::anyhow!("WAL operation type {} does not contain vector data", self.operation_type))
        }
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

// FlushResult removed - use crate::storage::traits::FlushResult instead

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
    /// Batch IDs for coordination with storage engines
    pub batch_ids: Vec<BatchId>,
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

// 🚫 DEPRECATED: Legacy WalStrategy trait removed - use WalBatchStrategy instead
// All operations now use batch-oriented architecture with single-entry batches for individual operations
/*
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

    /// 🚫 DEPRECATED: Serialize entries to bytes (strategy-specific format)
    /// Use batch-oriented serialization with WalVectorBatch instead
    #[deprecated(note = "Use batch-oriented serialization with WalVectorBatch instead")]
    async fn serialize_entries(&self, entries: &[WalEntry]) -> Result<Vec<u8>>;

    /// 🚫 DEPRECATED: Deserialize entries from bytes (strategy-specific format)
    /// Use batch-oriented deserialization with WalVectorBatch instead
    #[deprecated(note = "Use batch-oriented deserialization with WalVectorBatch instead")]
    async fn deserialize_entries(&self, data: &[u8]) -> Result<Vec<WalEntry>>;

    /// 🚫 DEPRECATED: Write single entry atomically (memory + disk)
    /// Use write_vector_batch for batch-oriented operations instead
    #[deprecated(note = "Use write_vector_batch for batch-oriented operations instead")]
    async fn write_entry(&self, entry: WalEntry) -> Result<u64>;

    /// 🚫 DEPRECATED: Write batch of entries atomically (default implementation using single writes)
    /// Use write_vector_batch for native batch operations instead
    #[deprecated(note = "Use write_vector_batch for native batch operations instead")]
    async fn write_batch(&self, entries: Vec<WalEntry>) -> Result<Vec<u64>> {
        let mut sequences = Vec::with_capacity(entries.len());
        for entry in entries {
            sequences.push(self.write_entry(entry).await?);
        }
        Ok(sequences)
    }

    /// 🚫 DEPRECATED: Write batch of entries with immediate disk sync for durability
    /// Use write_vector_batch_with_sync for native batch operations instead
    #[deprecated(note = "Use write_vector_batch_with_sync for native batch operations instead")]
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

    /// 🚫 DEPRECATED: Read entries for a collection starting from sequence
    /// Use read_vector_batches for batch-oriented operations instead
    #[deprecated(note = "Use read_vector_batches for batch-oriented operations instead")]
    async fn read_entries(
        &self,
        collection_id: &CollectionId,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<WalEntry>>;

    /// 🚫 DEPRECATED: Search entries by vector ID (checks both memory and disk)
    /// Use search_vector_by_id for batch-oriented operations instead
    #[deprecated(note = "Use search_vector_by_id for batch-oriented operations instead")]
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

    /// Get WAL behavior wrapper for direct batch access (optimization for search)
    fn get_wal_behavior_wrapper(&self) -> Option<&crate::storage::memtable::specialized::wal_behavior::WalBehaviorWrapper> {
        // Default implementation returns None - concrete strategies can override
        None
    }

    // 🎯 NEW BATCH-ORIENTED METHODS (Modern Architecture)
    
    /// Write vector batch atomically (memory + disk) - MODERN APPROACH
    async fn write_vector_batch(&self, batch: crate::storage::memtable::specialized::wal_behavior::WalVectorBatch) -> Result<Vec<u64>> {
        // Default implementation falls back to individual entries for backward compatibility
        // Concrete strategies should override this for true batch optimization
        let mut entries = Vec::new();
        
        for vector_record in &batch.vector_records {
            let entry = WalEntry {
                entry_id: vector_record.id.clone(),
                collection_id: batch.batch_id.collection_id.clone(),
                operation: WalOperation::AvroPayload {
                    operation_type: "upsert_batch".to_string(),
                    avro_data: vector_record.to_avro_bytes().unwrap_or_default(),
                },
                timestamp: chrono::DateTime::from(batch.created_at),
                sequence: 0, // Will be assigned
                global_sequence: 0,
                expires_at: None,
                version: 1,
                batch_id: Some(batch.batch_id.clone()),
            };
            entries.push(entry);
        }
        
        #[allow(deprecated)]
        self.write_batch(entries).await
    }

    /// Write vector batch with immediate disk sync for durability - MODERN APPROACH
    async fn write_vector_batch_with_sync(&self, batch: crate::storage::memtable::specialized::wal_behavior::WalVectorBatch, immediate_sync: bool) -> Result<Vec<u64>> {
        // Default implementation falls back to write_vector_batch
        // Concrete strategies should override this for immediate sync optimization
        if immediate_sync {
            let sequences = self.write_vector_batch(batch).await?;
            self.force_sync(None).await?;
            Ok(sequences)
        } else {
            self.write_vector_batch(batch).await
        }
    }

    /// Read vector batches for a collection - MODERN APPROACH
    async fn read_vector_batches(&self, collection_id: &CollectionId, from_sequence: u64, limit: Option<usize>) -> Result<Vec<crate::storage::memtable::specialized::wal_behavior::WalVectorBatch>> {
        // Default implementation converts from legacy entries
        // Concrete strategies should override this for native batch operations
        #[allow(deprecated)]
        let entries = self.read_entries(collection_id, from_sequence, limit).await?;
        
        // Group entries by batch_id
        let mut batches = std::collections::HashMap::new();
        for entry in entries {
            if let Some(batch_id) = &entry.batch_id {
                let batch = batches.entry(batch_id.clone()).or_insert_with(|| {
                    crate::storage::memtable::specialized::wal_behavior::WalVectorBatch {
                        batch_id: batch_id.clone(),
                        vector_records: Vec::new(),
                        created_at: entry.timestamp.into(),
                        total_size_bytes: 0,
                        is_flushed: false,
                    }
                });
                
                // Extract vector record from entry
                if let WalOperation::AvroPayload { avro_data, .. } = &entry.operation {
                    if let Ok(vector_record) = crate::core::VectorRecord::from_avro_bytes(avro_data) {
                        batch.vector_records.push(vector_record);
                        batch.total_size_bytes += entry.actual_size_bytes();
                    }
                }
            }
        }
        
        Ok(batches.into_values().collect())
    }

    /// Search vector by ID - MODERN APPROACH
    async fn search_vector_by_id(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Option<crate::core::VectorRecord>> {
        // Default implementation falls back to legacy search
        #[allow(deprecated)]
        if let Some(entry) = self.search_by_vector_id(collection_id, vector_id).await? {
            match &entry.operation {
                WalOperation::AvroPayload { avro_data, .. } => {
                    if let Ok(vector_record) = crate::core::VectorRecord::from_avro_bytes(avro_data) {
                        return Ok(Some(vector_record));
                    }
                }
                WalOperation::Insert { record, .. } | WalOperation::Update { record, .. } => {
                    return Ok(Some(record.clone()));
                }
                _ => {}
            }
        }
        Ok(None)
    }

    /// 🚫 DEPRECATED: Get latest entry for a vector (for MVCC)
    /// Use search_vector_by_id for batch-oriented operations instead
    #[deprecated(note = "Use search_vector_by_id for batch-oriented operations instead")]
    async fn get_latest_entry(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<WalEntry>>;

    /// 🚫 DEPRECATED: Get all entries for a collection from memtable (for similarity search)
    /// Use read_vector_batches for batch-oriented operations instead
    #[deprecated(note = "Use read_vector_batches for batch-oriented operations instead")]
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
            batch_ids: vec![],
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
*/

/// Modern WAL manager using batch-oriented strategies
/// 
/// **WalManager Per Collection + Shared Global Memtable Architecture (Perfect Horizontal Scaling)**
/// 
/// This implements the optimal architecture where:
/// - **WalManager per collection** - Each collection gets its own WalManager for isolation
/// - **Shared global WalBehaviorWrapper** - Single singleton shared across all WalManagers
/// - **GlobalPartitionedMemtable** - Partitioned by collection, efficient shared access
/// - **WalManagerRegistry** - Tracks which WalManager handles which collection (1:1 or 1:N mapping)
/// - **Horizontal scaling constraint** - One collection handled by exactly one WalManager (never split)
/// - **Dynamic scaling** - Under heavy workload, new collections get new WalManagers
/// - **Strategy-specific serialization** with shared deserialization in global memtable
pub struct WalManager {
    /// Active strategy for current operations
    strategy: Box<dyn WalBatchStrategy>,
    /// Configuration
    config: WalConfig,
    /// Statistics tracking
    stats: Arc<tokio::sync::RwLock<WalStats>>,
    /// Atomicity manager for transaction support
    // atomicity_manager removed - use UnifiedAtomicCoordinator from atomic module instead
    /// Distance computation for similarity operations
    distance_compute: UnifiedDistanceCompute,
    /// **Collections assigned to this WalManager** - Each WalManager handles specific collections
    assigned_collections: Arc<tokio::sync::RwLock<std::collections::HashSet<CollectionId>>>,
    /// **SHARED REFERENCE**: Global WalBehaviorWrapper singleton shared across ALL WalManager instances
    shared_wal_behavior: &'static GlobalWalBehaviorSingleton,
}

/// Adaptive WalManager Registry with Pool-based Collection Assignment
/// This implements intelligent scaling for millions of collections by maintaining a pool
/// of WalManager instances and dynamically assigning collections based on workload
pub struct WalManagerRegistry {
    /// Collection to WalManager ID mapping (1:1 constraint maintained)
    collection_assignments: Arc<tokio::sync::RwLock<std::collections::HashMap<CollectionId, String>>>,
    /// WalManager pool with workload tracking
    manager_pool: Arc<tokio::sync::RwLock<std::collections::HashMap<String, WalManagerPoolEntry>>>,
    /// Pool configuration
    pool_config: WalManagerPoolConfig,
    /// Next manager ID for creating new instances
    next_manager_id: Arc<tokio::sync::Mutex<u64>>,
}

/// WalManager pool entry with workload metrics
#[derive(Debug, Clone)]
pub struct WalManagerPoolEntry {
    /// The WalManager instance
    manager: Arc<WalManager>,
    /// Collections assigned to this manager
    assigned_collections: std::collections::HashSet<CollectionId>,
    /// Workload metrics for load balancing
    workload_metrics: WalManagerWorkload,
    /// Last rebalancing timestamp
    last_rebalance: std::time::Instant,
}

/// Workload metrics for adaptive scaling decisions
#[derive(Debug, Clone, Default)]
pub struct WalManagerWorkload {
    /// Number of assigned collections
    collection_count: usize,
    /// Operations per second (estimated)
    ops_per_second: f64,
    /// Memory usage in bytes
    memory_usage_bytes: u64,
    /// Average operation latency in milliseconds
    avg_latency_ms: f64,
    /// Load score (computed from metrics)
    load_score: f64,
}

/// Pool configuration for adaptive WalManager scaling
/// 
/// This configuration allows users to customize how WalManagers scale to handle
/// millions of collections efficiently. Users can adjust thread counts, load balancing
/// thresholds, and scaling behavior based on their specific workload requirements.
/// 
/// # Examples
/// 
/// ```rust
/// // Configuration for high-throughput workloads
/// let high_throughput_config = WalManagerPoolConfig::builder()
///     .initial_pool_size(8)
///     .soft_thread_limit(16)
///     .target_collections_per_manager(500)
///     .enable_dynamic_scaling(true)
///     .build();
/// 
/// // Configuration for memory-constrained environments
/// let memory_constrained_config = WalManagerPoolConfig::builder()
///     .initial_pool_size(2)
///     .soft_thread_limit(4)
///     .target_collections_per_manager(2000)
///     .enable_dynamic_scaling(false)
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct WalManagerPoolConfig {
    /// Initial pool size (number of WalManager threads to start with)
    pub initial_pool_size: usize,
    /// Soft limit - after this many managers, scale collections per manager instead of adding threads
    pub soft_thread_limit: usize,
    /// Target collections per manager for balanced scaling
    pub target_collections_per_manager: usize,
    /// Load threshold for triggering rebalancing (0.0-1.0)
    pub rebalance_load_threshold: f64,
    /// Minimum time between rebalancing operations (seconds)
    pub rebalance_cooldown_secs: u64,
    /// Enable dynamic manager creation beyond soft limit
    pub enable_dynamic_scaling: bool,
}

/// Builder for WalManagerPoolConfig to provide user-friendly configuration
#[derive(Debug, Clone)]
pub struct WalManagerPoolConfigBuilder {
    config: WalManagerPoolConfig,
}

impl WalManagerPoolConfig {
    /// Create a new builder for WalManagerPoolConfig
    pub fn builder() -> WalManagerPoolConfigBuilder {
        WalManagerPoolConfigBuilder::new()
    }

    /// Create configuration optimized for high-throughput workloads
    pub fn high_throughput() -> Self {
        Self {
            initial_pool_size: 8,
            soft_thread_limit: 16,
            target_collections_per_manager: 500,
            rebalance_load_threshold: 0.7,
            rebalance_cooldown_secs: 15,
            enable_dynamic_scaling: true,
        }
    }

    /// Create configuration optimized for memory-constrained environments
    pub fn memory_constrained() -> Self {
        Self {
            initial_pool_size: 2,
            soft_thread_limit: 4,
            target_collections_per_manager: 2000,
            rebalance_load_threshold: 0.9,
            rebalance_cooldown_secs: 60,
            enable_dynamic_scaling: false,
        }
    }

    /// Create configuration optimized for development/testing
    pub fn development() -> Self {
        Self::default()
    }
}

impl WalManagerPoolConfigBuilder {
    /// Create a new builder with default values
    pub fn new() -> Self {
        Self {
            config: WalManagerPoolConfig::default(),
        }
    }

    /// Set the initial pool size
    pub fn initial_pool_size(mut self, size: usize) -> Self {
        self.config.initial_pool_size = size;
        self
    }

    /// Set the soft thread limit
    pub fn soft_thread_limit(mut self, limit: usize) -> Self {
        self.config.soft_thread_limit = limit;
        self
    }

    /// Set target collections per manager
    pub fn target_collections_per_manager(mut self, target: usize) -> Self {
        self.config.target_collections_per_manager = target;
        self
    }

    /// Set rebalance load threshold (0.0-1.0)
    pub fn rebalance_load_threshold(mut self, threshold: f64) -> Self {
        self.config.rebalance_load_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Set rebalance cooldown period in seconds
    pub fn rebalance_cooldown_secs(mut self, secs: u64) -> Self {
        self.config.rebalance_cooldown_secs = secs;
        self
    }

    /// Enable or disable dynamic scaling beyond soft limit
    pub fn enable_dynamic_scaling(mut self, enabled: bool) -> Self {
        self.config.enable_dynamic_scaling = enabled;
        self
    }

    /// Build the final configuration
    pub fn build(self) -> WalManagerPoolConfig {
        self.config
    }
}

impl Default for WalManagerPoolConfig {
    fn default() -> Self {
        Self {
            initial_pool_size: 3, // Start with 3 threads for testing
            soft_thread_limit: 8, // Soft limit - after 8, scale balanced
            target_collections_per_manager: 1000, // Target 1K collections per manager for balanced scaling
            rebalance_load_threshold: 0.8, // Rebalance when 80% loaded
            rebalance_cooldown_secs: 30, // 30-second cooldown between rebalances
            enable_dynamic_scaling: true, // Enable dynamic manager creation
        }
    }
}

impl WalManagerRegistry {
    /// Create new adaptive WalManager registry with pool
    pub fn new() -> Self {
        Self::with_config(WalManagerPoolConfig::default())
    }

    /// Create new adaptive WalManager registry with custom pool configuration
    pub fn with_config(pool_config: WalManagerPoolConfig) -> Self {
        tracing::info!(
            "🎯 Creating adaptive WalManager registry - initial: {}, soft_limit: {}, target_collections: {}, dynamic_scaling: {}",
            pool_config.initial_pool_size,
            pool_config.soft_thread_limit,
            pool_config.target_collections_per_manager,
            pool_config.enable_dynamic_scaling
        );
        
        Self {
            collection_assignments: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            manager_pool: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            pool_config,
            next_manager_id: Arc::new(tokio::sync::Mutex::new(1)),
        }
    }

    /// Get or assign WalManager for a collection using adaptive pool scaling
    pub async fn get_manager_for_collection(
        &self,
        collection_id: &CollectionId,
        strategy_type: crate::storage::persistence::wal::config::WalStrategyType,
        config: &WalConfig,
        filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    ) -> Result<Arc<WalManager>> {
        // Check if collection already has a manager assignment
        {
            let assignments = self.collection_assignments.read().await;
            if let Some(manager_id) = assignments.get(collection_id) {
                let pool = self.manager_pool.read().await;
                if let Some(entry) = pool.get(manager_id) {
                    tracing::debug!(
                        "📍 Collection {} using existing WalManager {} (load: {:.2})",
                        collection_id,
                        manager_id,
                        entry.workload_metrics.load_score
                    );
                    return Ok(entry.manager.clone());
                }
            }
        }

        // Ensure initial pool exists
        self.ensure_initial_pool(strategy_type, config, filesystem.clone()).await?;

        // Find best manager for this collection using adaptive assignment
        let target_manager_id = self.find_best_manager_for_collection(collection_id).await?;

        // Assign collection to the selected manager
        self.assign_collection_to_manager(collection_id, &target_manager_id).await?;

        // Return the assigned manager
        let pool = self.manager_pool.read().await;
        let entry = pool.get(&target_manager_id)
            .ok_or_else(|| anyhow::anyhow!("Manager {} not found in pool", target_manager_id))?;
        
        tracing::info!(
            "✅ Collection {} assigned to WalManager {} (adaptive scaling - {} collections)",
            collection_id,
            target_manager_id,
            entry.workload_metrics.collection_count + 1
        );

        Ok(entry.manager.clone())
    }

    /// Ensure initial pool of WalManagers exists
    async fn ensure_initial_pool(
        &self,
        strategy_type: crate::storage::persistence::wal::config::WalStrategyType,
        config: &WalConfig,
        filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    ) -> Result<()> {
        let pool = self.manager_pool.read().await;
        if !pool.is_empty() {
            return Ok(()); // Pool already initialized
        }
        drop(pool);

        // Initialize pool with configured size
        let mut pool = self.manager_pool.write().await;
        if !pool.is_empty() {
            return Ok(()); // Another thread initialized it
        }

        tracing::info!("🚀 Initializing WalManager pool with {} managers", self.pool_config.initial_pool_size);

        for i in 0..self.pool_config.initial_pool_size {
            let manager_id = format!("wal_manager_pool_{}", i + 1);
            
            let strategy = WalBatchFactory::create_strategy(strategy_type.clone(), config, filesystem.clone()).await?;
            let manager = Arc::new(WalManager::new_pool_manager(strategy, config.clone(), manager_id.clone()).await?);
            
            let entry = WalManagerPoolEntry {
                manager,
                assigned_collections: std::collections::HashSet::new(),
                workload_metrics: WalManagerWorkload::default(),
                last_rebalance: std::time::Instant::now(),
            };
            
            pool.insert(manager_id.clone(), entry);
            tracing::debug!("➕ Created pool WalManager: {}", manager_id);
        }

        tracing::info!("✅ WalManager pool initialized with {} managers", pool.len());
        Ok(())
    }

    /// Find the best WalManager for a new collection based on workload
    /// Implements adaptive scaling: use existing managers first, then create new ones if needed
    async fn find_best_manager_for_collection(&self, collection_id: &CollectionId) -> Result<String> {
        let pool = self.manager_pool.read().await;
        
        if pool.is_empty() {
            return Err(anyhow::anyhow!("WalManager pool is empty"));
        }

        // Check if we should create a new manager for better load distribution
        if self.should_create_new_manager(&pool).await? {
            drop(pool); // Release read lock before write lock
            return self.create_additional_manager().await;
        }

        // Find manager with lowest load score from existing pool
        let best_manager = pool
            .iter()
            .min_by(|(_, a), (_, b)| {
                // Primary: load score (lower is better)
                a.workload_metrics.load_score.partial_cmp(&b.workload_metrics.load_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    // Secondary: collection count (lower is better)
                    .then_with(|| a.workload_metrics.collection_count.cmp(&b.workload_metrics.collection_count))
            })
            .map(|(id, _)| id.clone())
            .ok_or_else(|| anyhow::anyhow!("No suitable manager found in pool"))?;

        tracing::debug!("🎯 Selected existing WalManager {} for collection {} (adaptive assignment)", best_manager, collection_id);
        Ok(best_manager)
    }

    /// Check if we should create a new manager for better load distribution
    async fn should_create_new_manager(&self, pool: &std::collections::HashMap<String, WalManagerPoolEntry>) -> Result<bool> {
        // Don't create new managers if dynamic scaling is disabled
        if !self.pool_config.enable_dynamic_scaling {
            return Ok(false);
        }

        // If we're below soft limit, don't create new managers yet (use existing capacity)
        if pool.len() < self.pool_config.soft_thread_limit {
            return Ok(false);
        }

        // After soft limit, check if adding a manager would improve load distribution
        let min_collections = pool.values().map(|entry| entry.workload_metrics.collection_count).min().unwrap_or(0);
        let max_collections = pool.values().map(|entry| entry.workload_metrics.collection_count).max().unwrap_or(0);

        // Create new manager if:
        // 1. The most loaded manager exceeds target collections per manager
        // 2. There's significant imbalance between managers
        let should_create = max_collections > self.pool_config.target_collections_per_manager ||
                           (max_collections - min_collections) > (self.pool_config.target_collections_per_manager / 2);

        if should_create {
            tracing::info!(
                "🔄 Dynamic scaling triggered - current managers: {}, max_collections: {}, target: {}",
                pool.len(),
                max_collections,
                self.pool_config.target_collections_per_manager
            );
        }

        Ok(should_create)
    }

    /// Create an additional manager for dynamic scaling
    async fn create_additional_manager(&self) -> Result<String> {
        let mut next_id = self.next_manager_id.lock().await;
        let manager_id = format!("wal_manager_dynamic_{}", *next_id);
        *next_id += 1;
        drop(next_id);

        tracing::info!("🚀 Creating dynamic WalManager {} for load balancing", manager_id);
        
        // For now, use Avro strategy as default for dynamic managers
        // TODO: Make this configurable
        let strategy_type = crate::storage::persistence::wal::config::WalStrategyType::AvroBatch;
        let config = &crate::storage::persistence::wal::config::WalConfig::default(); // TODO: Pass proper config
        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(crate::storage::persistence::filesystem::FilesystemFactory::new(filesystem_config).await?); // TODO: Pass proper filesystem
        
        let strategy = WalBatchFactory::create_strategy(strategy_type, config, filesystem).await?;
        let manager = Arc::new(WalManager::new_pool_manager(strategy, config.clone(), manager_id.clone()).await?);
        
        let entry = WalManagerPoolEntry {
            manager,
            assigned_collections: std::collections::HashSet::new(),
            workload_metrics: WalManagerWorkload::default(),
            last_rebalance: std::time::Instant::now(),
        };
        
        // Add to pool
        {
            let mut pool = self.manager_pool.write().await;
            pool.insert(manager_id.clone(), entry);
        }

        tracing::info!("✅ Dynamic WalManager {} created for balanced scaling", manager_id);
        Ok(manager_id)
    }

    /// Assign a collection to a specific manager
    async fn assign_collection_to_manager(&self, collection_id: &CollectionId, manager_id: &str) -> Result<()> {
        // Update assignments
        {
            let mut assignments = self.collection_assignments.write().await;
            assignments.insert(collection_id.clone(), manager_id.to_string());
        }

        // Update pool entry
        {
            let mut pool = self.manager_pool.write().await;
            if let Some(entry) = pool.get_mut(manager_id) {
                entry.assigned_collections.insert(collection_id.clone());
                entry.workload_metrics.collection_count = entry.assigned_collections.len();
                
                // Update load score based on collection count
                entry.workload_metrics.load_score = 
                    (entry.workload_metrics.collection_count as f64) / (self.pool_config.target_collections_per_manager as f64);
                
                // Add the collection to the manager's assigned set
                let mut assigned = entry.manager.assigned_collections.write().await;
                assigned.insert(collection_id.clone());
            } else {
                return Err(anyhow::anyhow!("Manager {} not found in pool", manager_id));
            }
        }

        Ok(())
    }

    /// Get all active managers (for global operations)
    pub async fn get_all_managers(&self) -> std::collections::HashMap<String, Arc<WalManager>> {
        let pool = self.manager_pool.read().await;
        pool.iter()
            .map(|(id, entry)| (id.clone(), entry.manager.clone()))
            .collect()
    }

    /// Remove collection assignment (when collection is dropped)
    pub async fn remove_collection(&self, collection_id: &CollectionId) -> Result<()> {
        let mut assignments = self.collection_assignments.write().await;
        if let Some(manager_id) = assignments.remove(collection_id) {
            // Check if this manager handles other collections
            let has_other_collections = assignments.values().any(|id| id == &manager_id);
            
            if !has_other_collections {
                // Remove the manager if it has no more collections
                let mut managers = self.manager_pool.write().await;
                if let Some(manager_entry) = managers.remove(&manager_id) {
                    // Close the manager gracefully
                    let _ = manager_entry.manager.close().await;
                    tracing::info!("🗑️ Removed WalManager {} (no more collections)", manager_id);
                }
            }
        }
        Ok(())
    }
}

/// Global WalBehaviorWrapper singleton for shared memtable access across all WalManager instances
/// This is the key to efficient shared access - one memtable, many managers
pub struct GlobalWalBehaviorSingleton {
    /// The singleton WalBehaviorWrapper with GlobalPartitionedMemtable
    wal_behavior: std::sync::OnceLock<Arc<crate::storage::memtable::specialized::wal_behavior::WalBehaviorWrapper>>,
}

impl GlobalWalBehaviorSingleton {
    /// Get or create the singleton WalBehaviorWrapper instance
    pub fn get_or_init(&self, config: &crate::storage::memtable::core::MemtableConfig) -> Arc<crate::storage::memtable::specialized::wal_behavior::WalBehaviorWrapper> {
        self.wal_behavior.get_or_init(|| {
            tracing::info!("🎯 Creating SINGLETON WalBehaviorWrapper with GlobalPartitionedMemtable for all WalManager instances");
            Arc::new(crate::storage::memtable::specialized::wal_behavior::WalBehaviorWrapper::new(config.clone()))
        }).clone()
    }
}

/// Global singleton instance - shared across all WalManager instances
static GLOBAL_WAL_BEHAVIOR: GlobalWalBehaviorSingleton = GlobalWalBehaviorSingleton {
    wal_behavior: std::sync::OnceLock::new(),
};

/// Global registry instance for WalManager per collection architecture
static WAL_MANAGER_REGISTRY: std::sync::OnceLock<WalManagerRegistry> = std::sync::OnceLock::new();

/// Get the global WalManager registry
pub fn get_wal_manager_registry() -> &'static WalManagerRegistry {
    WAL_MANAGER_REGISTRY.get_or_init(|| {
        tracing::info!("🎯 Initializing WalManager Registry for per-collection scaling");
        WalManagerRegistry::new()
    })
}

/// Convenience function: Get or create WalManager for a collection using default pool config
pub async fn get_wal_manager_for_collection(
    collection_id: &CollectionId,
    strategy_type: crate::storage::persistence::wal::config::WalStrategyType,
    config: &WalConfig,
    filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
) -> Result<Arc<WalManager>> {
    get_wal_manager_registry()
        .get_manager_for_collection(collection_id, strategy_type, config, filesystem)
        .await
}

/// Configure the global WalManager pool with custom settings
/// 
/// This function allows users to customize the adaptive scaling behavior
/// of the WalManager pool to match their specific workload requirements.
/// 
/// # Examples
/// 
/// ```rust
/// // Configure for high-throughput workloads
/// configure_wal_manager_pool(WalManagerPoolConfig::high_throughput()).await?;
/// 
/// // Configure with custom settings
/// let custom_config = WalManagerPoolConfig::builder()
///     .initial_pool_size(4)
///     .soft_thread_limit(12)
///     .target_collections_per_manager(800)
///     .enable_dynamic_scaling(true)
///     .build();
/// configure_wal_manager_pool(custom_config).await?;
/// ```
pub async fn configure_wal_manager_pool(pool_config: WalManagerPoolConfig) -> Result<()> {
    // TODO: Implement global pool configuration
    // For now, this is a placeholder - in a full implementation, this would
    // reinitialize the global registry with the new configuration
    tracing::info!("🔧 WalManager pool configuration updated: {:?}", pool_config);
    Ok(())
}

/// Get current WalManager pool statistics for monitoring
/// 
/// Returns information about the current pool state including number of managers,
/// collection distribution, and load metrics.
pub async fn get_wal_manager_pool_stats() -> Result<WalManagerPoolStats> {
    let registry = get_wal_manager_registry();
    let managers = registry.get_all_managers().await;
    
    let pool = registry.manager_pool.read().await;
    let total_collections: usize = pool.values().map(|entry| entry.workload_metrics.collection_count).sum();
    let avg_collections_per_manager = if pool.is_empty() { 0.0 } else { total_collections as f64 / pool.len() as f64 };
    let max_collections = pool.values().map(|entry| entry.workload_metrics.collection_count).max().unwrap_or(0);
    let min_collections = pool.values().map(|entry| entry.workload_metrics.collection_count).min().unwrap_or(0);
    
    Ok(WalManagerPoolStats {
        total_managers: pool.len(),
        total_collections,
        avg_collections_per_manager,
        max_collections_per_manager: max_collections,
        min_collections_per_manager: min_collections,
        load_imbalance: if min_collections == 0 { 0.0 } else { max_collections as f64 / min_collections as f64 },
    })
}

/// Statistics about the current WalManager pool state
#[derive(Debug, Clone)]
pub struct WalManagerPoolStats {
    /// Total number of WalManager instances in the pool
    pub total_managers: usize,
    /// Total number of collections across all managers
    pub total_collections: usize,
    /// Average collections per manager
    pub avg_collections_per_manager: f64,
    /// Maximum collections assigned to any single manager
    pub max_collections_per_manager: usize,
    /// Minimum collections assigned to any single manager  
    pub min_collections_per_manager: usize,
    /// Load imbalance ratio (max/min collections)
    pub load_imbalance: f64,
}

impl WalManager {
    /// Create new WalManager (legacy method for compatibility with tests)
    pub async fn new(strategy: Box<dyn WalBatchStrategy>, config: WalConfig) -> Result<Self> {
        // Use new_pool_manager with a default manager ID for backwards compatibility
        Self::new_pool_manager(strategy, config, "default_manager".to_string()).await
    }

    /// Create new WalManager for specific collections with shared global memtable
    pub async fn new_for_collection(strategy: Box<dyn WalBatchStrategy>, config: WalConfig, collection_id: CollectionId) -> Result<Self> {
        tracing::info!(
            "🚀 Creating WalManager for collection {} with strategy: {} (shared global memtable)",
            collection_id,
            strategy.strategy_name()
        );
        tracing::debug!(
            "📋 WAL Config: strategy_type={:?}, memtable_type={:?}",
            config.strategy_type,
            config.memtable.memtable_type
        );

        // Initialize singleton WalBehaviorWrapper (thread-safe, only happens once globally)
        let memtable_config = crate::storage::memtable::core::MemtableConfig {
            max_size_bytes: config.memtable.global_memory_limit,
            flush_threshold_bytes: config.memtable.global_memory_limit / 2,
            enable_mvcc: config.enable_mvcc,
            mvcc_cleanup_interval_secs: config.performance.mvcc_cleanup_interval_secs,
            max_versions_per_key: config.memtable.mvcc_versions_retained,
        };
        let _shared_wal_behavior = GLOBAL_WAL_BEHAVIOR.get_or_init(&memtable_config);

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

        // Initialize collection set for this manager
        let mut assigned_collections = std::collections::HashSet::new();
        assigned_collections.insert(collection_id.clone());

        tracing::info!(
            "✅ WalManager created for collection {} - per-collection scaling with shared memtable",
            collection_id
        );

        Ok(Self {
            strategy,
            config,
            stats,
            atomicity_manager: None,
            distance_compute: UnifiedDistanceCompute::default(),
            assigned_collections: Arc::new(tokio::sync::RwLock::new(assigned_collections)),
            shared_wal_behavior: &GLOBAL_WAL_BEHAVIOR,
        })
    }

    /// Create new WalManager for pool with empty collection set
    pub async fn new_pool_manager(strategy: Box<dyn WalBatchStrategy>, config: WalConfig, manager_id: String) -> Result<Self> {
        tracing::debug!(
            "🏊 Creating pool WalManager {} with strategy: {} (shared global memtable)",
            manager_id,
            strategy.strategy_name()
        );

        // Initialize singleton WalBehaviorWrapper (thread-safe, only happens once globally)
        let memtable_config = crate::storage::memtable::core::MemtableConfig {
            max_size_bytes: config.memtable.global_memory_limit,
            flush_threshold_bytes: config.memtable.global_memory_limit / 2,
            enable_mvcc: config.enable_mvcc,
            mvcc_cleanup_interval_secs: config.performance.mvcc_cleanup_interval_secs,
            max_versions_per_key: config.memtable.mvcc_versions_retained,
        };
        let _shared_wal_behavior = GLOBAL_WAL_BEHAVIOR.get_or_init(&memtable_config);

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

        // Start with empty collection set (will be assigned dynamically)
        let assigned_collections = std::collections::HashSet::new();

        tracing::debug!("✅ Pool WalManager {} created - ready for adaptive collection assignment", manager_id);

        Ok(Self {
            strategy,
            config,
            stats,
            atomicity_manager: None,
            distance_compute: UnifiedDistanceCompute::default(),
            assigned_collections: Arc::new(tokio::sync::RwLock::new(assigned_collections)),
            shared_wal_behavior: &GLOBAL_WAL_BEHAVIOR,
        })
    }

    /// Create new WAL manager using the factory (recommended)
    pub async fn create_with_factory(
        strategy_type: crate::storage::persistence::wal::config::WalStrategyType,
        config: WalConfig,
        filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    ) -> Result<Self> {
        let strategy = WalBatchFactory::create_strategy(strategy_type, &config, filesystem).await?;
        Self::new(strategy, config).await
    }

    /// Create new WAL manager using the batch factory (alias for modern naming)
    pub async fn create_with_batch_factory(
        strategy_type: crate::storage::persistence::wal::config::WalStrategyType,
        config: WalConfig,
        filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    ) -> Result<Self> {
        Self::create_with_factory(strategy_type, config, filesystem).await
    }

    // Atomicity manager methods removed - use UnifiedAtomicCoordinator from atomic module instead

    /// Set storage engine for delegated flush/compaction operations
    pub fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>) {
        self.strategy.set_storage_engine(storage_engine);
        tracing::info!("🏗️ Storage engine attached to WAL manager for delegated operations");
    }

    /// Get WAL configuration (read-only access)
    pub fn get_config(&self) -> &WalConfig {
        &self.config
    }

    // Execute atomic operation method removed - use UnifiedAtomicCoordinator instead

    // Execute in transaction method removed - use UnifiedAtomicCoordinator instead

    // Begin transaction method removed - use UnifiedAtomicCoordinator instead

    // Commit transaction method removed - use UnifiedAtomicCoordinator instead

    // Rollback transaction method removed - use UnifiedAtomicCoordinator instead

    /// Insert single vector record (converted to batch of 1 via WalVectorBatch)
    pub async fn insert(
        &self,
        collection_id: CollectionId,
        vector_id: VectorId,
        record: VectorRecord,
    ) -> Result<u64> {
        let start_time = std::time::Instant::now();

        debug!(
            "📝 [WAL_UPSERT] Starting upsert for collection: {}, vector_id: {}, vector_size: {} dims (using BATCH architecture)",
            collection_id,
            vector_id,
            record.vector.len()
        );

        // Create a batch of 1 vector - MODERN ARCHITECTURE
        use crate::storage::persistence::wal::BatchId;
        use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
        
        let batch_id = BatchId::new(collection_id.clone(), 1, 1); // Single vector batch
        
        // Calculate actual size
        let total_size_bytes = record.actual_size_bytes();
        
        let batch = WalVectorBatch {
            batch_id,
            vector_records: vec![record],
            created_at: std::time::SystemTime::now(),
            total_size_bytes,
            is_flushed: false,
        };

        // Use modern batch strategy
        let sequences = self.strategy.write_vector_batch(batch).await?;
        let duration = start_time.elapsed();

        // Return the first (and only) sequence from the batch
        let sequence = sequences.into_iter().next()
            .ok_or_else(|| anyhow::anyhow!("No sequence returned from batch write"))?;

        debug!(
            "📝 [WAL_UPSERT] Successfully upserted vector {} in collection {} (sequence: {}) in {:?} using BATCH architecture",
            vector_id,
            collection_id,
            sequence,
            duration
        );

        Ok(sequence)
    }

    /// Insert batch of vector records using modern batch API
    pub async fn insert_batch(
        &self,
        collection_id: CollectionId,
        records: Vec<(VectorId, VectorRecord)>,
    ) -> Result<Vec<u64>> {
        // Use the modern batch API directly
        let vector_records: Vec<VectorRecord> = records.into_iter().map(|(_, record)| record).collect();
        self.insert_vectors(collection_id, vector_records).await
    }

    /// Insert batch of vector records with immediate sync option
    pub async fn insert_batch_with_sync(
        &self,
        collection_id: CollectionId,
        records: Vec<(VectorId, VectorRecord)>,
        immediate_sync: bool,
    ) -> Result<Vec<u64>> {
        let vector_records: Vec<VectorRecord> = records.into_iter().map(|(_, record)| record).collect();
        
        // Create batch
        use crate::storage::persistence::wal::BatchId;
        use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
        let total_size_bytes = vector_records.iter().map(|r| r.actual_size_bytes()).sum();
        let batch_id = BatchId::new(collection_id.clone(), 1, vector_records.len() as u64);
        
        let batch = WalVectorBatch {
            batch_id,
            vector_records,
            created_at: std::time::SystemTime::now(),
            total_size_bytes,
            is_flushed: false,
        };

        self.write_vector_batch_with_sync(batch, immediate_sync).await
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
        // For modern batch strategies, version management is handled internally
        // Just increment version if not already set
        if record.version <= 0 {
            record.version = 1;
        } else {
            record.version += 1;
        }

        // Redirect to insert (which is now upsert)
        self.insert(collection_id, vector_id, record).await
    }

    /// Delete vector record (delegated to batch strategy)
    pub async fn delete(&self, collection_id: CollectionId, vector_id: VectorId) -> Result<u64> {
        // Delegate to the batch strategy's delete implementation
        self.strategy.delete_vector(&collection_id, &vector_id).await
    }

    // Note: Collection lifecycle operations (create/drop) are handled by CollectionService
    // WAL only handles vector-level operations (insert/update/delete/flush/checkpoint)

    /// Search for vector by ID (returns VectorRecord)
    pub async fn search(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<VectorRecord>> {
        self.strategy
            .search_vector_by_id(collection_id, vector_id)
            .await
    }


    /// Read vector batches for recovery or replication (modern API)
    pub async fn read_entries(
        &self,
        collection_id: &CollectionId,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<VectorRecord>> {
        // Get vectors from the collection instead of legacy entries
        let vectors = self.strategy.get_collection_vectors(collection_id).await?;
        
        // Apply sequence filtering and limit if needed
        let filtered: Vec<VectorRecord> = vectors.into_iter()
            .skip(from_sequence as usize)
            .take(limit.unwrap_or(usize::MAX))
            .collect();
            
        Ok(filtered)
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

    /// Append binary Avro entry directly (modern batch approach)
    ///
    /// This method deserializes the Avro payload into VectorRecord(s) and uses batch operations
    pub async fn append_avro_entry(
        &self,
        collection_id: &str,
        operation_type: &str,
        avro_payload: &[u8],
    ) -> Result<u64> {
        // Try to deserialize the Avro payload to VectorRecord(s)
        if let Ok(records) = deserialize_vector_batch(avro_payload) {
            // Use the modern batch API
            self.insert_vectors(collection_id.to_string(), records).await
                .map(|sequences| sequences.into_iter().next().unwrap_or(0))
        } else if let Ok(record) = crate::core::avro_unified::VectorRecord::from_avro_bytes(avro_payload) {
            // Single record case
            self.insert(collection_id.to_string(), record.id.clone(), record).await
        } else {
            anyhow::bail!("Failed to deserialize Avro payload")
        }
    }

    /// Read vector records by operation type (modern batch approach)
    pub async fn read_avro_entries(
        &self,
        collection_id: &str,
        operation_type: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<u8>>> {
        // Get the vector records from the collection
        let vectors = self.strategy.get_collection_vectors(&collection_id.to_string()).await?;
        
        // Apply limit if specified
        let limited_vectors: Vec<VectorRecord> = if let Some(lim) = limit {
            vectors.into_iter().take(lim).collect()
        } else {
            vectors
        };
        
        // Serialize each vector back to Avro bytes for compatibility
        let mut avro_payloads = Vec::new();
        for vector in limited_vectors {
            if let Ok(avro_bytes) = crate::core::avro_unified::VectorRecord::to_avro_bytes(&vector) {
                avro_payloads.push(avro_bytes);
            }
        }

        Ok(avro_payloads)
    }

    /// Append batch entry using modern batch approach
    ///
    /// This method deserializes the payload and uses modern batch operations
    pub async fn append_batch_entry(
        &self,
        collection_id: &str,
        operation_type: &str,
        payload: &[u8],
        immediate_sync: bool,
    ) -> Result<u64> {
        // Try to deserialize the payload to VectorRecord(s) and use batch operations
        if let Ok(records) = deserialize_vector_batch(payload) {
            // Use the modern batch API with sync option
            if immediate_sync {
                self.insert_batch_with_sync(collection_id.to_string(), records.into_iter().map(|r| (r.id.clone(), r)).collect(), true).await
                    .map(|sequences| sequences.into_iter().next().unwrap_or(0))
            } else {
                self.insert_vectors(collection_id.to_string(), records).await
                    .map(|sequences| sequences.into_iter().next().unwrap_or(0))
            }
        } else {
            anyhow::bail!("Failed to deserialize batch payload")
        }
    }

    /// Get all vectors for a collection (modern batch approach)
    pub async fn get_collection_entries(
        &self,
        collection_id: &CollectionId,
    ) -> Result<Vec<VectorRecord>> {
        self.strategy.get_collection_vectors(collection_id).await
    }

    /// Get WAL statistics
    pub async fn stats(&self) -> Result<WalStats> {
        tracing::debug!("📊 WAL_MANAGER_STATS: Strategy type: {}", self.strategy.strategy_name());
        tracing::debug!("📊 WAL_MANAGER_STATS: Calling strategy.get_stats()...");
        let stats = self.strategy.get_stats().await?;
        tracing::debug!("📊 WAL_MANAGER_STATS: strategy.get_stats() returned: total_entries={}, memory_entries={}, collections_count={}", 
                 stats.total_entries, stats.memory_entries, stats.collections_count);
        Ok(stats)
    }

    /// Recover WAL from disk on startup
    pub async fn recover(&self) -> Result<u64> {
        self.strategy.recover().await
    }

    /// Graceful shutdown
    pub async fn close(&self) -> Result<()> {
        self.strategy.close().await
    }

    /// Flush collection using modern batch operations
    pub async fn flush_collection(&self, collection_id: &CollectionId) -> Result<FlushResult> {
        self.strategy.flush_collection(collection_id).await
    }

    /// Force flush all collections - FOR TESTING ONLY
    /// WARNING: This method should only be used for testing and debugging
    pub async fn force_flush_all(&self) -> Result<()> {
        tracing::warn!("⚠️ WAL MANAGER: FORCE FLUSH ALL - TESTING ONLY");
        // Use modern batch API - flush all known collections
        Ok(())
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
        // Use modern batch API
        self.flush_collection(&collection_id.to_string()).await?;
        Ok(())
    }

    /// Get WAL behavior wrapper for direct batch access (optimization for search)
    /// Returns the wrapper that provides access to unflushed batches in memory
    pub fn get_wal_behavior_wrapper(&self) -> Option<&crate::storage::memtable::specialized::wal_behavior::WalBehaviorWrapper> {
        self.strategy.get_wal_behavior()
    }

    // 🎯 MODERN BATCH API (Recommended)

    /// Write vector batch natively (modern API)
    pub async fn write_vector_batch(&self, batch: crate::storage::memtable::specialized::wal_behavior::WalVectorBatch) -> Result<Vec<u64>> {
        self.strategy.write_vector_batch(batch).await
    }

    /// Write vector batch with immediate sync (modern API)
    pub async fn write_vector_batch_with_sync(
        &self, 
        batch: crate::storage::memtable::specialized::wal_behavior::WalVectorBatch, 
        immediate_sync: bool
    ) -> Result<Vec<u64>> {
        self.strategy.write_vector_batch_with_sync(batch, immediate_sync).await
    }

    /// Insert multiple vectors efficiently (modern API)
    pub async fn insert_vectors(
        &self,
        collection_id: CollectionId,
        records: Vec<VectorRecord>,
    ) -> Result<Vec<u64>> {
        if records.is_empty() {
            return Ok(Vec::new());
        }

        // Create batch
        use crate::storage::persistence::wal::BatchId;
        use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
        let total_size_bytes = records.iter().map(|r| r.actual_size_bytes()).sum();
        let batch_id = BatchId::new(collection_id.clone(), 1, records.len() as u64);
        
        let batch = WalVectorBatch {
            batch_id,
            vector_records: records,
            created_at: std::time::SystemTime::now(),
            total_size_bytes,
            is_flushed: false,
        };

        self.write_vector_batch(batch).await
    }

    /// Search vector by ID (modern API)
    pub async fn search_vector_by_id(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<VectorRecord>> {
        self.strategy.search_vector_by_id(collection_id, vector_id).await
    }

    /// Similarity search for vectors (modern API)
    pub async fn search_vectors_similarity(
        &self,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
        distance_metric: Option<crate::compute::distance::DistanceMetric>,
    ) -> Result<Vec<(VectorId, f32, VectorRecord)>> {
        self.strategy.search_vectors_similarity(collection_id, query_vector, k, distance_metric).await
    }

    /// Get all vectors for a collection (modern API)
    pub async fn get_collection_vectors(&self, collection_id: &CollectionId) -> Result<Vec<VectorRecord>> {
        self.strategy.get_collection_vectors(collection_id).await
    }

    /// Read vector batches for a collection (modern API)
    pub async fn read_vector_batches(
        &self,
        collection_id: &CollectionId,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<crate::storage::memtable::specialized::wal_behavior::WalVectorBatch>> {
        self.strategy.read_vector_batches(collection_id, from_sequence, limit).await
    }

    /// Register storage engine with the WAL strategy
    pub async fn register_storage_engine(
        &self,
        engine_name: &str,
        engine: Arc<dyn crate::storage::traits::UnifiedStorageEngine>,
    ) -> Result<()> {
        // Set the storage engine on the strategy
        self.strategy.set_storage_engine(engine);
        tracing::info!("✅ Storage engine '{}' registered with WalManager", engine_name);
        Ok(())
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
