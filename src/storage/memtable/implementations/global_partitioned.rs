//! Global Partitioned Memtable Implementation for WAL
//!
//! Optimized for global WAL with collection partitioning:
//! - Global sequence ordering for flush coordination
//! - Per-collection data partitions for efficient operations
//! - Content-based search within collections
//! - Efficient per-collection flush isolation

use anyhow::Result;
// async_trait removed - no longer implementing trait methods
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::super::core::MemtableMetrics;
use crate::compute::distance::DistanceMetric as CoreDistanceMetric;
use crate::compute::unified_distance::{
    DistanceComputeProvider, DistanceResultOrdering, UnifiedDistanceCompute,
};
use crate::core::VectorRecord;
// WalEntry removed - working directly with VectorRecord and WalVectorBatch

/// Collection partition within the global memtable
#[derive(Debug)]
struct CollectionPartition {
    /// WAL Batches stored as native deserialized batches (PRIMARY STORAGE)
    wal_batches: HashMap<String, crate::storage::memtable::specialized::wal_behavior::WalVectorBatch>,

    /// Vector ID to batch lookup index for fast get operations
    vector_id_index: HashMap<String, String>, // vector_id -> batch_id

    /// Collection statistics
    total_size: usize,
    vector_count: usize,
    batch_count: usize,
    last_flush_sequence: u64,
    created_at: std::time::SystemTime,
}

impl CollectionPartition {
    fn new() -> Self {
        Self {
            wal_batches: HashMap::new(),
            vector_id_index: HashMap::new(),
            total_size: 0,
            vector_count: 0,
            batch_count: 0,
            last_flush_sequence: 0,
            created_at: std::time::SystemTime::now(),
        }
    }

    /// Add WAL batch to this collection partition
    fn add_batch(&mut self, batch: crate::storage::memtable::specialized::wal_behavior::WalVectorBatch) -> Result<()> {
        let batch_id = batch.batch_id.batch_uuid.clone();
        let batch_size = batch.total_size_bytes;
        let vector_count = batch.vector_records.len();

        // Update vector ID index for fast lookups
        for vector_record in &batch.vector_records {
            if !vector_record.id.is_empty() {
                self.vector_id_index.insert(vector_record.id.clone(), batch_id.clone());
            }
        }

        // Store the batch
        self.wal_batches.insert(batch_id, batch);

        // Update statistics
        self.total_size += batch_size;
        self.vector_count += vector_count;
        self.batch_count += 1;

        Ok(())
    }

    /// Get vector by ID within this collection with MVCC + logical delete support
    fn get_vector_by_id(&self, vector_id: &str) -> Option<VectorRecord> {
        // 🔧 FLEXIBLE: Skip immutable vectors (those without client-provided IDs)
        if vector_id.is_empty() {
            return None;
        }
        
        let current_time = chrono::Utc::now().timestamp_micros();
        let mut latest_record: Option<(VectorRecord, u64, i64)> = None; // (record, sequence, version)
        
        // Search through all batches to find the latest version
        for batch in self.wal_batches.values() {
            for vector_record in &batch.vector_records {
                if !vector_record.id.is_empty() && vector_record.id == vector_id {
                    let sequence = batch.batch_id.sequence_range.0;
                    let version = vector_record.version;
                    
                    // Check if this is a newer version
                    let is_newer = match &latest_record {
                        Some((_, existing_seq, existing_version)) => {
                            sequence > *existing_seq || (sequence == *existing_seq && version > *existing_version)
                        }
                        None => true, // First occurrence
                    };
                    
                    if is_newer {
                        latest_record = Some((vector_record.clone(), sequence, version));
                    }
                }
            }
        }
        
        // Check the latest record we found
        if let Some((record, _, _)) = latest_record {
            // Check if it's expired (logical delete)
            let is_expired = record.expires_at
                .map(|expires| expires < current_time)
                .unwrap_or(false);
            
            if is_expired {
                tracing::debug!("🗑️ Vector {} found but expired (tombstone)", vector_id);
                return None; // Logically deleted
            }
            
            return Some(record);
        }
        
        None
    }

