//! WAL Behavior Wrapper with Batch Coordination
//!
//! Extends GlobalPartitionedMemtable with Write Buffer-specific behaviors using composition:
//! - Batch coordination for WAL consistency  
//! - Sequential write optimizations for compression
//! - MVCC support for recovery consistency
//! - Ordered flush operations for RLE/dictionary encodings
//! - Specialized serialization strategies

use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::compute::distance::DistanceMetric as CoreDistanceMetric;
use crate::core::VectorRecord;
use crate::storage::memtable::core::MemtableConfig;
use crate::storage::memtable::implementations::global_partitioned::GlobalPartitionedMemtable;
use crate::storage::persistence::write_buffer::{BatchId, WriteBufferOperation, WriteBufferStats};

/// Write Buffer-specific vector batch for tracking deserialized data
#[derive(Debug, Clone)]
pub struct WriteBufferVectorBatch {
    /// Batch coordination ID
    pub batch_id: BatchId,
    /// Deserialized vector records (ready for search/flush)
    /// Using Arc for zero-copy sharing across WAL strategies
    pub vector_records: Arc<Vec<VectorRecord>>,
    /// Batch metadata
    pub created_at: std::time::SystemTime,
    pub total_size_bytes: usize,
    pub is_flushed: bool,
}

/// Write Buffer-specific batch coordinator
#[derive(Debug)]
struct BatchCoordinator {
    /// Active batches by collection (collection_id -> batch_id -> WriteBufferVectorBatch)
    batches: HashMap<String, HashMap<String, WriteBufferVectorBatch>>,
    /// Individual vector index for fast search (vector_id -> (collection_id, batch_id, index_in_batch))
    vector_index: HashMap<String, (String, String, usize)>,
}

impl BatchCoordinator {
    fn new() -> Self {
        Self {
            batches: HashMap::new(),
            vector_index: HashMap::new(),
        }
    }

    /// Add a vector batch to the coordinator
    fn add_batch(&mut self, collection_id: &str, batch: WriteBufferVectorBatch) -> Result<()> {
        let batch_id = batch.batch_id.to_base62();
        
        // Update vector index
        for (index, vector_record) in batch.vector_records.iter().enumerate() {
            self.vector_index.insert(
                vector_record.id.as_deref().unwrap_or("").to_string(),
                (collection_id.to_string(), batch_id.clone(), index),
            );
        }

        // Store batch
        self.batches
            .entry(collection_id.to_string())
            .or_insert_with(HashMap::new)
            .insert(batch_id, batch);

        Ok(())
    }

