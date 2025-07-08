//! Batch-Oriented WAL Strategy (Modern Architecture)
//!
//! This module defines the new WalBatchStrategy trait that replaces the deprecated
//! individual-entry based WalStrategy. The batch-oriented approach provides:
//! - Better performance through batch operations
//! - Zero-copy Avro serialization 
//! - Native batch storage in memtables
//! - Simplified consistency guarantees

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use crate::compute::distance::DistanceMetric as CoreDistanceMetric;
use crate::compute::unified_distance::{DistanceComputeProvider, UnifiedDistanceCompute};
use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::UnifiedStorageEngine;

use super::{WalConfig, WalStats};
use crate::storage::traits::FlushResult;

/// Modern batch-oriented WAL strategy trait
/// 
/// This trait focuses on batch operations for optimal performance:
/// - All vector operations work with WalVectorBatch
/// - No individual entry operations (use batches of size 1)
/// - Direct integration with native batch storage
/// - Simplified API surface
#[async_trait]
pub trait WalBatchStrategy: Send + Sync + DistanceComputeProvider + std::fmt::Debug {
    /// Strategy name for identification and logging
    fn strategy_name(&self) -> &'static str;

    /// Initialize the strategy with configuration
    async fn initialize(
        &mut self,
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<()>;

    /// Set storage engine for delegated flush/compaction operations
    fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>);

    // 🎯 CORE BATCH OPERATIONS (Modern Architecture)

    /// Write Avro bytes directly (zero-copy optimization for Avro, convert for other formats)
    /// This is the optimal write operation that accepts pre-serialized Avro from Vector Service
    async fn write_avro_batch(
        &self, 
        collection_id: &CollectionId,
        avro_bytes: &[u8]
    ) -> Result<super::WalOperation>;

    /// Write vector batch atomically (memory + disk) - Legacy interface
    /// This is kept for backward compatibility but write_avro_batch is preferred
    async fn write_vector_batch(&self, batch: WalVectorBatch) -> Result<Vec<u64>>;

    /// Write vector batch with immediate disk sync for durability
    async fn write_vector_batch_with_sync(
        &self, 
        batch: WalVectorBatch, 
        immediate_sync: bool
    ) -> Result<Vec<u64>>;