    /// Clear batches up to sequence number within this collection
    fn clear_up_to(&mut self, up_to_seq: u64) -> usize {
        let mut cleared_count = 0;
        let mut removed_size = 0;

        // Find batches to remove based on sequence range
        let batch_ids_to_remove: Vec<String> = self.wal_batches
            .iter()
            .filter(|(_, batch)| {
                // Remove if batch is flushed or if all sequences are <= up_to_seq
                batch.is_flushed || batch.batch_id.sequence_range.1 <= up_to_seq
            })
            .map(|(id, _)| id.clone())
            .collect();
        
        for batch_id in batch_ids_to_remove {
            if let Some(batch) = self.wal_batches.remove(&batch_id) {
                // Remove vector IDs from index
                for vector_record in &batch.vector_records {
                    if !vector_record.id.is_empty() {
                        self.vector_id_index.remove(&vector_record.id);
                    }
                }
                
                cleared_count += batch.vector_records.len();
                removed_size += batch.total_size_bytes;
                self.batch_count = self.batch_count.saturating_sub(1);
            }
        }

        self.vector_count = self.vector_count.saturating_sub(cleared_count);
        self.total_size = self.total_size.saturating_sub(removed_size);
        self.last_flush_sequence = up_to_seq;

        cleared_count
    }

    /// Check if this collection needs flushing
    fn needs_flush(&self, size_threshold: usize, count_threshold: usize) -> bool {
        self.total_size >= size_threshold || self.vector_count >= count_threshold
    }

    /// Get all vectors for iteration or flush operations with MVCC + logical delete support
    fn get_all_vectors(&self) -> Vec<VectorRecord> {
        use std::collections::HashMap;
        
        let mut id_to_latest: HashMap<String, (VectorRecord, u64, i64)> = HashMap::new(); // (record, sequence, version)
        let mut vectors_without_id = Vec::new();
        let current_time = chrono::Utc::now().timestamp_micros();
        
        // Collect latest versions for each ID
        for batch in self.wal_batches.values() {
            for vector_record in &batch.vector_records {
                let sequence = batch.batch_id.sequence_range.0;
                let version = vector_record.version;
                
                if !vector_record.id.is_empty() {
                    // Check if this is the latest version
                    let is_newer = match id_to_latest.get(&vector_record.id) {
                        Some((_, existing_seq, existing_version)) => {
                            sequence > *existing_seq || (sequence == *existing_seq && version > *existing_version)
                        }
                        None => true,
                    };
                    
                    if is_newer {
                        id_to_latest.insert(
                            vector_record.id.clone(),
                            (vector_record.clone(), sequence, version)
                        );
                    }
                } else {
                    // No ID - include directly if not expired
                    let is_expired = vector_record.expires_at
                        .map(|expires| expires < current_time)
                        .unwrap_or(false);
                    
                    if !is_expired {
                        vectors_without_id.push(vector_record.clone());
                    }
                }
            }
        }
        
        // Collect final results, filtering out expired records
        let mut vectors = Vec::new();
        
        for (_, (record, _, _)) in id_to_latest {
            let is_expired = record.expires_at
                .map(|expires| expires < current_time)
                .unwrap_or(false);
            
            if !is_expired {
                vectors.push(record);
            }
        }
        
        vectors.extend(vectors_without_id);
        vectors
    }

