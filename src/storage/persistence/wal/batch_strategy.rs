//! Batch-Oriented WAL Strategy (Modern Architecture)
//!
//! This module defines the new WalBatchStrategy trait that replaces the deprecated
//! individual-entry based WalStrategy. The batch-oriented approach provides:
//! - Better performance through batch operations
//! - Zero-copy Avro serialization 
//! - Native batch storage in memtables
//! - Simplified consistency guarantees

use anyhow::{Result, Context};
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

    /// Get filesystem factory for cloud operations
    fn get_filesystem(&self) -> Option<Arc<FilesystemFactory>>;

    /// Set storage engine for delegated flush/compaction operations
    fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>);

    /// Write WAL batch to cloud storage with URL-based routing
    async fn write_batch_to_cloud(
        &self,
        collection_id: &CollectionId,
        batch: &WalVectorBatch,
        cloud_url: &str,
    ) -> Result<String> {
        if let Some(fs) = self.get_filesystem() {
            // Validate URL format before proceeding
            fs.validate_url(cloud_url)
                .context("Invalid cloud URL format")?;
            
            // Serialize batch to bytes
            let batch_bytes = bincode::serialize(batch)
                .context("Failed to serialize batch for cloud storage")?;
            
            // Generate unique filename for the batch with timestamp
            let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
            let batch_filename = format!(
                "wal_batch_{}_{}_{}.bin",
                collection_id,
                timestamp,
                batch.batch_id.batch_uuid
            );
            
            // Construct full cloud URL
            let full_url = if cloud_url.ends_with('/') {
                format!("{}{}", cloud_url, batch_filename)
            } else {
                format!("{}/{}", cloud_url, batch_filename)
            };
            
            // Validate the constructed URL
            fs.validate_url(&full_url)
                .context("Invalid constructed cloud URL")?;
            
            // Get filesystem for URL and write atomically
            let filesystem = fs.get_filesystem(&full_url)
                .context("Failed to get filesystem for cloud URL")?;
            
            let path = fs.extract_path_from_url(&full_url)
                .context("Failed to extract path from cloud URL")?;
            
            let options = Some(crate::storage::persistence::filesystem::FileOptions {
                create_dirs: true,
                overwrite: true,
                ..Default::default()
            });
            
            filesystem.write_atomic(&path, &batch_bytes, options).await
                .context("Failed to write batch to cloud storage")?;
            
            // Log detailed information for monitoring
            let bucket = fs.extract_bucket_from_url(&full_url)
                .unwrap_or_default()
                .unwrap_or_else(|| "unknown".to_string());
            
            tracing::info!(
                "☁️ CLOUD_WRITE: Wrote batch {} ({} bytes) to {} [bucket: {}]",
                batch.batch_id.batch_uuid,
                batch_bytes.len(),
                full_url,
                bucket
            );
            
            Ok(full_url)
        } else {
            Err(anyhow::anyhow!("Filesystem not initialized for cloud operations"))
        }
    }

    /// Read WAL batch from cloud storage with URL-based routing
    async fn read_batch_from_cloud(
        &self,
        cloud_url: &str,
    ) -> Result<WalVectorBatch> {
        if let Some(fs) = self.get_filesystem() {
            // Validate URL format before proceeding
            fs.validate_url(cloud_url)
                .context("Invalid cloud URL format")?;
            
            let filesystem = fs.get_filesystem(cloud_url)
                .context("Failed to get filesystem for cloud URL")?;
            
            let path = fs.extract_path_from_url(cloud_url)
                .context("Failed to extract path from cloud URL")?;
            
            let batch_bytes = filesystem.read(&path).await
                .context("Failed to read batch from cloud storage")?;
            
            let batch: WalVectorBatch = bincode::deserialize(&batch_bytes)
                .context("Failed to deserialize batch from cloud storage")?;
            
            // Log detailed information for monitoring
            let bucket = fs.extract_bucket_from_url(cloud_url)
                .unwrap_or_default()
                .unwrap_or_else(|| "unknown".to_string());
            
            tracing::info!(
                "☁️ CLOUD_READ: Read batch {} ({} bytes) from {} [bucket: {}]",
                batch.batch_id.batch_uuid,
                batch_bytes.len(),
                cloud_url,
                bucket
            );
            
            Ok(batch)
        } else {
            Err(anyhow::anyhow!("Filesystem not initialized for cloud operations"))
        }
    }

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

    /// Migrate WAL batch from local to cloud storage
    async fn migrate_batch_to_cloud(
        &self,
        collection_id: &CollectionId,
        batch: &WalVectorBatch,
        local_path: &str,
        cloud_url: &str,
    ) -> Result<String> {
        if let Some(fs) = self.get_filesystem() {
            // Write to cloud first
            let cloud_batch_url = self.write_batch_to_cloud(collection_id, batch, cloud_url).await?;
            
            // Verify cloud write by reading back
            let _verified_batch = self.read_batch_from_cloud(&cloud_batch_url).await
                .context("Failed to verify cloud write during migration")?;
            
            // Remove local file after successful cloud write
            let local_fs = fs.get_filesystem(&format!("file://{}", local_path))
                .context("Failed to get local filesystem")?;
            
            local_fs.delete(local_path).await
                .context("Failed to delete local file after migration")?;
            
            tracing::info!(
                "🔄 MIGRATION: Migrated batch {} from {} to {}",
                batch.batch_id.batch_uuid,
                local_path,
                cloud_batch_url
            );
            
            Ok(cloud_batch_url)
        } else {
            Err(anyhow::anyhow!("Filesystem not initialized for migration"))
        }
    }

    /// List WAL batches from cloud storage with URL-based routing
    async fn list_cloud_batches(
        &self,
        collection_id: &CollectionId,
        cloud_base_url: &str,
    ) -> Result<Vec<String>> {
        if let Some(fs) = self.get_filesystem() {
            // Validate URL format before proceeding
            fs.validate_url(cloud_base_url)
                .context("Invalid cloud base URL format")?;
            
            let filesystem = fs.get_filesystem(cloud_base_url)
                .context("Failed to get filesystem for cloud URL")?;
            
            let base_path = fs.extract_path_from_url(cloud_base_url)
                .context("Failed to extract path from cloud URL")?;
            
            let entries = filesystem.list(&base_path).await
                .context("Failed to list cloud directory")?;
            
            // Filter for WAL batch files for this collection with multiple patterns
            let batch_prefix = format!("wal_batch_{}_", collection_id);
            let batch_urls: Vec<String> = entries
                .iter()
                .filter(|entry| {
                    !entry.metadata.is_directory && 
                    entry.name.starts_with(&batch_prefix) &&
                    entry.name.ends_with(".bin")
                })
                .map(|entry| {
                    if cloud_base_url.ends_with('/') {
                        format!("{}{}", cloud_base_url, entry.name)
                    } else {
                        format!("{}/{}", cloud_base_url, entry.name)
                    }
                })
                .collect();
            
            // Log detailed information for monitoring
            let bucket = fs.extract_bucket_from_url(cloud_base_url)
                .unwrap_or_default()
                .unwrap_or_else(|| "unknown".to_string());
            
            tracing::debug!(
                "☁️ CLOUD_LIST: Found {} WAL batches for collection {} in {} [bucket: {}]",
                batch_urls.len(),
                collection_id,
                cloud_base_url,
                bucket
            );
            
            Ok(batch_urls)
        } else {
            Err(anyhow::anyhow!("Filesystem not initialized for cloud operations"))
        }
    }

    /// Delete WAL batch from cloud storage
    async fn delete_cloud_batch(
        &self,
        cloud_url: &str,
    ) -> Result<()> {
        if let Some(fs) = self.get_filesystem() {
            // Validate URL format before proceeding
            fs.validate_url(cloud_url)
                .context("Invalid cloud URL format")?;
            
            let filesystem = fs.get_filesystem(cloud_url)
                .context("Failed to get filesystem for cloud URL")?;
            
            let path = fs.extract_path_from_url(cloud_url)
                .context("Failed to extract path from cloud URL")?;
            
            filesystem.delete(&path).await
                .context("Failed to delete batch from cloud storage")?;
            
            // Log detailed information for monitoring
            let bucket = fs.extract_bucket_from_url(cloud_url)
                .unwrap_or_default()
                .unwrap_or_else(|| "unknown".to_string());
            
            tracing::info!("🗑️ CLOUD_DELETE: Deleted batch from {} [bucket: {}]", cloud_url, bucket);
            
            Ok(())
        } else {
            Err(anyhow::anyhow!("Filesystem not initialized for cloud operations"))
        }
    }

    /// Check if cloud storage is available and accessible
    async fn check_cloud_health(
        &self,
        cloud_base_url: &str,
    ) -> Result<bool> {
        if let Some(fs) = self.get_filesystem() {
            // Validate URL format before proceeding
            match fs.validate_url(cloud_base_url) {
                Ok(_) => {},
                Err(e) => {
                    tracing::warn!("❌ CLOUD_HEALTH: Invalid URL format {}: {}", cloud_base_url, e);
                    return Ok(false);
                }
            }
            
            let filesystem = fs.get_filesystem(cloud_base_url)
                .context("Failed to get filesystem for cloud URL")?;
            
            let base_path = fs.extract_path_from_url(cloud_base_url)
                .context("Failed to extract path from cloud URL")?;
            
            // Try to list the directory to check accessibility
            match filesystem.list(&base_path).await {
                Ok(_) => {
                    // Log detailed information for monitoring
                    let bucket = fs.extract_bucket_from_url(cloud_base_url)
                        .unwrap_or_default()
                        .unwrap_or_else(|| "unknown".to_string());
                    
                    tracing::debug!("✅ CLOUD_HEALTH: Cloud storage accessible at {} [bucket: {}]", cloud_base_url, bucket);
                    Ok(true)
                }
                Err(e) => {
                    tracing::warn!("❌ CLOUD_HEALTH: Cloud storage not accessible at {}: {}", cloud_base_url, e);
                    Ok(false)
                }
            }
        } else {
            tracing::warn!("❌ CLOUD_HEALTH: Filesystem not initialized");
            Ok(false)
        }
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
            
            // Cleanup disk WAL files for the flushed batches
            if let Some(fs) = self.get_filesystem() {
                for batch_id in &flush_cycle.batch_ids {
                    // Try to clean up local WAL files if they exist
                    let local_wal_path = format!("wal_batch_{}_{}.bin", 
                        flush_cycle.collection_id, batch_id.batch_uuid);
                    
                    if let Ok(local_fs) = fs.get_filesystem(&format!("file://{}", local_wal_path)) {
                        let _ = local_fs.delete(&local_wal_path).await; // Ignore errors - file might not exist
                    }
                }
            }
            
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

    /// Insert vectors with cloud backup option
    async fn insert_vectors_with_cloud_backup(
        &self,
        collection_id: CollectionId,
        vector_records: Vec<VectorRecord>,
        cloud_backup_url: Option<&str>,
    ) -> Result<Vec<u64>> {
        // Insert vectors normally
        let sequences = self.insert_vectors(collection_id.clone(), vector_records.clone()).await?;
        
        // If cloud backup is enabled, also write to cloud
        if let Some(cloud_url) = cloud_backup_url {
            let batch_id = super::BatchId::new(
                collection_id.clone(), 
                1, 
                vector_records.len() as u64
            );
            
            let total_size_bytes: usize = vector_records.iter()
                .map(|r| r.actual_size_bytes())
                .sum();
            
            let batch = WalVectorBatch {
                batch_id,
                vector_records,
                created_at: std::time::SystemTime::now(),
                total_size_bytes,
                is_flushed: false,
            };
            
            // Write to cloud as backup (fire and forget)
            let cloud_result = self.write_batch_to_cloud(&collection_id, &batch, cloud_url).await;
            if let Err(e) = cloud_result {
                tracing::warn!("Failed to write batch to cloud backup: {}", e);
            }
        }
        
        Ok(sequences)
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