    /// Read vector batches for a collection starting from sequence
    async fn read_vector_batches(
        &self,
        collection_id: &CollectionId,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<WalVectorBatch>>;

    /// Search vector by ID within a collection
    async fn search_vector_by_id(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<VectorRecord>>;

    /// Similarity search for vectors in WAL with configurable distance metric
    async fn search_vectors_similarity(
        &self,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
        distance_metric: Option<CoreDistanceMetric>,
    ) -> Result<Vec<(VectorId, f32, VectorRecord)>>;

    // 🎯 COLLECTION MANAGEMENT

    /// Get all vector records for a collection (for flush operations)
    async fn get_collection_vectors(&self, collection_id: &CollectionId) -> Result<Vec<VectorRecord>>;

    /// Flush collection to storage (delegates to storage engine)
    async fn flush_collection(&self, collection_id: &CollectionId) -> Result<FlushResult>;

    /// Drop all data for a collection
    async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()>;

    // 🎯 STATISTICS AND MONITORING

    /// Get comprehensive WAL statistics
    async fn get_stats(&self) -> Result<WalStats>;

    /// Get statistics for a specific collection
    async fn get_collection_stats(&self, collection_id: &CollectionId) -> Result<WalStats>;

    // 🎯 LIFECYCLE MANAGEMENT

    /// Recover from disk on startup
    async fn recover(&self) -> Result<u64>;

    /// Close and cleanup resources
    async fn close(&self) -> Result<()>;

    /// Force immediate sync of in-memory data to disk
    async fn force_sync(&self, collection_id: Option<&CollectionId>) -> Result<()>;

    // 🎯 ADVANCED OPERATIONS

    /// Compact collection (clean up old MVCC versions, TTL expired entries)
    async fn compact_collection(&self, collection_id: &CollectionId) -> Result<u64>;

    /// Get WAL behavior wrapper for specialized operations
    fn get_wal_behavior(&self) -> Option<&crate::storage::memtable::specialized::wal_behavior::WalBehaviorWrapper> {
        None // Default implementation - concrete strategies can override
    }

    // 🎯 ADDITIONAL BATCH OPERATIONS

    /// Delete vector by ID using batch operations
    async fn delete_vector(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<u64> {
        // Create a tombstone vector record for deletion
        let tombstone = VectorRecord {
            id: vector_id.clone(),
            collection_id: collection_id.clone(),
            vector: vec![], // Empty vector for tombstone
            metadata: std::collections::HashMap::new(),
            timestamp: chrono::Utc::now().timestamp_micros(),
            created_at: chrono::Utc::now().timestamp_micros(),
            updated_at: chrono::Utc::now().timestamp_micros(),
            expires_at: Some(chrono::Utc::now().timestamp_micros() + (30 * 24 * 60 * 60 * 1_000_000)), // 30 days
            version: -1, // Negative version indicates deletion
            rank: None,
            score: None,
            distance: None,
        };

        // Create single-vector batch for deletion
        use super::BatchId;
        let batch_id = BatchId::new(collection_id.clone(), 1, 1);
        let batch = WalVectorBatch {
            batch_id,
            vector_records: vec![tombstone],
            created_at: std::time::SystemTime::now(),
            total_size_bytes: std::mem::size_of::<VectorRecord>(),
            is_flushed: false,
        };

        let sequences = self.write_vector_batch(batch).await?;
        Ok(sequences.into_iter().next().unwrap_or(0))
    }

    /// Flush collections using batch operations
    async fn flush(&self, collection_id: Option<&CollectionId>) -> Result<FlushResult> {
        if let Some(cid) = collection_id {
            self.flush_collection(cid).await
        } else {
            // Flush all collections - default implementation
            Ok(FlushResult {
                success: true,
                collections_affected: vec![],
                entries_flushed: 0,
                bytes_written: 0,
                files_created: 0,
                duration_ms: 0,
                completed_at: chrono::Utc::now(),
                engine_metrics: std::collections::HashMap::new(),
                compaction_triggered: false,
                flushed_batch_ids: vec![],
            })
        }
    }

    /// Atomically retrieve and mark WAL batches for flush operation
    /// 
    /// This method:
    /// 1. Retrieves unflushed batches from GlobalPartitionedMemtable with deserialized data
    /// 2. Marks batches for flush to prevent concurrent access
    /// 3. Returns batch data with BatchIds for atomic cleanup after successful flush
    /// 4. Prepares for disk WAL file cleanup upon flush completion
    async fn atomic_retrieve_for_flush(
        &self,
        collection_id: &CollectionId,
        flush_id: &str,
    ) -> Result<super::FlushCycle> {
        // Get WAL behavior wrapper to access GlobalPartitionedMemtable
        if let Some(wal_behavior) = self.get_wal_behavior() {
            // Retrieve unflushed batches from global memtable (already deserialized)
            let unflushed_batches = wal_behavior.get_unflushed_batches(collection_id).await?;
            
            // Extract vector records and batch IDs for atomic operations
            let mut all_vector_records = Vec::new();
            let mut batch_ids = Vec::new();
            let mut marked_sequences = Vec::new();
            
            for batch in &unflushed_batches {
                all_vector_records.extend(batch.vector_records.clone());
                batch_ids.push(batch.batch_id.clone());
                marked_sequences.push(batch.batch_id.sequence_range);
            }
            
            tracing::info!(
                "🔄 Atomic flush retrieval: {} batches, {} vectors for collection {} (flush_id: {})",
                unflushed_batches.len(),
                all_vector_records.len(),
                collection_id,
                flush_id
            );
            
            // Create flush cycle with batch-oriented data
            Ok(super::FlushCycle {
                flush_id: flush_id.to_string(),
                collection_id: collection_id.clone(),
                entries: vec![], // Legacy entries - not used in batch architecture
                vector_records: all_vector_records,
                marked_segments: vec![], // Will be populated with disk WAL file paths for cleanup
                marked_sequences,
                batch_ids,
                state: super::FlushCycleState::Active,
            })
        } else {
            // Fallback for strategies without WAL behavior wrapper
            let vector_records = self.get_collection_vectors(collection_id).await?;
            let record_count = vector_records.len() as u64;
            
            Ok(super::FlushCycle {
                flush_id: flush_id.to_string(),
                collection_id: collection_id.clone(),
                entries: vec![], // Legacy entries - not used in batch architecture
                vector_records,
                marked_segments: vec![],
                marked_sequences: vec![(0, record_count)],
                batch_ids: vec![],
                state: super::FlushCycleState::Active,
            })
        }
    }

    /// Complete flush cycle - cleanup GlobalPartitionedMemtable and disk WAL files
    /// Called after successful storage engine flush to atomically clean up WAL data
    async fn complete_flush_cycle(&self, flush_cycle: super::FlushCycle) -> Result<super::FlushCompletionResult> {
        if let Some(wal_behavior) = self.get_wal_behavior() {
            // Atomically clear flushed batches from GlobalPartitionedMemtable
            let cleared_count = wal_behavior.clear_flushed(&flush_cycle.collection_id, u64::MAX).await?;
            
            // TODO: Cleanup disk WAL files for the flushed batches
            // This would delete the corresponding WAL segment files on disk
            
            tracing::info!(
                "✅ Flush completion: {} batches cleared from memtable for collection {} (flush_id: {})",
                cleared_count,
                flush_cycle.collection_id,
                flush_cycle.flush_id
            );
            
            Ok(super::FlushCompletionResult {
                entries_removed: cleared_count,
                segments_cleaned: flush_cycle.marked_segments.len(),
                bytes_reclaimed: flush_cycle.vector_records.iter().map(|v| v.actual_size_bytes() as u64).sum(),
            })
        } else {
            // Fallback for strategies without WAL behavior wrapper
            Ok(super::FlushCompletionResult {
                entries_removed: flush_cycle.vector_records.len(),
                segments_cleaned: 0,
                bytes_reclaimed: flush_cycle.vector_records.iter().map(|v| v.actual_size_bytes() as u64).sum(),
            })
        }
    }

    /// Check if collection needs flush based on thresholds (called during writes)
    /// Returns true if flush should be triggered for the collection
    async fn should_trigger_flush(&self, collection_id: &CollectionId) -> Result<bool> {
        if let Some(wal_behavior) = self.get_wal_behavior() {
            // Get collection statistics from GlobalPartitionedMemtable
            let stats = wal_behavior.get_stats().await?;
            
            if let Some(collection_stats) = stats.get(collection_id) {
                // Check thresholds: memory size, entry count, or time-based
                let memory_threshold_mb = 100; // 100MB threshold
                let entry_threshold = 10000; // 10K entries threshold
                
                let should_flush = collection_stats.memory_size_bytes > (memory_threshold_mb * 1024 * 1024) ||
                                 collection_stats.total_entries > entry_threshold;
                
                if should_flush {
                    tracing::info!(
                        "🚨 Flush threshold reached for collection {}: {} MB, {} entries",
                        collection_id,
                        collection_stats.memory_size_bytes / (1024 * 1024),
                        collection_stats.total_entries
                    );
                }
                
                Ok(should_flush)
            } else {
                Ok(false) // No data for collection
            }
        } else {
            Ok(false) // No WAL behavior wrapper
        }
    }
}

/// Convenience methods for common operations
pub trait WalBatchStrategyExt: WalBatchStrategy {
    /// Insert single vector (creates batch of size 1)
    async fn insert_vector(
        &self,
        collection_id: CollectionId,
        vector_record: VectorRecord,
    ) -> Result<u64> {
        use super::BatchId;
        
        // Create single-vector batch
        let batch_id = BatchId::new(collection_id.clone(), 1, 1);
        let total_size_bytes = vector_record.actual_size_bytes();
        
        let batch = WalVectorBatch {
            batch_id,
            vector_records: vec![vector_record],
            created_at: std::time::SystemTime::now(),
            total_size_bytes,
            is_flushed: false,
        };

        let sequences = self.write_vector_batch(batch).await?;
        Ok(sequences.into_iter().next().unwrap_or(0))
    }

    /// Insert multiple vectors efficiently
    async fn insert_vectors(
        &self,
        collection_id: CollectionId,
        vector_records: Vec<VectorRecord>,
    ) -> Result<Vec<u64>> {
        use super::BatchId;
        
        if vector_records.is_empty() {
            return Ok(Vec::new());
        }
        
        // Calculate total size
        let total_size_bytes: usize = vector_records.iter()
            .map(|r| r.actual_size_bytes())
            .sum();
        
        // Create multi-vector batch
        let batch_id = BatchId::new(
            collection_id.clone(), 
            1, 
            vector_records.len() as u64
        );
        
        let batch = WalVectorBatch {
            batch_id,
            vector_records,
            created_at: std::time::SystemTime::now(),
            total_size_bytes,
            is_flushed: false,
        };

        self.write_vector_batch(batch).await
    }
}

// Blanket implementation of convenience methods for all batch strategies
impl<T: WalBatchStrategy> WalBatchStrategyExt for T {}

// 🚫 REMOVED: LegacyWalStrategyAdapter no longer needed - WalStrategy trait removed
// All code now uses WalBatchStrategy with single-entry batches for individual operations
/*
pub struct LegacyWalStrategyAdapter {
    legacy_strategy: Box<dyn super::WalStrategy>,
}*/

/*
impl LegacyWalStrategyAdapter {
    pub fn new(legacy_strategy: Box<dyn super::WalStrategy>) -> Self {
        Self { legacy_strategy }
    }
}

#[async_trait]
impl WalBatchStrategy for LegacyWalStrategyAdapter {
    fn strategy_name(&self) -> &'static str {
        "LegacyAdapter"
    }

    async fn initialize(
        &mut self,
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<()> {
        #[allow(deprecated)]
        self.legacy_strategy.initialize(config, filesystem).await
    }

    fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>) {
        self.legacy_strategy.set_storage_engine(storage_engine);
    }

    async fn write_vector_batch(&self, batch: WalVectorBatch) -> Result<Vec<u64>> {
        // Convert batch to individual entries for legacy strategy
        #[allow(deprecated)]
        self.legacy_strategy.write_vector_batch(batch).await
    }

    async fn write_vector_batch_with_sync(
        &self, 
        batch: WalVectorBatch, 
        immediate_sync: bool
    ) -> Result<Vec<u64>> {
        #[allow(deprecated)]
        self.legacy_strategy.write_vector_batch_with_sync(batch, immediate_sync).await
    }

    async fn read_vector_batches(
        &self,
        collection_id: &CollectionId,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<WalVectorBatch>> {
        #[allow(deprecated)]
        self.legacy_strategy.read_vector_batches(collection_id, from_sequence, limit).await
    }

    async fn search_vector_by_id(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<VectorRecord>> {
        #[allow(deprecated)]
        self.legacy_strategy.search_vector_by_id(collection_id, vector_id).await
    }

    async fn search_vectors_similarity(
        &self,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
        distance_metric: Option<CoreDistanceMetric>,
    ) -> Result<Vec<(VectorId, f32, VectorRecord)>> {
        #[allow(deprecated)]
        let results = self.legacy_strategy.search_vectors_similarity(collection_id, query_vector, k, distance_metric).await?;
        
        // Convert from (VectorId, f32, WalEntry) to (VectorId, f32, VectorRecord)
        let mut converted_results = Vec::new();
        for (vector_id, score, entry) in results {
            if let Ok(vector_record) = entry.extract_vector_record() {
                converted_results.push((vector_id, score, vector_record));
            }
        }
        Ok(converted_results)
    }

    async fn get_collection_vectors(&self, collection_id: &CollectionId) -> Result<Vec<VectorRecord>> {
        #[allow(deprecated)]
        let entries = self.legacy_strategy.get_collection_entries(collection_id).await?;
        
        let mut vectors = Vec::new();
        for entry in entries {
            if let Ok(vector_record) = entry.extract_vector_record() {
                vectors.push(vector_record);
            }
        }
        Ok(vectors)
    }

    async fn flush_collection(&self, collection_id: &CollectionId) -> Result<FlushResult> {
        #[allow(deprecated)]
        self.legacy_strategy.flush(Some(collection_id)).await
    }

    async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()> {
        #[allow(deprecated)]
        self.legacy_strategy.drop_collection(collection_id).await
    }

    async fn get_stats(&self) -> Result<WalStats> {
        #[allow(deprecated)]
        self.legacy_strategy.get_stats().await
    }

    async fn get_collection_stats(&self, _collection_id: &CollectionId) -> Result<WalStats> {
        // Legacy strategies don't support per-collection stats, return global stats
        self.get_stats().await
    }

    async fn recover(&self) -> Result<u64> {
        #[allow(deprecated)]
        self.legacy_strategy.recover().await
    }

    async fn close(&self) -> Result<()> {
        #[allow(deprecated)]
        self.legacy_strategy.close().await
    }

    async fn force_sync(&self, collection_id: Option<&CollectionId>) -> Result<()> {
        #[allow(deprecated)]
        self.legacy_strategy.force_sync(collection_id).await
    }

    async fn compact_collection(&self, collection_id: &CollectionId) -> Result<u64> {
        #[allow(deprecated)]
        self.legacy_strategy.compact_collection(collection_id).await
    }
}

impl DistanceComputeProvider for LegacyWalStrategyAdapter {
    fn distance_compute(&self) -> &UnifiedDistanceCompute {
        self.legacy_strategy.distance_compute()
    }
}
*/