    /// Search for similar vectors using native batch processing with MVCC + logical deletes
    fn search_vectors(
        &self,
        query_vector: &[f32],
        distance_metric: &CoreDistanceMetric,
        distance_compute: &UnifiedDistanceCompute,
    ) -> Vec<(f32, VectorRecord)> {
        use std::collections::HashMap;
        
        let mut id_to_latest: HashMap<String, (f32, VectorRecord, u64, i64)> = HashMap::new(); // Added version
        let mut results_without_id = Vec::new();
        let current_time = chrono::Utc::now().timestamp_micros();

        // Search native WAL batches with MVCC logic
        for (batch_id, wal_batch) in &self.wal_batches {
            tracing::debug!("🔍 Searching native WAL batch {} with {} vectors", batch_id, wal_batch.vector_records.len());
            
            for vector_record in &wal_batch.vector_records {
                // Skip expired records (logical deletes)
                let is_expired = vector_record.expires_at
                    .map(|expires| expires < current_time)
                    .unwrap_or(false);
                
                if is_expired {
                    // This is a tombstone/delete - mark ID as deleted
                    if !vector_record.id.is_empty() {
                        id_to_latest.remove(&vector_record.id);
                        tracing::debug!("🗑️ Tombstone found for ID {}, removing from results", vector_record.id);
                    }
                    continue;
                }
                
                let score = distance_compute.calculate_distance(query_vector, &vector_record.vector, distance_metric);
                let sequence = wal_batch.batch_id.sequence_range.0;
                let version = vector_record.version;
                
                // MVCC: Keep only latest version by (sequence, version) for same ID
                if !vector_record.id.is_empty() {
                    match id_to_latest.get(&vector_record.id) {
                        Some((_, _, existing_seq, existing_version)) => {
                            // Skip if we have a newer version already
                            if sequence < *existing_seq || (sequence == *existing_seq && version <= *existing_version) {
                                continue;
                            }
                        }
                        None => {}
                    }
                    
                    // Keep this entry (newer version or first occurrence)
                    id_to_latest.insert(
                        vector_record.id.clone(),
                        (score, vector_record.clone(), sequence, version)
                    );
                    
                    tracing::debug!("📝 Updated latest version for ID {}: seq={}, version={}", 
                                   vector_record.id, sequence, version);
                } else {
                    // No ID - include directly (no MVCC possible)
                    results_without_id.push((score, vector_record.clone()));
                }
            }
        }

        // Combine deduplicated ID-based results with non-ID results
        let mut final_results = Vec::new();
        
        // Count results before moving
        let unique_ids_count = id_to_latest.len();
        let results_without_id_count = results_without_id.len();
        
        // Add latest version of each ID
        for (_, (score, vector_record, _, _)) in id_to_latest {
            final_results.push((score, vector_record));
        }
        
        // Add non-ID results
        final_results.extend(results_without_id);

        tracing::debug!(
            "🔍 Search results: {} batches searched, {} unique IDs, {} without ID, {} final results",
            self.wal_batches.len(),
            unique_ids_count,
            results_without_id_count,
            final_results.len()
        );

        final_results
    }

    // Legacy append method removed - use add_batch() directly with WalVectorBatch
}

/// Global partitioned memtable implementation for WAL operations
///
/// This implements a two-tier index structure:
/// 1. Global sequence ordering for flush coordination
/// 2. Per-collection data partitions with batch-based storage
#[derive(Debug)]
pub struct GlobalPartitionedMemtable {
    /// Global sequence generator for cross-collection ordering
    global_sequence: AtomicU64,

    /// Per-collection data partitions (collection_id -> partition)
    collections: Arc<RwLock<HashMap<String, CollectionPartition>>>,

    /// Unified distance computation manager
    distance_compute: UnifiedDistanceCompute,

    /// Global metrics
    metrics: Arc<RwLock<MemtableMetrics>>,
}

impl GlobalPartitionedMemtable {
    /// Create new global partitioned memtable
    pub fn new() -> Self {
        Self {
            global_sequence: AtomicU64::new(1),
            collections: Arc::new(RwLock::new(HashMap::new())),
            distance_compute: UnifiedDistanceCompute::default(),
            metrics: Arc::new(RwLock::new(MemtableMetrics::default())),
        }
    }