    /// Get all unflushed batches for a collection
    fn get_unflushed_batches(&self, collection_id: &str) -> Vec<&WriteBufferVectorBatch> {
        if let Some(collection_batches) = self.batches.get(collection_id) {
            collection_batches
                .values()
                .filter(|batch| !batch.is_flushed)
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Mark batch as flushed
    fn mark_batch_flushed(&mut self, collection_id: &str, batch_id: &str) -> Result<()> {
        if let Some(collection_batches) = self.batches.get_mut(collection_id) {
            if let Some(batch) = collection_batches.get_mut(batch_id) {
                batch.is_flushed = true;
                tracing::debug!("✅ Marked batch {} as flushed", batch_id);
                return Ok(());
            }
        }
        Err(anyhow::anyhow!("Batch {}:{} not found", collection_id, batch_id))
    }

    /// Clear flushed batches from coordinator
    fn clear_flushed_batches(&mut self, collection_id: &str) -> Result<usize> {
        let mut cleared_count = 0;

        if let Some(collection_batches) = self.batches.get_mut(collection_id) {
            // OPTIMIZATION: Use retain instead of collect+remove to avoid extra allocation
            // First collect IDs from flushed batches for index cleanup
            let mut cleared_batch_records = Vec::new();
            for batch in collection_batches.values() {
                if batch.is_flushed {
                    cleared_batch_records.extend(batch.vector_records.iter().map(|v| v.id.as_deref().unwrap_or("").to_string()));
                }
            }
            
            let original_count = collection_batches.len();
            collection_batches.retain(|_, batch| !batch.is_flushed);

            // Remove vector index entries for cleared batches
            for vector_id in cleared_batch_records {
                self.vector_index.remove(&vector_id);
            }
            cleared_count = original_count - collection_batches.len();
        }

        tracing::debug!("🧹 Cleared {} flushed batches from collection {}", cleared_count, collection_id);
        Ok(cleared_count)
    }
}

/// Write Buffer-specific behavior wrapper around global partitioned memtable implementation
///
/// This uses a global partitioned memtable that supports:
/// - Global sequence ordering for flush coordination
/// - Per-collection data partitions for efficient operations
/// - Content-based search within collections on unflushed data
#[derive(Debug, Clone)]
pub struct WriteBufferBehaviorWrapper {
    /// The wrapped global partitioned memtable implementation (generic storage) - Arc for memory efficiency
    inner: Arc<GlobalPartitionedMemtable>,

    /// Write Buffer-specific batch coordinator (handles deserialized batches)
    batch_coordinator: Arc<RwLock<BatchCoordinator>>,

    /// Write Buffer-specific configuration
    config: MemtableConfig,

    /// Sequence number generator for WAL entries (Arc for Clone)
    sequence_generator: Arc<AtomicU64>,

    /// MVCC tracking: vector_id -> [sequences] - Not needed for WAL, but kept for compatibility
    mvcc_versions: Arc<RwLock<std::collections::HashMap<String, Vec<u64>>>>,

    /// Write Buffer-specific metrics
    wal_metrics: Arc<RwLock<WriteBufferMetrics>>,

    /// Flush coordination state
    flush_state: Arc<RwLock<FlushState>>,
}

impl WriteBufferBehaviorWrapper {
    /// Create new WAL behavior wrapper with global partitioned memtable
    pub fn new(config: MemtableConfig) -> Self {
        Self {
            inner: Arc::new(GlobalPartitionedMemtable::new()),
            batch_coordinator: Arc::new(RwLock::new(BatchCoordinator::new())),
            config,
            sequence_generator: Arc::new(AtomicU64::new(1)),
            mvcc_versions: Arc::new(RwLock::new(std::collections::HashMap::new())),
            wal_metrics: Arc::new(RwLock::new(WriteBufferMetrics::default())),
            flush_state: Arc::new(RwLock::new(FlushState::default())),
        }
    }

    /// Get the wrapped implementation
    pub fn inner(&self) -> &GlobalPartitionedMemtable {
        &*self.inner
    }

    /// Get next sequence number for WAL ordering
    pub fn next_sequence(&self) -> u64 {
        let old_value = self.sequence_generator.load(Ordering::SeqCst);
        let new_value = self.sequence_generator.fetch_add(1, Ordering::SeqCst);
        tracing::info!(
            "🔍 SEQUENCE_TRACE: next_sequence() - old_value={}, returned_value={}, new_current={}",
            old_value,
            new_value,
            old_value + 1
        );
        new_value
    }

    /// Get current sequence number without incrementing
    pub fn current_sequence(&self) -> u64 {
        self.sequence_generator.load(Ordering::SeqCst)
    }

    // WalEntry and extract_vector_id removed - use WriteBufferVectorBatch for batch operations

    /// Add WriteBufferOperation with single deserialization (OPTIMAL: single CPU deserialize for all strategies)
    /// This deserializes the payload once and creates WriteBufferVectorBatch for storage
    pub async fn add_wal_operation(&self, collection_id: &str, operation: crate::storage::persistence::write_buffer::WriteBufferOperation) -> Result<Vec<u64>> {
        tracing::debug!(
            "🔄 WAL_BEHAVIOR: Single deserialization for {} format with {} vectors",
            operation.payload_format,
            operation.vector_count
        );

        // Single point of deserialization - leverage this for ALL strategies
        let vector_records = match operation.payload_format.as_str() {
            "avro" => {
                // Use centralized Avro deserializer
                // Use the avro serializer for deserialization
                use crate::storage::persistence::write_buffer::serialization::{AvroSerializer, VectorBatchSerializer};
                let serializer = AvroSerializer::new();
                serializer.deserialize_batch(&operation.payload_data)?
            }
            "bincode" => {
                // Use Bincode deserializer
                bincode::deserialize::<Vec<crate::core::VectorRecord>>(&operation.payload_data)
                    .map_err(|e| anyhow::anyhow!("Failed to deserialize Bincode payload: {}", e))?
            }
            format => {
                anyhow::bail!("Unsupported payload format: {}", format);
            }
        };

        // Create WriteBufferVectorBatch from deserialized records
        let batch = WriteBufferVectorBatch {
            batch_id: crate::storage::persistence::write_buffer::BatchId::new(),
            vector_records: Arc::new(vector_records),
            created_at: std::time::SystemTime::now(),
            total_size_bytes: operation.payload_data.len(),
            is_flushed: false,
        };

        tracing::debug!(
            "✅ WAL_BEHAVIOR: Single deserialization complete, storing {} vectors",
            batch.vector_records.len()
        );

        // Use existing batch storage
        self.add_vector_batch(collection_id, batch).await
    }

    /// Unified batch addition method - STREAMLINED ARCHITECTURE (stores entire batch natively)
    /// This is used when WriteBufferVectorBatch is already deserialized
    pub async fn add_vector_batch(&self, collection_id: &str, batch: WriteBufferVectorBatch) -> Result<Vec<u64>> {
        let batch_id = batch.batch_id.to_base62();
        let vector_count = batch.vector_records.len();

        tracing::debug!(
            "🚀 WAL_BEHAVIOR: Starting add_vector_batch for batch {} to collection {} ({} vectors)",
            batch_id,
            collection_id,
            vector_count
        );
        
        tracing::debug!(
            "🚀 WAL_BEHAVIOR: Batch size info - total_size_bytes: {}, vector_count: {}",
            batch.total_size_bytes,
            vector_count
        );

        // STREAMLINED: Store batch natively in GlobalPartitionedMemtable (no duplication)
        tracing::debug!("🚀 WAL_BEHAVIOR: Calling inner.add_wal_batch()...");
        let sequences = self.inner.add_wal_batch(collection_id, batch.clone()).await?;
        tracing::debug!("🚀 WAL_BEHAVIOR: inner.add_wal_batch() returned sequences: {:?}", sequences);

        // Store batch in Write Buffer-specific coordinator for backward compatibility and coordination
        tracing::debug!("🚀 WAL_BEHAVIOR: Updating batch_coordinator...");
        let mut coordinator = self.batch_coordinator.write().await;
        coordinator.add_batch(&collection_id, batch.clone())?;
        drop(coordinator);

        // Update WAL metrics
        tracing::debug!("🚀 WAL_BEHAVIOR: Updating WAL metrics...");
        let mut metrics = self.wal_metrics.write().await;
        metrics.entries_written += vector_count as u64;
        metrics.bytes_written += batch.total_size_bytes as u64;
        tracing::debug!(
            "🚀 WAL_BEHAVIOR: WAL metrics updated - entries_written: {}, bytes_written: {}",
            metrics.entries_written,
            metrics.bytes_written
        );
        drop(metrics);
        
        // Debug: Check what GlobalPartitionedMemtable stats look like
        let collection_stats = self.inner.get_all_collection_stats().await;
        tracing::debug!(
            "🚀 WAL_BEHAVIOR: After add, GlobalPartitionedMemtable has {} collections",
            collection_stats.len()
        );
        for (coll_id, (entry_count, size_bytes)) in &collection_stats {
            tracing::debug!(
                "🚀 WAL_BEHAVIOR: Collection {} has {} entries, {} bytes",
                coll_id,
                entry_count,
                size_bytes
            );
        }

        tracing::debug!(
            "✅ WAL_BEHAVIOR: Completed add_vector_batch for batch {} with sequences: {:?}",
            batch_id,
            sequences
        );

        Ok(sequences)
    }

    // Legacy get_unflushed_batches removed - use the modern GlobalPartitionedMemtable-based method

    /// Mark batch as flushed (Write Buffer-specific behavior)
    pub async fn mark_batch_flushed(&self, collection_id: &str, batch_id: &str) -> Result<()> {
        let mut coordinator = self.batch_coordinator.write().await;
        coordinator.mark_batch_flushed(collection_id, batch_id)?;
        
        tracing::info!("✅ WAL_BATCH: Marked batch {} as flushed", batch_id);
        Ok(())
    }

    /// Clear flushed batches from memory (Write Buffer-specific behavior)
    pub async fn clear_flushed_batches(&self, collection_id: &str) -> Result<usize> {
        let mut coordinator = self.batch_coordinator.write().await;
        let cleared_count = coordinator.clear_flushed_batches(collection_id)?;
        
        tracing::info!("🧹 WAL_BATCH: Cleared {} flushed batches", cleared_count);
        Ok(cleared_count)
    }
}

/// Write Buffer-specific implementation
impl WriteBufferBehaviorWrapper {
    // REMOVED: insert_wal_entry() - Use add_vector_batch() for unified API

    /// Get all vectors from the memtable (for recovery) - MODERN
    pub async fn get_all_vectors(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<VectorRecord>> {
        let vectors_with_sequences = self.inner.get_all_vectors(limit).await?;
        Ok(vectors_with_sequences.into_iter().map(|(_, vector)| vector).collect())
    }

    /// Search vectors in unflushed WAL data with configurable distance metric
    ///
    /// This searches the WAL memtable for similar vectors that haven't been flushed yet.
    /// Should be called BEFORE searching storage engines to get complete results.
    pub async fn search_unflushed_vectors(
        &self,
        query_vector: &[f32],
        k: usize,
        collection_id: &str,
        distance_metric: CoreDistanceMetric,
    ) -> Result<Vec<(f32, VectorRecord)>> {
        tracing::info!(
            "🔍 WAL_SEARCH: Searching unflushed vectors in collection {} (k={}) using {:?}",
            collection_id,
            k,
            distance_metric
        );

        let results = self
            .inner
            .search_vectors(query_vector, k, collection_id, distance_metric)
            .await?;

        eprintln!("🔍 WAL_SEARCH: Found {} unflushed results", results.len());
        tracing::info!("🔍 WAL_SEARCH: Found {} unflushed results", results.len());

        // Convert SimilarityResult back to f32 for compatibility with existing API
        let converted_results: Vec<(f32, VectorRecord)> = results
            .into_iter()
            .map(|(result, record)| (result.rank_value, record))
            .collect();
        Ok(converted_results)
    }

    /// Get vector by ID within a specific collection (MODERN)
    pub async fn get_vector_by_id(&self, collection_id: &str, vector_id: &str) -> Result<Option<VectorRecord>> {
        self.inner.get_vector_by_id(collection_id, vector_id).await
    }

    /// Check if global flush is needed
    pub async fn needs_global_flush(&self) -> Result<bool> {
        // Use the same logic as should_flush but with a more conservative threshold
        let size = self.size_bytes().await; // Use our actual size calculation
        let count = self.inner.len().await;

        // Global flush needed if we exceed larger thresholds
        Ok(size >= self.config.flush_threshold_bytes * 2 || count >= 50000)
    }

    /// Clear flushed entries for a specific collection (legacy compatibility wrapper)
    pub async fn clear_flushed_by_collection_id(
        &self,
        collection_id: &crate::core::String,
    ) -> Result<usize> {
        // Delegate to the string-based method
        self.clear_flushed(collection_id).await
    }

    // REMOVED: insert_batch() - Use add_vector_batch() for unified API

    // REMOVED: insert_with_sequence() - Use add_vector_batch() for unified API with custom sequence

    /// Check if flush is needed based on Write Buffer-specific thresholds
    pub async fn should_flush(&self) -> bool {
        let size = self.size_bytes().await; // Use our actual size calculation
        let count = self.inner.len().await;
        let size_threshold = self.config.flush_threshold_bytes;
        let count_threshold = 10000;

        let size_exceeds = size >= size_threshold;
        let count_exceeds = count >= count_threshold;
        let should_flush = size_exceeds || count_exceeds;

        tracing::info!(
            "🔍 FLUSH_CHECK: size={}B ({}MB), threshold={}B ({}MB), size_exceeds={}",
            size,
            size / 1024 / 1024,
            size_threshold,
            size_threshold / 1024 / 1024,
            size_exceeds
        );
        tracing::info!(
            "🔍 FLUSH_CHECK: count={}, threshold={}, count_exceeds={}",
            count,
            count_threshold,
            count_exceeds
        );
        tracing::info!(
            "🔍 FLUSH_CHECK: Final result - should_flush={} (size_exceeds={} OR count_exceeds={})",
            should_flush,
            size_exceeds,
            count_exceeds
        );

        should_flush
    }

    /// Get unflushed batches for collection (MODERN - for direct storage engine flush)
    pub async fn get_unflushed_batches(&self, collection_id: &str) -> Result<Vec<WriteBufferVectorBatch>> {
        // Return deserialized vector batches directly to storage engines
        // Storage engines handle their own serialization (SST for LSM, Parquet for VIPER)
        // This avoids double serialization/deserialization overhead
        
        let coordinator = self.batch_coordinator.read().await;
        let unflushed_batch_refs = coordinator.get_unflushed_batches(collection_id);
        
        // ZERO-COPY: Share Arc references instead of cloning entire batches
        let unflushed_batches = unflushed_batch_refs
            .into_iter()
            .map(|batch_ref| WriteBufferVectorBatch {
                batch_id: batch_ref.batch_id.clone(),
                vector_records: batch_ref.vector_records.clone(), // Arc clone (pointer copy)
                created_at: batch_ref.created_at,
                total_size_bytes: batch_ref.total_size_bytes,
                is_flushed: batch_ref.is_flushed,
            })
            .collect();
        
        Ok(unflushed_batches)
    }

    /// Clear flushed batches for collection (MODERN - after successful storage engine flush)
    pub async fn clear_flushed(&self, collection_id: &str) -> Result<usize> {
        // Clear data from GlobalPartitionedMemtable after successful storage engine flush
        // This prevents memory explosion and ensures data consistency
        self.inner.clear_flushed_batches(collection_id).await
    }

    /// Get statistics for WAL collection management (MODERN)
    pub async fn get_stats(&self) -> Result<HashMap<String, WriteBufferStats>> {
        let all_stats = self.inner.get_all_collection_stats().await;
        let mut stats_map = HashMap::new();
        
        for (collection_id, (vector_count, size_bytes)) in all_stats {
            stats_map.insert(collection_id, WriteBufferStats {
                total_entries: vector_count as u64,
                memory_entries: vector_count as u64,
                disk_segments: 0,
                total_disk_size_bytes: 0,
                memory_size_bytes: size_bytes as u64,
                collections_count: 1,
                last_flush_time: None,
                write_throughput_entries_per_sec: 0.0,
                read_throughput_entries_per_sec: 0.0,
                compression_ratio: 1.0,
            });
        }
        
        Ok(stats_map)
    }

    fn get_operation_type(&self, operation: &WriteBufferOperation) -> u8 {
        // Map operation types to numeric codes
        match operation.operation_type.as_str() {
            "upsert_batch" => 1,
            "delete_batch" => 2,
            _ => 0, // Unknown operation
        }
    }

    async fn serialize_operation(&self, operation: &WriteBufferOperation) -> Result<Vec<u8>> {
        Ok(bincode::serialize(operation)?)
    }

    /// Flush vectors up to sequence number (MODERN)
    pub async fn flush_all_vectors(&self) -> Result<Vec<VectorRecord>> {
        // Get all vectors from memtable
        let all_vectors_with_sequences = self.inner.get_all_vectors(None).await?;
        
        // Extract just the vectors
        let vectors_to_flush: Vec<VectorRecord> = all_vectors_with_sequences
            .into_iter()
            .map(|(_, vector)| vector)
            .collect();

        // Mark all batches as flushed (simulating successful storage engine write)
        self.inner.mark_all_batches_as_flushed().await?;

        // Remove flushed vectors
        self.inner.clear_all_flushed().await?;

        // MVCC tracking no longer uses sequences - batches handle versioning

        Ok(vectors_to_flush)
    }

    /// Get Write Buffer-specific metrics
    pub async fn get_wal_metrics(&self) -> WriteBufferMetrics {
        self.wal_metrics.read().await.clone()
    }

    /// Get latest version of a vector by ID (MODERN - MVCC handled by GlobalPartitionedMemtable)
    pub async fn get_latest_vector(&self, collection_id: &str, vector_id: &str) -> Result<Option<VectorRecord>> {
        // MVCC is now handled natively by GlobalPartitionedMemtable
        // No need for separate version tracking at WAL level
        self.inner.get_vector_by_id(collection_id, vector_id).await
    }

    // get_latest_version removed - use get_latest_vector with collection_id parameter

    /// Cleanup old versions (keep only N latest)
    pub async fn cleanup_versions(&self, vector_id: &str, keep_count: usize) -> Result<usize> {
        if !self.config.enable_mvcc {
            return Ok(0);
        }

        let mut versions = self.mvcc_versions.write().await;
        let sequences = match versions.get_mut(vector_id) {
            Some(seqs) => seqs,
            None => return Ok(0),
        };

        if sequences.len() <= keep_count {
            return Ok(0);
        }

        // Sort and keep only latest versions
        sequences.sort();
        let old_sequences = sequences
            .drain(0..sequences.len() - keep_count)
            .collect::<Vec<_>>();
        let removed_count = old_sequences.len();

        drop(versions);

        // MVCC cleanup no longer uses sequences - handled at batch level
        // Old entries are removed when batches are flushed

        Ok(removed_count)
    }
}

// REMOVED: Legacy MemtableCore<u64, WalEntry> trait implementation
// This trait used the deprecated WalEntry type which has been eliminated
// All functionality has been moved to modern batch-oriented methods

impl WriteBufferBehaviorWrapper {
    /// Get all vectors from all collections ordered by sequence (MODERN)
    pub async fn get_all_ordered(&self) -> Result<Vec<(u64, VectorRecord)>> {
        self.inner.get_all_vectors(None).await
    }

    /// Get all vectors for a specific collection (MODERN)
    pub async fn get_collection_vectors(
        &self,
        collection_id: &crate::core::String,
    ) -> Result<Vec<VectorRecord>> {
        // Direct access to collection vectors from GlobalPartitionedMemtable
        let vectors = self.inner.get_collection_vectors(&collection_id.to_string()).await?;
        
        tracing::debug!(
            "🚀 MODERN_GET_ALL: Returning {} vectors for collection {} (direct VectorRecord access)",
            vectors.len(),
            collection_id
        );
        
        Ok(vectors)
    }

    /// Get current size in bytes (with actual vector data size calculation)
    pub async fn size_bytes(&self) -> usize {
        // Direct access to GlobalPartitionedMemtable
        self.inner.size_bytes().await
    }

    /// Get current entry count
    pub async fn len(&self) -> usize {
        // Direct access to GlobalPartitionedMemtable
        self.inner.len().await
    }

    /// Get collections that need flushing (global WAL, collection-partitioned)
    pub async fn collections_needing_flush(&self) -> Result<Vec<crate::core::String>> {
        tracing::info!("🔍 COLLECTIONS_FLUSH_CHECK: Checking which collections need flushing...");

        // Use collection-aware flush detection from global partitioned memtable
        let threshold_bytes = self.config.flush_threshold_bytes;
        let collections_to_flush = self
            .inner
            .collections_needing_flush(threshold_bytes)
            .await?;

        tracing::info!(
            "🔍 COLLECTIONS_FLUSH_CHECK: {} collections need flushing with threshold {}MB",
            collections_to_flush.len(),
            threshold_bytes / 1024 / 1024
        );

        for collection_id in &collections_to_flush {
            let (entries, size) = self.inner.get_collection_stats(collection_id).await;
            tracing::info!(
                "🔍 COLLECTIONS_FLUSH_CHECK: Collection {} has {} entries, {} bytes ({}MB)",
                collection_id,
                entries,
                size,
                size / 1024 / 1024
            );
        }

        Ok(collections_to_flush)
    }


    /// Remove a specific batch from the memtable (for atomic rollback)
    pub async fn remove_batch(&self, collection_id: &str, batch_id: &str) -> Result<()> {
        // First remove from coordinator (for backward compatibility)
        let mut coordinator = self.batch_coordinator.write().await;
        
        // Remove batch from coordinator
        if let Some(collection_batches) = coordinator.batches.get_mut(collection_id) {
            if let Some(removed_batch) = collection_batches.remove(batch_id) {
                // Remove vector index entries for this batch
                for vector_record in removed_batch.vector_records.iter() {
                    if let Some(ref id) = vector_record.id {
                        coordinator.vector_index.remove(id);
                    }
                }
            }
        }
        drop(coordinator);
        
        // IMPORTANT: Also remove from the actual GlobalPartitionedMemtable
        // This is the real storage - coordinator is just for backward compatibility
        self.inner.remove_batch(collection_id, batch_id).await
    }

    /// Get statistics with String keys (legacy compatibility wrapper)
    pub async fn get_stats_by_collection_id(
        &self,
    ) -> Result<
        std::collections::HashMap<
            crate::core::String,
            crate::storage::persistence::write_buffer::WriteBufferStats,
        >,
    > {
        // Call the main get_stats method and convert String keys to String
        let string_stats = self.get_stats().await?;
        let mut result = std::collections::HashMap::new();
        for (k, v) in string_stats {
            result.insert(k, v);
        }
        Ok(result)
    }

    /// Search for specific vector by ID (MODERN)
    pub async fn search_vector(
        &self,
        collection_id: &crate::core::String,
        vector_id: &str,
    ) -> Result<Option<VectorRecord>> {
        self.inner.get_vector_by_id(collection_id, vector_id).await
    }

    /// Get vectors for specific collection with limit (MODERN)
    pub async fn get_collection_vectors_with_limit(
        &self,
        collection_id: &crate::core::String,
        limit: Option<usize>,
    ) -> Result<Vec<VectorRecord>> {
        let mut vectors = self.inner.get_collection_vectors(collection_id).await?;
        
        // Apply limit if specified
        if let Some(limit) = limit {
            vectors.truncate(limit);
        }
        
        Ok(vectors)
    }

    /// Get collection-specific statistics
    pub async fn get_collection_stats(
        &self,
        collection_id: &crate::core::String,
    ) -> Result<crate::storage::persistence::write_buffer::WriteBufferStats> {
        let all_stats = WriteBufferBehaviorWrapper::get_stats(self).await?;

        match all_stats.get(collection_id) {
            Some(stats) => Ok(stats.clone()),
            None => Ok(crate::storage::persistence::write_buffer::WriteBufferStats {
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
            }),
        }
    }

    /// Drop collection from memtable (MODERN)
    pub async fn drop_collection(
        &self,
        collection_id: &crate::core::String,
    ) -> Result<usize> {
        // Use the collection-specific clear method for efficient removal
        self.inner.clear_flushed_batches(collection_id).await
    }

    /// Perform maintenance operations
    pub async fn maintenance(&self) -> Result<crate::storage::persistence::write_buffer::WriteBufferStats> {
        // Cleanup old MVCC versions if enabled
        if self.config.enable_mvcc {
            let _cleaned = self.cleanup_versions("", 10).await?; // Keep 10 versions
        }

        // Return current stats after maintenance
        let all_stats = WriteBufferBehaviorWrapper::get_stats(self).await?;

        // Return aggregated stats
        let total_entries: u64 = all_stats.values().map(|s| s.total_entries).sum();
        let total_memory: u64 = all_stats.values().map(|s| s.memory_size_bytes).sum();

        Ok(crate::storage::persistence::write_buffer::WriteBufferStats {
            total_entries,
            memory_entries: total_entries,
            disk_segments: 0,
            total_disk_size_bytes: 0,
            memory_size_bytes: total_memory,
            collections_count: all_stats.len(),
            last_flush_time: None,
            write_throughput_entries_per_sec: 0.0,
            read_throughput_entries_per_sec: 0.0,
            compression_ratio: 1.0,
        })
    }

    /// Get unflushed batches for atomic flush (MODERN)
    pub async fn atomic_mark_for_flush(
        &self,
        collection_id: &crate::core::String,
        _up_to_sequence: u64,
    ) -> Result<Vec<WriteBufferVectorBatch>> {
        // Return unflushed batches directly for storage engine processing
        self.get_unflushed_batches(collection_id).await
    }

    /// Complete flush and remove marked entries
    pub async fn complete_flush_removal(
        &self,
        collection_id: &crate::core::String,
    ) -> Result<usize> {
        self.clear_flushed(collection_id).await
    }

    /// Abort flush and restore batches
    pub async fn abort_flush_restore(
        &self,
        collection_id: &crate::core::String,
        _batches: Vec<WriteBufferVectorBatch>,
    ) -> Result<()> {
        // In a real implementation, this would restore the batches
        // For now, this is a no-op since we haven't actually removed them
        tracing::warn!(
            "Flush aborted for collection {}, batches preserved in memtable",
            collection_id
        );
        Ok(())
    }
}

// OrderedWalEntry removed - use WriteBufferVectorBatch for batch operations

// OrderedVectorRecord removed - storage engines handle their own ordering and serialization
// VectorRecord is passed directly to avoid double serialization overhead

/// Write Buffer-specific metrics
#[derive(Debug, Clone, Default)]
pub struct WriteBufferMetrics {
    pub entries_written: u64,
    pub bytes_written: u64,
    pub flushes_performed: u64,
    pub total_flushed_bytes: u64,
    pub recovery_operations: u64,
    pub mvcc_versions_active: usize,
}

/// Flush coordination state
#[derive(Debug, Clone, Default)]
struct FlushState {
    last_flush_sequence: u64,
    flush_in_progress: bool,
    flush_start_time: Option<std::time::Instant>,
}

#[cfg(test)]
mod tests {
    use super::*;
    // Tests now use unified add_vector_batch API only

    #[tokio::test]
    async fn test_wal_behavior_wrapper() {
        let config = MemtableConfig::default();
        let wal_wrapper = WriteBufferBehaviorWrapper::new(config);

        // Create test vector records using the new unified API
        let now = chrono::Utc::now().timestamp_millis();
        let vector_record1 = crate::core::VectorRecord {
            id: Some("test_vector_1".to_string()),
            vector: vec![0.1, 0.2, 0.3],
            metadata: vec![],
            timestamp: now,
            created_at: now,
            updated_at: now,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };

        let vector_record2 = crate::core::VectorRecord {
            id: Some("test_vector_2".to_string()),
            vector: vec![0.4, 0.5, 0.6],
            metadata: vec![],
            timestamp: now + 1,
            created_at: now + 1,
            updated_at: now + 1,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };

        // Test first batch insertion using unified add_vector_batch API
        let batch1 = WriteBufferVectorBatch {
            batch_id: BatchId::new(),
            vector_records: Arc::new(vec![vector_record1]),
            created_at: std::time::SystemTime::now(),
            total_size_bytes: 1024,
            is_flushed: false,
        };
        
        let sequences1 = wal_wrapper
            .add_vector_batch("test_collection", batch1)
            .await
            .unwrap();
        let seq1 = sequences1[0];

        // Test second batch insertion
        let batch2 = WriteBufferVectorBatch {
            batch_id: BatchId::new(),
            vector_records: Arc::new(vec![vector_record2]),
            created_at: std::time::SystemTime::now(),
            total_size_bytes: 1024,
            is_flushed: false,
        };
        
        let sequences2 = wal_wrapper
            .add_vector_batch("test_collection", batch2)
            .await
            .unwrap();
        let seq2 = sequences2[0];

        // Verify sequence ordering (newer batches get higher sequences)
        assert!(seq2 > seq1);
        
        // Verify batch count
        let unflushed_batches = wal_wrapper.get_unflushed_batches("test_collection").await.unwrap();
        assert_eq!(unflushed_batches.len(), 2);

        // Test retrieving all vectors
        let entries = wal_wrapper.get_all_vectors(None).await.unwrap();
        assert_eq!(entries.len(), 2);

        // Test flush threshold
        assert!(!wal_wrapper.should_flush().await); // Small entries shouldn't trigger flush

        // Test flush operation
        let flushed = wal_wrapper.flush_all_vectors().await.unwrap();
        assert_eq!(flushed.len(), 2);
        assert_eq!(wal_wrapper.len().await, 0);

        // Test metrics
        let metrics = wal_wrapper.get_wal_metrics().await;
        assert_eq!(metrics.entries_written, 2);
        assert_eq!(metrics.flushes_performed, 0); // flush_up_to_sequence doesn't update this metric
    }

    #[tokio::test]
    async fn test_wal_mvcc_functionality() {
        let mut config = MemtableConfig::default();
        config.enable_mvcc = true;

        let wal_wrapper = WriteBufferBehaviorWrapper::new(config);

        let vector_id = "test_vector_mvcc";

        // Insert multiple versions of the same vector using unified API
        for i in 0..3 {
            let now = chrono::Utc::now().timestamp_millis();
            let vector_record = crate::core::VectorRecord {
                id: Some(vector_id.to_string()),
                vector: vec![i as f32, (i + 1) as f32],
                metadata: vec![
                    crate::proto::proximadb::MetadataItem {
                        key: "version".to_string(),
                        value: Some(crate::proto::proximadb::metadata_item::Value::StringValue(i.to_string())),
                    }
                ],
                timestamp: now + i as i64,
                created_at: now + i as i64,
                updated_at: now + i as i64,
                expires_at: None,
                version: (i + 1) as i64,
                rank: None,
                score: None,
                distance: None,
            };

            let batch = WriteBufferVectorBatch {
                batch_id: BatchId::new(),
                vector_records: Arc::new(vec![vector_record]),
                created_at: std::time::SystemTime::now(),
                total_size_bytes: 1024,
                is_flushed: false,
            };
            
            wal_wrapper.add_vector_batch("test_collection", batch).await.unwrap();
        }

        // Test that vectors were added (using modern API)
        let all_vectors = wal_wrapper.get_all_ordered().await.unwrap();
        assert!(all_vectors.len() >= 3);

        // Test that we can retrieve vectors by collection
        let collection_vectors = wal_wrapper.get_all_vectors(None).await.unwrap();
        assert!(!collection_vectors.is_empty());

        // Verify vector data integrity
        let found_vectors: Vec<_> = all_vectors.iter()
            .filter(|(_, record)| record.id.as_deref().unwrap_or("") == vector_id)
            .collect();
        assert!(!found_vectors.is_empty());
    }
}