    /// Add native WAL batch to the appropriate collection partition (STREAMLINED ARCHITECTURE)
    pub async fn add_wal_batch(&self, wal_batch: crate::storage::memtable::specialized::wal_behavior::WalVectorBatch) -> Result<Vec<u64>> {
        let collection_id = wal_batch.batch_id.collection_id.clone();
        let batch_id = wal_batch.batch_id.batch_uuid.clone();
        let vector_count = wal_batch.vector_records.len();
        let batch_size = wal_batch.total_size_bytes;
        
        tracing::debug!(
            "🚀 NATIVE_BATCH_ADD: Adding WAL batch {} to collection {} ({} vectors, {} bytes)",
            batch_id,
            collection_id,
            vector_count,
            batch_size
        );

        // Generate global sequences for the batch
        let start_seq = self.global_sequence.load(Ordering::SeqCst);
        let sequences: Vec<u64> = (start_seq..start_seq + vector_count as u64).collect();
        self.global_sequence.store(start_seq + vector_count as u64, Ordering::SeqCst);

        // Get or create collection partition
        let mut collections = self.collections.write().await;
        let partition_exists = collections.contains_key(&collection_id);
        let partition = collections
            .entry(collection_id.clone())
            .or_insert_with(CollectionPartition::new);

        tracing::debug!(
            "🚀 GLOBAL_PARTITIONED_DEBUG: Adding batch to partition for collection {}, partition existed: {}",
            collection_id,
            partition_exists
        );

        // Store the batch natively in the partition
        partition.add_batch(wal_batch)?;
        
        tracing::debug!(
            "🚀 GLOBAL_PARTITIONED_DEBUG: Updated partition stats - vector_count: {}, total_size: {}",
            partition.vector_count,
            partition.total_size
        );
        
        drop(collections);

        // Update global metrics
        let mut metrics = self.metrics.write().await;
        metrics.insert_count += 1; // One batch insert
        metrics.entry_count += vector_count; // Multiple vectors
        metrics.size_bytes += batch_size;

        tracing::info!(
            "✅ NATIVE_BATCH_COMPLETE: Added batch {} with sequences {:?} (collection={}, vectors={}, bytes={})",
            batch_id,
            sequences,
            collection_id,
            vector_count,
            batch_size
        );

        Ok(sequences)
    }

    // Legacy append() method removed - use add_wal_batch() with modern WalVectorBatch architecture

    /// Get vector by global sequence number (MODERN - returns VectorRecord directly)
    pub async fn get_vector_by_sequence(&self, sequence: u64) -> Result<Option<VectorRecord>> {
        let collections = self.collections.read().await;

        // Linear search through all collections (could be optimized with sequence->collection mapping)
        for partition in collections.values() {
            // Search through native WAL batches
            for batch in partition.wal_batches.values() {
                if batch.batch_id.sequence_range.0 <= sequence && sequence <= batch.batch_id.sequence_range.1 {
                    // Find vector record at this sequence within the batch
                    for (index, vector_record) in batch.vector_records.iter().enumerate() {
                        let entry_sequence = batch.batch_id.sequence_range.0 + index as u64;
                        if entry_sequence == sequence {
                            return Ok(Some(vector_record.clone()));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    // Legacy get_by_content() removed - use vector search methods instead

    /// Search for similar vectors within a specific collection using configurable distance metric
    pub async fn search_vectors(
        &self,
        query_vector: &[f32],
        k: usize,
        collection_id: &str,
        distance_metric: CoreDistanceMetric,
    ) -> Result<Vec<(f32, VectorRecord)>> {
        let collections = self.collections.read().await;
        
        eprintln!("🔍 GLOBAL_PARTITIONED_SEARCH: Searching for collection_id '{}' in {} collections", collection_id, collections.len());
        for (id, partition) in collections.iter() {
            eprintln!("🔍 Available collection: '{}' with {} vectors", id, partition.vector_count);
        }

        if let Some(partition) = collections.get(collection_id) {
            let mut results = partition.search_vectors(
                query_vector,
                &distance_metric,
                &self.distance_compute,
            );

            // Sort and limit results using unified distance system
            DistanceResultOrdering::sort_and_limit(
                &mut results,
                &distance_metric,
                &self.distance_compute,
                k,
            );

            tracing::debug!("📊 GLOBAL_PARTITIONED_SEARCH: Found {} results in collection {} (partition has {} vectors) using {:?}", 
                           results.len(), collection_id, partition.vector_count, distance_metric);
            Ok(results)
        } else {
            tracing::debug!(
                "📊 GLOBAL_PARTITIONED_SEARCH: Collection {} not found",
                collection_id
            );
            Ok(Vec::new())
        }
    }

    /// Get vector by ID within a specific collection (MODERN - no deserialization)
    pub async fn get_vector_by_id(&self, collection_id: &str, vector_id: &str) -> Result<Option<VectorRecord>> {
        let collections = self.collections.read().await;

        if let Some(partition) = collections.get(collection_id) {
            Ok(partition.get_vector_by_id(vector_id))
        } else {
            Ok(None)
        }
    }

    /// Get all vectors for a specific collection (MODERN - returns VectorRecord directly)
    pub async fn get_collection_vectors(&self, collection_id: &str) -> Result<Vec<VectorRecord>> {
        let collections = self.collections.read().await;

        if let Some(partition) = collections.get(collection_id) {
            let vectors = partition.get_all_vectors();
            
            tracing::debug!(
                "🚀 COLLECTION_VECTORS: Returning {} vectors from {} native batches",
                vectors.len(),
                partition.wal_batches.len()
            );
            
            Ok(vectors)
        } else {
            Ok(Vec::new())
        }
    }

    /// Get collection statistics
    pub async fn get_collection_stats(&self, collection_id: &str) -> (usize, usize) {
        let collections = self.collections.read().await;

        if let Some(partition) = collections.get(collection_id) {
            (partition.vector_count, partition.total_size)
        } else {
            (0, 0)
        }
    }

    /// Clear entries for a specific collection up to sequence number
    pub async fn clear_collection_up_to(
        &self,
        collection_id: &str,
        up_to_seq: u64,
    ) -> Result<usize> {
        let mut collections = self.collections.write().await;

        if let Some(partition) = collections.get_mut(collection_id) {
            let cleared_count = partition.clear_up_to(up_to_seq);

            // Update global metrics
            let mut metrics = self.metrics.write().await;
            metrics.entry_count = metrics.entry_count.saturating_sub(cleared_count);
            // Note: size_bytes will be recalculated in next size_bytes() call

            tracing::debug!(
                "📊 GLOBAL_PARTITIONED: Cleared {} entries from collection {} up to sequence {}",
                cleared_count,
                collection_id,
                up_to_seq
            );

            Ok(cleared_count)
        } else {
            Ok(0)
        }
    }

    /// Get collections that need flushing based on thresholds
    pub async fn collections_needing_flush(&self, size_threshold: usize) -> Result<Vec<String>> {
        let collections = self.collections.read().await;
        let count_threshold = 10000; // Could be configurable

        let mut collections_to_flush = Vec::new();
        let mut total_collections = 0;

        for (collection_id, partition) in collections.iter() {
            total_collections += 1;
            if partition.needs_flush(size_threshold, count_threshold) {
                collections_to_flush.push(collection_id.clone());
                tracing::debug!(
                    "📊 GLOBAL_PARTITIONED: Collection {} needs flush - {} entries, {} bytes",
                    collection_id,
                    partition.vector_count,
                    partition.total_size
                );
            }
        }

        tracing::debug!(
            "📊 GLOBAL_PARTITIONED: {} of {} collections need flushing",
            collections_to_flush.len(),
            total_collections
        );

        Ok(collections_to_flush)
    }

    /// Get metrics for external access
    pub async fn get_metrics(&self) -> MemtableMetrics {
        self.metrics.read().await.clone()
    }

    /// Update metrics externally (for specialized behavior wrappers)
    pub async fn update_metrics<F>(&self, updater: F) -> Result<()>
    where
        F: FnOnce(&mut MemtableMetrics),
    {
        let mut metrics = self.metrics.write().await;
        updater(&mut *metrics);
        Ok(())
    }

    /// Get vectors from sequence number onwards (for recovery) - MODERN
    pub async fn get_vectors_from_sequence(
        &self,
        from_seq: u64,
        limit: Option<usize>,
    ) -> Result<Vec<(u64, VectorRecord)>> {
        let collections = self.collections.read().await;
        let mut all_vectors = Vec::new();

        // Collect all vectors from all collections with their sequences
        for partition in collections.values() {
            for batch in partition.wal_batches.values() {
                for (index, vector_record) in batch.vector_records.iter().enumerate() {
                    let vector_sequence = batch.batch_id.sequence_range.0 + index as u64;
                    if vector_sequence >= from_seq {
                        all_vectors.push((vector_sequence, vector_record.clone()));
                    }
                }
            }
        }

        // Sort by global sequence
        all_vectors.sort_by_key(|(seq, _)| *seq);

        // Apply limit if specified
        if let Some(limit) = limit {
            all_vectors.truncate(limit);
        }

        Ok(all_vectors)
    }

    /// Clear entries up to sequence number (global operation)
    pub async fn clear_up_to(&self, up_to_seq: u64) -> Result<usize> {
        let mut collections = self.collections.write().await;
        let mut total_cleared = 0;

        for partition in collections.values_mut() {
            total_cleared += partition.clear_up_to(up_to_seq);
        }

        // Update global metrics
        let mut metrics = self.metrics.write().await;
        metrics.entry_count = metrics.entry_count.saturating_sub(total_cleared);

        Ok(total_cleared)
    }

    /// Clear all vectors and batches
    pub async fn clear(&self) -> Result<()> {
        let mut collections = self.collections.write().await;
        collections.clear();
        drop(collections);

        let mut metrics = self.metrics.write().await;
        *metrics = MemtableMetrics::default();

        // Reset global sequence
        self.global_sequence.store(1, Ordering::SeqCst);

        Ok(())
    }

    /// Get current number of entries across all collections
    pub async fn len(&self) -> usize {
        let collections = self.collections.read().await;
        collections.values().map(|p| p.vector_count).sum()
    }

    /// Get current size in bytes across all collections
    pub async fn size_bytes(&self) -> usize {
        let collections = self.collections.read().await;
        collections.values().map(|p| p.total_size).sum()
    }

    /// Check if empty
    pub async fn is_empty(&self) -> bool {
        let collections = self.collections.read().await;
        collections.is_empty() || collections.values().all(|p| p.vector_count == 0)
    }

    /// Get statistics for all collections
    pub async fn get_all_collection_stats(&self) -> HashMap<String, (usize, usize)> {
        let collections = self.collections.read().await;
        collections
            .iter()
            .map(|(id, partition)| (id.clone(), (partition.vector_count, partition.total_size)))
            .collect()
    }
}

// MemtableCore trait removed - GlobalPartitionedMemtable works directly with VectorRecord/WalVectorBatch

impl GlobalPartitionedMemtable {
    /// Get all vectors (for flush operations) - MODERN
    pub async fn get_all_vectors(&self) -> Result<Vec<VectorRecord>> {
        let vectors_with_sequences = self.get_vectors_from_sequence(0, None).await?;
        Ok(vectors_with_sequences.into_iter().map(|(_, vector)| vector).collect())
    }
}

impl Default for GlobalPartitionedMemtable {
    fn default() -> Self {
        Self::new()
    }
}

// Tests moved to src/storage/memtable/implementations/tests/global_partitioned_tests.rs

impl DistanceComputeProvider for GlobalPartitionedMemtable {
    fn distance_compute(&self) -> &UnifiedDistanceCompute {
        &self.distance_compute
    }
}
